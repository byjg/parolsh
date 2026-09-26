//! The user's shell environment. Started from a desktop launcher, a terminal
//! profile command or an IDE, Parolsh does not get what `~/.profile` and
//! `~/.bashrc` set up (nvm's `PATH`, exported API keys, ...). It cannot run
//! those files itself, so it asks the user's shell to, and imports the
//! resulting environment: `<shell> -ilc 'env -0'`.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::config::ShellEnv;

/// How long the shell may take to start before Parolsh gives up.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Variables that describe Parolsh's own process, not the user's setup.
const KEEP_OURS: [&str; 6] = ["TERM", "PWD", "OLDPWD", "SHLVL", "_", "COLUMNS"];

/// Shells that already give their children the user's environment.
const SHELLS: [&str; 8] = ["bash", "zsh", "fish", "sh", "dash", "ksh", "tcsh", "nu"];

/// Imports the shell environment if `mode` asks for it. Returns a message
/// when the import failed.
///
/// # Safety
///
/// Changes the process environment: call it before any other thread starts.
pub unsafe fn load(mode: ShellEnv, shell: &str) -> Option<String> {
    let wanted = match mode {
        ShellEnv::Never => false,
        ShellEnv::Always => true,
        ShellEnv::Auto => !started_from_shell(),
    };
    if !wanted {
        return None;
    }
    match resolve(shell, TIMEOUT) {
        Ok(vars) => {
            for (key, value) in vars {
                // SAFETY: the caller guarantees no other thread runs yet.
                unsafe { std::env::set_var(key, value) };
            }
            None
        }
        Err(e) => Some(format!(
            "could not load your shell environment ({shell}): {e}"
        )),
    }
}

/// Runs `shell -ilc 'env -0'` (a login and interactive shell reads both
/// `~/.profile` and `~/.bashrc`) and returns the variables to import.
/// Whatever the startup files print is ignored; only `env -0` is read.
///
/// The shell runs in a new session, without a controlling terminal: an
/// interactive shell would otherwise take the terminal for its job control
/// and leave Parolsh in the background, stopped.
fn resolve(shell: &str, timeout: Duration) -> std::io::Result<Vec<(String, String)>> {
    let mut command = Command::new(shell);
    command
        .args(["-ilc", "env -0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: between fork and exec only setsid runs, which is
    // async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = tx.send(stdout.read_to_end(&mut bytes).map(|_| bytes));
    });
    let bytes = match rx.recv_timeout(timeout) {
        Ok(read) => read?,
        Err(_) => {
            // A startup file waiting for input, or a very slow one.
            let _ = child.kill();
            return Err(std::io::Error::other(format!(
                "no answer after {}s",
                timeout.as_secs()
            )));
        }
    };
    let status = child.wait()?;
    let vars = parse(&bytes);
    if vars.is_empty() {
        return Err(std::io::Error::other(format!("no environment ({status})")));
    }
    Ok(vars)
}

/// `env -0` output: `KEY=value` entries separated by NUL, without the
/// variables that belong to Parolsh's own process.
fn parse(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|&b| b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            let valid = !key.is_empty() && !key.contains('\n') && !KEEP_OURS.contains(&key);
            valid.then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

/// True when the parent process is a shell, which already passed on the
/// user's environment.
fn started_from_shell() -> bool {
    parent_name().is_some_and(|name| is_shell(&name))
}

fn is_shell(name: &str) -> bool {
    // A login shell's name starts with `-`, e.g. `-bash`.
    let name = name.trim().trim_start_matches('-');
    let name = name.rsplit('/').next().unwrap_or(name);
    SHELLS.contains(&name)
}

fn parent_name() -> Option<String> {
    let parent = nix::unistd::getppid();
    std::fs::read_to_string(format!("/proc/{parent}/comm"))
        .ok()
        .or_else(|| {
            // No /proc (macOS): ask ps.
            let output = Command::new("ps")
                .args(["-o", "comm=", "-p", &parent.to_string()])
                .output()
                .ok()?;
            String::from_utf8(output.stdout).ok()
        })
}

/// The first executable named `command` in the directories of `search_path`
/// (a `PATH`-style list). A command with a `/` is checked as a path.
pub fn find_command(command: &str, search_path: Option<&str>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |path: &PathBuf| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if command.contains('/') {
        let path = PathBuf::from(command);
        return executable(&path).then_some(path);
    }
    std::env::split_paths(search_path?)
        .map(|dir| dir.join(command))
        .find(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_output_without_our_own_variables() {
        let output = b"PATH=/nvm/bin:/usr/bin\0NVM_DIR=/home/j/.nvm\0TERM=dumb\0\
                       MULTI=line1\nline2\0SHLVL=2\0=broken\0";

        assert_eq!(
            parse(output),
            [
                ("PATH".to_string(), "/nvm/bin:/usr/bin".to_string()),
                ("NVM_DIR".to_string(), "/home/j/.nvm".to_string()),
                ("MULTI".to_string(), "line1\nline2".to_string()),
            ]
        );
    }

    #[test]
    fn recognizes_shells() {
        for name in ["bash", "-bash", "zsh\n", "/bin/sh", "fish"] {
            assert!(is_shell(name), "{name}");
        }
        for name in ["gnome-shell", "python3", "code", "kitty", "sudo"] {
            assert!(!is_shell(name), "{name}");
        }
    }

    /// A login shell reads ~/.profile: what it exports is imported, and what
    /// the startup files print is not mistaken for variables.
    #[test]
    fn resolves_the_login_environment() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".profile"),
            "echo noise from profile\nexport PAROLSH_FROM_PROFILE=yes\n",
        )
        .unwrap();
        // The test runs bash with this HOME, like a user's login.
        let shell = home.path().join("bash-with-home");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nHOME={} exec bash \"$@\"\n",
                home.path().display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&shell).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&shell, permissions).unwrap();

        let vars = resolve(shell.to_str().unwrap(), TIMEOUT).unwrap();

        assert!(vars.contains(&("PAROLSH_FROM_PROFILE".to_string(), "yes".to_string())));
        assert!(!vars.iter().any(|(key, _)| key.contains("noise")));
    }

    #[test]
    fn a_shell_that_hangs_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 30\n").unwrap();
        let mut permissions = std::fs::metadata(&shell).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&shell, permissions).unwrap();
        let started = std::time::Instant::now();

        let error = resolve(shell.to_str().unwrap(), Duration::from_secs(1)).unwrap_err();

        assert!(error.to_string().contains("no answer after 1s"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn finds_commands_on_the_path_or_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
        let path = dir.path().to_str();

        assert_eq!(find_command("tool", path), Some(tool.clone()));
        assert_eq!(find_command(tool.to_str().unwrap(), None), Some(tool));
        assert_eq!(find_command("missing", path), None);
        assert_eq!(find_command("tool", None), None);
    }
}
