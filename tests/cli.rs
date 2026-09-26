//! The `parolsh` binary as scripts and tools call it.

use std::process::Command;

fn parolsh() -> Command {
    Command::new(env!("CARGO_BIN_EXE_parolsh"))
}

#[test]
fn dash_c_returns_the_command_exit_code() {
    let status = parolsh().args(["-c", "exit 7"]).status().unwrap();

    assert_eq!(status.code(), Some(7));
}

#[test]
fn dash_c_passes_positional_arguments() {
    let output = parolsh()
        .args(["-c", "echo \"$0|$1|$2\"", "name", "-x", "second"])
        .output()
        .unwrap();

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "name|-x|second\n"
    );
}

#[test]
fn dash_c_does_not_load_bashrc() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bashrc"), "echo loaded-bashrc\n").unwrap();

    let output = parolsh()
        .args(["-c", "echo ok"])
        .env("HOME", home.path())
        .output()
        .unwrap();

    assert_eq!(String::from_utf8(output.stdout).unwrap(), "ok\n");
}

#[test]
fn version_matches_the_crate() {
    let output = parolsh().arg("--version").output().unwrap();

    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("parolsh {}\n", env!("CARGO_PKG_VERSION"))
    );
}

/// Parolsh started as a job from an interactive shell must get the terminal
/// back after a `!` command, even one that takes the terminal and exits
/// without returning it. Otherwise the kernel stops Parolsh (SIGTTOU) as soon
/// as it redraws its prompt.
#[test]
fn a_bang_command_cannot_keep_the_terminal() {
    let config = tempfile::tempdir().unwrap();
    let steal_terminal = "!python3 -c 'import os, signal; \
        signal.signal(signal.SIGTTOU, signal.SIG_IGN); \
        os.setpgid(0, 0); os.tcsetpgrp(0, os.getpgrp())'";

    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/job_shell.py"
        ))
        .arg(env!("CARGO_BIN_EXE_parolsh"))
        .arg(steal_terminal)
        .arg("!echo still-in-control")
        .arg("#exit")
        .arg("jobs; echo back-in-bash")
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", config.path())
        .output()
        .unwrap();
    let screen = String::from_utf8(output.stdout).unwrap();

    assert!(screen.contains("\nstill-in-control\n"), "{screen}");
    assert!(screen.contains("\nback-in-bash\n"), "{screen}");
    assert!(!screen.contains("Stopped"), "{screen}");
}

/// `#agent <name>` stops the running agent and starts the other one, and
/// `#agent list` marks the active agent.
#[test]
fn agent_switches_to_another_configured_agent() {
    let config = tempfile::tempdir().unwrap();
    let dir = config.path().join("parolsh");
    std::fs::create_dir(&dir).unwrap();
    let fake_agent = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_agent.py");
    std::fs::write(
        dir.join("config.toml"),
        format!(
            r#"
            default_agent = "first"

            [agents.first]
            command = "python3"
            args = ["{fake_agent}"]
            env = {{ WHO = "agent-one" }}

            [agents.second]
            command = "python3"
            args = ["{fake_agent}"]
            env = {{ WHO = "agent-two" }}
            "#
        ),
    )
    .unwrap();

    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/job_shell.py"
        ))
        .arg(env!("CARGO_BIN_EXE_parolsh"))
        .arg("env WHO")
        .arg("#agent second")
        .arg("env WHO")
        .arg("#agent list")
        .arg("#agent nope")
        .arg("#exit")
        // Plain output: the ANSI status line would redraw over the answers.
        .env("TERM", "dumb")
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", config.path())
        .output()
        .unwrap();
    let screen = String::from_utf8(output.stdout).unwrap();

    let one = screen.find("\nagent-one").expect(&screen);
    let switched = screen
        .find("Agent changed to second. Started a new conversation.")
        .expect(&screen);
    let two = screen.find("\nagent-two").expect(&screen);
    assert!(one < switched && switched < two, "{screen}");
    assert!(screen.contains("\n* second "), "{screen}");
    assert!(
        screen.contains("no agent named `nope`. Configured: first, second"),
        "{screen}"
    );
}

/// A pasted text with line breaks is one input: nothing runs until Enter,
/// then every line runs once. Without bracketed paste, each line break was
/// an Enter and the rest of the paste was lost.
#[test]
fn a_multi_line_paste_is_one_input() {
    let config = tempfile::tempdir().unwrap();

    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/job_shell.py"
        ))
        .arg(env!("CARGO_BIN_EXE_parolsh"))
        .arg(r"paste:!echo pasted-one\necho pasted-two")
        .arg("")
        .arg("#exit")
        .env("TERM", "xterm-256color")
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", config.path())
        .output()
        .unwrap();
    let screen = String::from_utf8(output.stdout).unwrap();

    assert!(screen.contains("\npasted-one\n"), "{screen}");
    assert!(screen.contains("\npasted-two\n"), "{screen}");
}
