//! Running shell commands: `!command`, `!bash` and `parolsh -c`.

use nix::errno::Errno;
use nix::sys::signal::{SigSet, Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, getpgrp, tcsetpgrp};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{IsTerminal, PipeReader, Read, Stdin, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

/// Exported variables, by name.
type Vars = BTreeMap<String, String>;

/// What your shell commands changed in their environment: `export`, `unset`,
/// a `source`d file, `nvm use`, a virtualenv's `activate`. Each command runs
/// in its own shell, which starts from Parolsh's environment and your
/// startup files as ever; these changes are then applied again before the
/// line, so they go on from one command to the next. For each variable the
/// last value replaces the one before.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    set: Vars,
    unset: BTreeSet<String>,
}

impl Changes {
    /// Takes in what a line changed: the exported variables of its shell
    /// when the line started, and when the shell ended.
    fn record(&mut self, before: &Vars, after: &Vars) {
        for (name, value) in after {
            if before.get(name) != Some(value) {
                self.unset.remove(name);
                self.set.insert(name.clone(), value.clone());
            }
        }
        for name in before.keys().filter(|name| !after.contains_key(*name)) {
            self.set.remove(name);
            self.unset.insert(name.clone());
        }
    }

    /// The shell lines that apply the changes: `unset A B`, then an `export`
    /// for each variable set. `None` when they do not fit in a command line.
    fn script(&self) -> Option<String> {
        let mut script = String::new();
        if !self.unset.is_empty() {
            let names: Vec<&str> = self.unset.iter().map(String::as_str).collect();
            script.push_str(&format!("unset {}\n", names.join(" ")));
        }
        for (name, value) in &self.set {
            script.push_str(&format!(
                "export {name}='{}'\n",
                value.replace('\'', r"'\''")
            ));
        }
        (script.len() <= MAX_CHANGES).then_some(script)
    }

    /// Applies the changes to a command that takes no script: `!bash`.
    fn apply(&self, command: &mut Command) {
        for name in &self.unset {
            command.env_remove(name);
        }
        command.envs(&self.set);
    }
}

/// The longest script of changes: the system gives one argument 128 KB, and
/// the line goes in it too.
const MAX_CHANGES: usize = 100 * 1024;

/// What a shell left for the next command.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Left {
    /// The directory it ended in: `None` when it did not say (killed, `exec`,
    /// a shell without `trap`).
    pub dir: Option<PathBuf>,
    /// The changes it was given, and the ones its line made.
    pub changes: Changes,
}

/// Variables that are each shell's own, not changes to carry: its depth and
/// directory, the history file Parolsh sets for `!` commands, the last
/// command's name.
const OURS: [&str; 4] = ["SHLVL", "PWD", "HISTFILE", "_"];

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

/// The descriptor on which the shell reports its environment and the
/// directory it ended in.
const REPORT_FD: RawFd = 5;

/// `line` in a script that first applies `changes` (what earlier commands
/// exported or unset), and reports what the next command needs, whatever the
/// line does (`cd dir && exit 3` still reports): the exported variables
/// before the line, then on exit the directory and the variables again, so
/// that Parolsh sees what the line changed. The report is `env -0`, an empty
/// entry, the directory, a NUL, and `env -0`. The line runs with the report's
/// descriptor closed, so nothing it starts keeps it. `redirects` go on the
/// line too.
fn tracked(line: &str, redirects: &str, changes: &Changes) -> String {
    let fd = REPORT_FD;
    let changes = changes.script().unwrap_or_else(|| {
        eprintln!(
            "parolsh: the variables your commands changed take more than {} KB: \
             they are not applied to this command",
            MAX_CHANGES / 1024
        );
        String::new()
    });
    format!(
        "trap 'command pwd -P >&{fd}; printf \"\\0\" >&{fd}; command env -0 >&{fd}' EXIT\n\
         {changes}\
         command env -0 >&{fd}; printf \"\\0\" >&{fd}\n\
         {{ {line}\n}} {redirects}{fd}>&-"
    )
}

