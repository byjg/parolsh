//! Running shell commands: `!command`, `!bash` and `parolsh -c`.

use nix::errno::Errno;
use nix::sys::signal::{SigSet, Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, getpgrp, tcsetpgrp};
use std::io::{IsTerminal, Read, Stdin, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// `!command`: the configured shell (default `bash -ic`, so `~/.bashrc`
/// aliases and functions work) with the command as the last argument.
pub fn command(shell: &[String], line: &str, cwd: &Path) -> Command {
    let mut command = Command::new(&shell[0]);
    command
        .args(&shell[1..])
        .arg(line)
        .current_dir(cwd)
        // Keep `!` commands out of the user's bash history.
        .env("HISTFILE", "/dev/null");
    command
}

/// `!bash`: a real interactive Bash session.
pub fn bash(cwd: &Path) -> Command {
    let mut command = Command::new("bash");
    command.current_dir(cwd);
    command
}

/// `parolsh -c`: a plain, non-interactive `bash -c`, as scripts calling
/// `$SHELL -c` expect. `args` become `$0`, `$1`, ...
pub fn passthrough(line: &str, args: &[String]) -> Command {
    let mut command = Command::new("bash");
    command.arg("-c").arg(line).args(args);
    command
}

/// Runs an interactive command as the terminal's foreground job, the way a
/// shell does: in its own process group, which owns the terminal while it
/// runs. Parolsh takes the terminal back afterwards, whatever the command did
/// with it (an interactive bash may keep it). Parolsh has no job control: if
/// the process it started is stopped with Ctrl+Z, it is resumed. Programs run
/// by an interactive bash (`bash -ic`) are paused by bash's own job control
/// instead. Returns the shell-style exit code.
pub fn run_foreground(command: Command) -> std::io::Result<i32> {
    run_job(command, |_| {})
}

/// The output of a `!+` command, for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub text: String,
    /// Only the last `limit` bytes were kept.
    pub truncated: bool,
}

/// `!+command`: like `!command`, but the command's stdout and stderr go
/// through Parolsh, which prints them as they come and keeps the last
/// `limit` bytes. The command sees pipes, not a terminal, so it prints plain
/// text; its input is still the terminal.
///
/// Only the command is captured, not the shell around it: the pipes are its
/// file descriptors 3 and 4, and the line runs as `{ line } >&3 2>&4`. What
/// the shell prints itself (an interactive bash reading `~/.bashrc`) goes to
/// the terminal as usual.
pub fn run_shared(
    shell: &[String],
    line: &str,
    cwd: &Path,
    limit: usize,
) -> std::io::Result<(i32, Captured)> {
    let (out_read, out_write) = std::io::pipe()?;
    let (err_read, err_write) = std::io::pipe()?;
    let wrapped = format!("{{ {line}\n}} >&3 2>&4 3>&- 4>&-");
    let mut command = self::command(shell, &wrapped, cwd);
    let (out_fd, err_fd) = (out_write.as_raw_fd(), err_write.as_raw_fd());
    // SAFETY: between fork and exec only dup2 runs, which is
    // async-signal-safe. dup2 clears close-on-exec on the new descriptors,
    // so the child keeps 3 and 4; the originals close on exec.
    unsafe {
        command.pre_exec(move || {
            for (from, to) in [(out_fd, 3), (err_fd, 4)] {
                if libc::dup2(from, to) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let tail = Arc::new(Mutex::new(Tail::new(limit)));
    let (done_tx, done_rx) = mpsc::channel();
    copy(out_read, std::io::stdout(), tail.clone(), done_tx.clone());
    copy(err_read, std::io::stderr(), tail.clone(), done_tx);
    let code = run_job(command, move |_| {
        // Our write ends, so the reads end when the command's do.
        drop((out_write, err_write));
    })?;
    // The copies end when the pipes close. A background process started by
    // the command may keep them open: do not wait for it.
    for _ in 0..2 {
        if done_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            break;
        }
    }
    let tail = tail.lock().map(|tail| tail.captured()).unwrap_or(Captured {
        text: String::new(),
        truncated: false,
    });
    Ok((code, tail))
}

/// Runs `command` as the terminal's foreground job. `start` runs right
/// after the spawn, to take the child's pipes.
fn run_job(mut command: Command, start: impl FnOnce(&mut Child)) -> std::io::Result<i32> {
    let terminal = std::io::stdin();
    if !terminal.is_terminal() {
        let mut child = command.spawn()?;
        start(&mut child);
        return child.wait().map(exit_code);
    }

    command.process_group(0);
    let mut child = command.spawn()?;
    start(&mut child);
    let pid = Pid::from_raw(child.id() as i32);

    // `spawn` returns after exec, so the child already leads its own group.
    // It may exit before this; then there is nothing to hand over.
    let _ = give_terminal(&terminal, pid);
    let code = wait_resuming(pid);
    give_terminal(&terminal, getpgrp())?;
    code
}

/// Copies a child's pipe to `to` while keeping its tail, on its own thread.
fn copy(
    mut from: impl Read + Send + 'static,
    mut to: impl Write + Send + 'static,
    tail: Arc<Mutex<Tail>>,
    done: mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(read) = from.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let _ = to.write_all(&buffer[..read]);
            let _ = to.flush();
            if let Ok(mut tail) = tail.lock() {
                tail.push(&buffer[..read]);
            }
        }
        let _ = done.send(());
    });
}

