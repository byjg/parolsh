//! Running shell commands: `!command`, `!bash` and `parolsh -c`.

use nix::errno::Errno;
use nix::sys::signal::{SigSet, Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, getpgrp, tcsetpgrp};
use std::io::{IsTerminal, Stdin};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitStatus};

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
pub fn run_foreground(mut command: Command) -> std::io::Result<i32> {
    let terminal = std::io::stdin();
    if !terminal.is_terminal() {
        return command.status().map(exit_code);
    }

    command.process_group(0);
    let child = command.spawn()?;
    let pid = Pid::from_raw(child.id() as i32);

    // `spawn` returns after exec, so the child already leads its own group.
    // It may exit before this; then there is nothing to hand over.
    let _ = give_terminal(&terminal, pid);
    let code = wait_resuming(pid);
    give_terminal(&terminal, getpgrp())?;
    code
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
    fn exit_code_follows_shell_conventions() {
        let status = |line: &str| passthrough(line, &[]).status().unwrap();

        assert_eq!(exit_code(status("exit 3")), 3);
        assert_eq!(exit_code(status("kill -TERM $$")), 128 + 15);
    }
}