/// `!command`: runs `line` as the terminal's foreground job, after applying
/// `changes`. Returns its exit code and what the shell left for the next
/// one.
pub fn run(
    shell: &[String],
    line: &str,
    cwd: &Path,
    changes: &Changes,
) -> std::io::Result<(i32, Left)> {
    let (report_read, report_write) = std::io::pipe()?;
    let mut command = self::command(shell, &tracked(line, "", changes), cwd);
    pass_fds(&mut command, vec![(report_write.as_raw_fd(), REPORT_FD)]);
    let report = Report::read(report_read);
    let code = run_job(command, move |_| drop(report_write))?;
    Ok((code, report.left(changes)))
}

/// Gives the child `fds`, each `(ours, its number)`. Ours are first copied
/// above every target, so that moving one in place never overwrites
/// another; dup2 then clears close-on-exec on the targets.
fn pass_fds(command: &mut Command, fds: Vec<(RawFd, RawFd)>) {
    const MAX: usize = 3;
    assert!(fds.len() <= MAX);
    // SAFETY: between fork and exec only fcntl and dup2 run, which are
    // async-signal-safe; nothing allocates.
    unsafe {
        command.pre_exec(move || {
            let mut high = [0; MAX];
            for (i, (from, _)) in fds.iter().enumerate() {
                high[i] = libc::fcntl(*from, libc::F_DUPFD_CLOEXEC, 10);
                if high[i] < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            for (i, (_, to)) in fds.iter().enumerate() {
                if libc::dup2(high[i], *to) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// What the shell reports (`tracked`), read while it runs: an environment
/// can be larger than the pipe holds, and the shell would wait for room, for
/// ever. A process started by `~/.bashrc` may keep the pipe open, so the end
/// is when the shell is gone and the pipe is empty, not when the pipe
/// closes.
struct Report {
    shell_gone: Arc<AtomicBool>,
    reader: Option<JoinHandle<Vec<u8>>>,
}

impl Report {
    fn read(mut pipe: PipeReader) -> Self {
        let shell_gone = Arc::new(AtomicBool::new(false));
        // SAFETY: fcntl on a descriptor we own.
        if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            return Self {
                shell_gone,
                reader: None,
            };
        }
        let gone = shell_gone.clone();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if gone.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            bytes
        });
        Self {
            shell_gone,
            reader: Some(reader),
        }
    }

    /// What the shell left, given the `changes` it started with. Call it
    /// once the shell is gone.
    fn left(self, changes: &Changes) -> Left {
        self.shell_gone.store(true, Ordering::SeqCst);
        let bytes = self.reader.and_then(|reader| reader.join().ok());
        let (dir, vars) = bytes.map_or((None, None), |bytes| parse_report(&bytes));
        let mut changes = changes.clone();
        if let Some((before, after)) = vars {
            changes.record(&before, &after);
        }
        Left { dir, changes }
    }
}

/// The directory the shell ended in, and its exported variables before the
/// line and at the end. The report is NUL-separated: the variables before,
/// an empty entry, the directory, the variables at the end. Each part is
/// `None` when the shell did not get to write it.
fn parse_report(bytes: &[u8]) -> (Option<PathBuf>, Option<(Vars, Vars)>) {
    let fields: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let Some(split) = fields.iter().position(|field| field.is_empty()) else {
        return (None, None);
    };
    // The shell was killed, or replaced with `exec`: no end.
    let Some(dir) = fields.get(split + 1).filter(|dir| !dir.is_empty()) else {
        return (None, None);
    };
    let dir = dir.strip_suffix(b"\n").and_then(|dir| {
        let dir = PathBuf::from(OsString::from_vec(dir.to_vec()));
        (dir.is_absolute() && dir.is_dir()).then_some(dir)
    });
    let before = vars(&fields[..split]);
    let after = vars(&fields[split + 2..]);
    // A shell without `env -0` says nothing: that is not "all unset".
    let both = (!before.is_empty() && !after.is_empty()).then_some((before, after));
    (dir, both)
}

/// `KEY=value` entries, without each shell's own variables and the names a
/// shell cannot `export` (bash's exported functions).
fn vars(entries: &[&[u8]]) -> Vars {
    entries
        .iter()
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (name, value) = entry.split_once('=')?;
            let identifier = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit());
            (!name.is_empty() && identifier && !OURS.contains(&name))
                .then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// `!bash`: a real interactive Bash session, with `changes` applied before
/// its startup files. What it changes itself stays in it.
pub fn bash(cwd: &Path, changes: &Changes) -> Command {
    let mut command = Command::new("bash");
    command.current_dir(cwd);
    changes.apply(&mut command);
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
/// the terminal as usual. Applies `changes` and returns what the shell left,
/// as [`run`] does.
pub fn run_shared(
    shell: &[String],
    line: &str,
    cwd: &Path,
    changes: &Changes,
    limit: usize,
) -> std::io::Result<(i32, Captured, Left)> {
    let (out_read, out_write) = std::io::pipe()?;
    let (err_read, err_write) = std::io::pipe()?;
    let (dir_read, dir_write) = std::io::pipe()?;
    let script = tracked(line, ">&3 2>&4 3>&- 4>&- ", changes);
    let mut command = self::command(shell, &script, cwd);
    pass_fds(
        &mut command,
        vec![
            (out_write.as_raw_fd(), 3),
            (err_write.as_raw_fd(), 4),
            (dir_write.as_raw_fd(), REPORT_FD),
        ],
    );

    let report = Report::read(dir_read);
    let tail = Arc::new(Mutex::new(Tail::new(limit)));
    let (done_tx, done_rx) = mpsc::channel();
    copy(out_read, std::io::stdout(), tail.clone(), done_tx.clone());
    copy(err_read, std::io::stderr(), tail.clone(), done_tx);
    let code = run_job(command, move |_| {
        // Our write ends, so the reads end when the command's do.
        drop((out_write, err_write, dir_write));
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
    Ok((code, tail, report.left(changes)))
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
    fn a_command_reports_the_directory_it_ended_in() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        // `exit` still reports, and keeps its code.
        let (code, left) =
            run(&shell, "cd sub && exit 3", dir.path(), &Changes::default()).unwrap();
        assert_eq!((code, left.dir), (3, Some(sub.canonicalize().unwrap())));
        let (code, left) = run(&shell, "true", dir.path(), &Changes::default()).unwrap();
        assert_eq!(
            (code, left.dir),
            (0, Some(dir.path().canonicalize().unwrap()))
        );
        // A shell replaced with `exec` cannot say.
        assert_eq!(
            run(&shell, "exec true", dir.path(), &Changes::default()).unwrap(),
            (0, Left::default())
        );
    }

    /// A process started before the command, as by `~/.bashrc`, keeps the
    /// directory pipe open: the directory is still read, without waiting.
    #[test]
    fn a_process_started_by_the_shell_does_not_block_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        std::fs::write(&rc, "sleep 30 &\n").unwrap();
        let shell = [
            "bash".to_string(),
            "--rcfile".to_string(),
            rc.display().to_string(),
            "-ic".to_string(),
        ];
        let started = std::time::Instant::now();

        let (code, left) = run(&shell, "cd /", dir.path(), &Changes::default()).unwrap();

        assert_eq!((code, left.dir), (0, Some(PathBuf::from("/"))));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// What `line` prints, after the lines of `before`, each in its own
    /// shell with the changes of the ones before it.
    fn after_with(shell: &[String], before: &[&str], line: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let mut changes = Changes::default();
        for line in before {
            changes = run(shell, line, dir.path(), &changes).unwrap().1.changes;
        }
        let (_, captured, _) = run_shared(shell, line, dir.path(), &changes, 4096).unwrap();
        captured.text
    }

    fn after(before: &[&str], line: &str) -> String {
        after_with(&["bash".to_string(), "-c".to_string()], before, line)
    }

    #[test]
    fn exported_variables_go_on_in_the_next_command() {
        assert_eq!(
            after(&["export FOO=\"it's a b\""], "echo \"$FOO\""),
            "it's a b\n"
        );
        // `source`, as `nvm use` or a virtualenv's `activate` do it.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env.sh");
        std::fs::write(
            &file,
            "export PATH=\"/opt/tool/bin:$PATH\"\nexport TOOL=1\n",
        )
        .unwrap();
        let source = format!("source {}", file.display());
        assert_eq!(
            after(&[&source, "true"], "echo ${PATH%%:*} $TOOL"),
            "/opt/tool/bin 1\n"
        );
    }

    #[test]
    fn the_last_value_replaces_the_one_before_and_unset_goes_on() {
        assert_eq!(
            after(
                &["export FOO=1 BAR=1", "unset FOO; export BAR=2"],
                "echo [$FOO] [$BAR]"
            ),
            "[] [2]\n"
        );
        // A variable of Parolsh's own environment can be unset too.
        assert_eq!(after(&["unset HOME"], "echo [$HOME]"), "[]\n");
        assert_eq!(
            after(&["unset HOME", "export HOME=/x"], "echo [$HOME]"),
            "[/x]\n"
        );
    }

    #[test]
    fn only_what_a_line_changed_is_kept() {
        let shell = ["bash".to_string(), "-c".to_string()];
        let dir = tempfile::tempdir().unwrap();
        let none = Changes::default();

        // Not exported, or nothing changed: nothing to carry.
        let (_, left) = run(&shell, "PLAIN=1; alias hi='echo hi'; ls", dir.path(), &none).unwrap();
        assert_eq!(left.changes, none);
        // A `cd` only changes the previous directory: `cd -` works next.
        let (_, left) = run(&shell, "cd /tmp && cd /", dir.path(), &none).unwrap();
        assert_eq!(left.changes.set.keys().collect::<Vec<_>>(), ["OLDPWD"]);
        let (_, captured, left) = run_shared(
            &shell,
            "cd - >/dev/null; pwd",
            Path::new("/"),
            &left.changes,
            1024,
        )
        .unwrap();
        assert_eq!(captured.text, "/tmp\n");
        assert_eq!(left.dir, Some(PathBuf::from("/tmp")));
    }

    /// Startup files run for every command. One that adds to `PATH` must
    /// not add again to what it added for the command before, and what you
    /// put in front stays in front of what it adds.
    #[test]
    fn a_startup_file_that_adds_to_the_path_does_not_make_it_grow() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        std::fs::write(&rc, "export PATH=\"/from/bashrc:$PATH\"\n").unwrap();
        let shell = [
            "bash".to_string(),
            "--rcfile".to_string(),
            rc.display().to_string(),
            "-ic".to_string(),
        ];
        let count = "echo $PATH | tr : '\\n' | grep -c /from/bashrc";

        assert_eq!(after_with(&shell, &["true", "true", "true"], count), "1\n");
        let activate = "export PATH=\"/venv/bin:$PATH\"";
        let path = after_with(&shell, &[activate, "true", "export OTHER=1"], "echo $PATH");
        assert!(path.starts_with("/venv/bin:/from/bashrc:"), "{path}");
        assert_eq!(path.matches("/venv/bin").count(), 1, "{path}");
        assert_eq!(path.matches("/from/bashrc").count(), 1, "{path}");
    }

    /// `!bash` gets the changes, and keeps its own history file.
    #[test]
    fn an_interactive_bash_starts_with_the_changes() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];
        let none = Changes::default();
        let (_, left) = run(&shell, "export FOO=1; unset HOME", dir.path(), &none).unwrap();

        assert!(!left.changes.set.contains_key("HISTFILE"));
        let mut bash = bash(dir.path(), &left.changes);
        bash.args(["--norc", "-c", "echo $FOO[$HOME][$HISTFILE]"]);
        let history = std::env::var("HISTFILE").unwrap_or_default();
        assert_eq!(stdout(bash), format!("1[][{history}]\n"));
    }

    /// An environment larger than a pipe holds (64 KB) is read while the
    /// shell writes it: the shell does not wait for room.
    #[test]
    fn a_large_environment_does_not_block_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        std::fs::write(
            &rc,
            "big=$(head -c 100000 /dev/zero | tr '\\0' x); export A=$big B=$big C=$big\n",
        )
        .unwrap();
        let shell = [
            "bash".to_string(),
            "--rcfile".to_string(),
            rc.display().to_string(),
            "-ic".to_string(),
        ];
        let started = std::time::Instant::now();

        let sizes = after_with(&shell, &["export FOO=1"], "echo ${#A} ${#C} $FOO");

        assert_eq!(sizes, "100000 100000 1\n");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    /// Changes too large for a command line are not applied; the command
    /// still runs.
    #[test]
    fn changes_too_large_for_a_command_line_are_left_out() {
        let big = "big=$(head -c 60000 /dev/zero | tr '\\0' x); export A=$big B=$big";

        assert_eq!(after(&[big], "echo ${#A} ok"), "0 ok\n");
    }

    #[test]
    fn the_report_is_the_variables_the_directory_and_the_variables_again() {
        let report = b"FOO=1\0GONE=x\0SHLVL=2\0\0/tmp\n\0FOO=a=b\0NEW=\0BASH_FUNC_f%%=() { :; }\0";
        let (dir, vars) = parse_report(report);
        let (before, after) = vars.unwrap();

        assert_eq!(dir, Some(PathBuf::from("/tmp")));
        assert_eq!(before.keys().collect::<Vec<_>>(), ["FOO", "GONE"]);
        assert_eq!(after.get("FOO").map(String::as_str), Some("a=b"));
        assert_eq!(after.keys().collect::<Vec<_>>(), ["FOO", "NEW"]);
        let mut changes = Changes::default();
        changes.record(&before, &after);
        assert_eq!(
            changes.script().unwrap(),
            "unset GONE\nexport FOO='a=b'\nexport NEW=''\n"
        );
        // Killed before the end, or without `env -0`: nothing to record.
        assert_eq!(parse_report(b"FOO=1\0\0"), (None, None));
        assert_eq!(
            parse_report(b"\0/tmp\n\0"),
            (Some(PathBuf::from("/tmp")), None)
        );
    }

    #[test]
    fn shared_commands_report_the_directory_without_sharing_it() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        let (code, captured, left) = run_shared(
            &shell,
            "cd / && echo moved",
            dir.path(),
            &Changes::default(),
            1024,
        )
        .unwrap();

        assert_eq!(code, 0);
        assert_eq!(captured.text, "moved\n");
        assert_eq!(left.dir, Some(PathBuf::from("/")));
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

        let (code, captured, _) = run_shared(
            &shell,
            "echo out; echo err >&2; exit 3",
            dir.path(),
            &Changes::default(),
            1024,
        )
        .unwrap();

        assert_eq!(code, 3);
        assert!(captured.text.contains("out\n"), "{captured:?}");
        assert!(captured.text.contains("err\n"), "{captured:?}");
        assert!(!captured.truncated);
    }

    #[test]
    fn shared_output_keeps_only_its_tail() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];

        let (_, captured, _) =
            run_shared(&shell, "seq 1 10000", dir.path(), &Changes::default(), 100).unwrap();

        assert!(captured.truncated);
        assert!(captured.text.len() <= 100);
        assert!(captured.text.ends_with("9999\n10000\n"), "{captured:?}");
    }

    #[test]
    fn a_background_process_does_not_block_the_capture() {
        let dir = tempfile::tempdir().unwrap();
        let shell = ["bash".to_string(), "-c".to_string()];
        let started = std::time::Instant::now();

        let (code, captured, _) = run_shared(
            &shell,
            "echo now; sleep 30 &",
            dir.path(),
            &Changes::default(),
            1024,
        )
        .unwrap();

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

        let (code, captured, _) = run_shared(
            &shell,
            "hello; (exit 5)",
            home.path(),
            &Changes::default(),
            1024,
        )
        .unwrap();

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