/// The last `limit` bytes written.
struct Tail {
    bytes: Vec<u8>,
    limit: usize,
    dropped: bool,
}

impl Tail {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            dropped: false,
        }
    }

    fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
        // Trim in batches, not on every write.
        if self.bytes.len() > self.limit * 2 {
            let cut = self.bytes.len() - self.limit;
            self.bytes.drain(..cut);
            self.dropped = true;
        }
    }

    fn captured(&self) -> Captured {
        let start = self.bytes.len().saturating_sub(self.limit);
        Captured {
            text: plain(&String::from_utf8_lossy(&self.bytes[start..])),
            truncated: self.dropped || start > 0,
        }
    }
}

/// Text without escape sequences and carriage returns, for the agent.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // CSI (ESC [ ... letter) or a two-character escape.
                if chars.next_if_eq(&'[').is_some() {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => out.push('\n'),
            c => out.push(c),
        }
    }
    out
}

fn wait_resuming(pid: Pid) -> std::io::Result<i32> {
    loop {
        match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => return Ok(code),
            Ok(WaitStatus::Signaled(_, signal, _)) => return Ok(128 + signal as i32),
            Ok(WaitStatus::Stopped(..)) => killpg(pid, Signal::SIGCONT)?,
            Ok(_) | Err(Errno::EINTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

/// Makes `pgrp` the terminal's foreground process group. Called from the
/// background, `tcsetpgrp` raises SIGTTOU, which would stop Parolsh, unless
/// the signal is blocked.
fn give_terminal(terminal: &Stdin, pgrp: Pid) -> nix::Result<()> {
    let mut ttou = SigSet::empty();
    ttou.add(Signal::SIGTTOU);
    ttou.thread_block()?;
    let result = tcsetpgrp(terminal, pgrp);
    ttou.thread_unblock()?;
    result
}

/// Shell-style exit code: the child's code, or 128 + signal number.
pub fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdout(mut command: Command) -> String {
        let output = command.output().unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn shell_commands_load_bashrc_aliases() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".bashrc"),
            "case $- in *i*) ;; *) return;; esac\nalias hello='echo hello from bashrc'\n",
        )
        .unwrap();
        let shell = ["bash".to_string(), "-ic".to_string()];

        let mut command = command(&shell, "hello", home.path());
        command.env("HOME", home.path());

        // Only the last line: an interactive bash also runs /etc/bash.bashrc,
        // which may print its own messages (Ubuntu prints a sudo hint).
        assert_eq!(stdout(command).lines().last(), Some("hello from bashrc"));
    }

    #[test]
    fn shell_commands_run_in_the_given_directory_without_history() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        let output = stdout(command(&shell, "pwd; echo $HISTFILE", dir.path()));

        let expected = format!(
            "{}\n/dev/null\n",
            dir.path().canonicalize().unwrap().display()
        );
        assert_eq!(output, expected);
    }

    #[test]
    fn passthrough_forwards_positional_arguments() {
        let args = ["name".to_string(), "first".to_string()];

        assert_eq!(stdout(passthrough("echo $0 $1", &args)), "name first\n");
    }

    #[test]
    fn shared_commands_capture_stdout_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        let (code, captured) =
            run_shared(&shell, "echo out; echo err >&2; exit 3", dir.path(), 1024).unwrap();

        assert_eq!(code, 3);
        assert!(captured.text.contains("out\n"), "{captured:?}");
        assert!(captured.text.contains("err\n"), "{captured:?}");
        assert!(!captured.truncated);
    }

    #[test]
    fn shared_output_keeps_only_its_tail() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        let (_, captured) = run_shared(&shell, "seq 1 10000", dir.path(), 100).unwrap();

        assert!(captured.truncated);
        assert!(captured.text.len() <= 100);
        assert!(captured.text.ends_with("9999\n10000\n"), "{captured:?}");
    }

    #[test]
    fn a_background_process_does_not_block_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];
        let started = std::time::Instant::now();

        let (code, captured) =
            run_shared(&shell, "echo now; sleep 30 &", dir.path(), 1024).unwrap();

        assert_eq!(code, 0);
        assert_eq!(captured.text, "now\n");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// What the interactive shell prints while it starts is not the
    /// command's output: only the command is captured.
    #[test]
    fn the_shell_startup_output_is_not_captured() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".bashrc"),
            "echo rc-out; echo rc-err >&2\nalias hello='echo hello from alias'\n",
        )
        .unwrap();
        let shell = [
            "bash".to_string(),
            "--rcfile".to_string(),
            home.path().join(".bashrc").display().to_string(),
            "-ic".to_string(),
        ];

        let (code, captured) = run_shared(&shell, "hello; (exit 5)", home.path(), 1024).unwrap();

        assert_eq!(code, 5);
        // The alias from the rc file works, and its output is captured.
        assert_eq!(captured.text, "hello from alias\n");
    }

    #[test]
    fn captured_text_is_plain() {
        assert_eq!(
            plain("\x1b[1;31mred\x1b[0m\r\nbar 10%\rbar 100%\n"),
            "red\nbar 10%\nbar 100%\n"
        );
    }

    #[test]
    fn exit_code_follows_shell_conventions() {
        let status = |line: &str| passthrough(line, &[]).status().unwrap();

        assert_eq!(exit_code(status("exit 3")), 3);
        assert_eq!(exit_code(status("kill -TERM $$")), 128 + 15);
    }
}
