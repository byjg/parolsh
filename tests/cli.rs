//! The `parolsh` binary as scripts and tools call it.

use std::process::Command;

fn parolsh() -> Command {
    Command::new(env!("CARGO_BIN_EXE_parolsh"))
}

const JOB_SHELL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/job_shell.py");
const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_agent.py");

/// An `XDG_CONFIG_HOME` whose `parolsh/config.toml` holds `toml`. A config
/// file must exist, or the first run writes one with the agents on `PATH`.
fn config_home(toml: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("parolsh")).unwrap();
    std::fs::write(home.path().join("parolsh/config.toml"), toml).unwrap();
    home
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
    let config = config_home("");
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
    let config = config_home("");

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

/// The first run writes the configuration with the agents found on PATH,
/// starts the default one, and says so; the next run leaves the file alone.
#[test]
fn the_first_run_configures_the_agents_on_path() {
    let config = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    // A `claude-agent-acp` that is really the fake agent, first on PATH.
    let fake = bin.path().join("claude-agent-acp");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\nexec python3 {FAKE_AGENT} \"$@\"\n"),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake, permissions).unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap()
    );
    let run = || {
        let output = Command::new("python3")
            .arg(JOB_SHELL)
            .arg(env!("CARGO_BIN_EXE_parolsh"))
            .arg("env PWD")
            .arg("#exit")
            .env("TERM", "dumb")
            .env("PATH", &path)
            .env("XDG_CONFIG_HOME", config.path())
            .env("XDG_STATE_HOME", config.path())
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    };

    let screen = run();

    let file = config.path().join("parolsh/config.toml");
    assert!(
        screen.contains(&format!("Created {}", file.display())),
        "{screen}"
    );
    assert!(
        screen.contains("Using claude; switch with #agent <name>."),
        "{screen}"
    );
    // The fake agent answered: the configured agent was started.
    assert!(!screen.contains("no agent configured"), "{screen}");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("\n[agents.claude]\ncommand = \"claude-agent-acp\"\n"));
    assert!(written.contains("\ndefault_agent = \"claude\"\n"));

    let screen = run();

    assert!(!screen.contains("Created "), "{screen}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), written);
}

/// `#config` lists the configuration files; `#config sample` prints the
/// built-in sample.
#[test]
fn config_shows_the_files_and_the_sample() {
    let config = config_home("");
    let output = Command::new("python3")
        .arg(JOB_SHELL)
        .arg(env!("CARGO_BIN_EXE_parolsh"))
        .arg("#config")
        .arg("#config sample")
        .arg("#exit")
        .env("TERM", "dumb")
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", config.path())
        .output()
        .unwrap();
    let screen = String::from_utf8(output.stdout).unwrap();

    let global = config.path().join("parolsh/config.toml");
    assert!(
        screen.contains(&format!("Global:  {}\n", global.display())),
        "{screen}"
    );
    assert!(
        screen.contains("Project: none (#project init creates one here)"),
        "{screen}"
    );
    assert!(
        screen.contains("# Parolsh configuration: every option, commented out."),
        "{screen}"
    );
}

/// Runs Parolsh with the fake agent as the only agent, `extra` added to the
/// configuration, and returns the screen after typing `lines`.
fn with_fake_agent(extra: &str, lines: &[&str]) -> String {
    let config = config_home(&format!(
        "{extra}\ndefault_agent = \"fake\"\n[agents.fake]\ncommand = \"python3\"\nargs = [\"{FAKE_AGENT}\"]\n"
    ));
    let output = Command::new("python3")
        .arg(JOB_SHELL)
        .arg(env!("CARGO_BIN_EXE_parolsh"))
        .args(lines)
        .arg("#exit")
        .env("TERM", "dumb")
        .env("XDG_CONFIG_HOME", config.path())
        .env("XDG_STATE_HOME", config.path())
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap()
}

/// `#options` lists the agent's options with the current value marked, and
/// `#options <id> <value>` sets one, checked against the offered values.
#[test]
fn options_are_listed_and_set_during_the_run() {
    let screen = with_fake_agent(
        "",
        &[
            "#options",
            "#options effort low",
            "opts",
            "#options effort max",
        ],
    );

    assert!(
        screen.contains("  effort             low, high*\n"),
        "{screen}"
    );
    assert!(
        screen.contains("  fast               true, false*\n"),
        "{screen}"
    );
    assert!(
        screen.contains("effort = low, until you leave Parolsh."),
        "{screen}"
    );
    assert!(screen.contains(r#""effort": "low""#), "{screen}");
    assert!(
        screen.contains("`effort` has no value `max`. Values: low, high"),
        "{screen}"
    );
}

/// `thinking = "show"` prints the reasoning before the answer; `"hidden"`
/// keeps it out.
#[test]
fn thinking_can_be_shown_or_hidden() {
    let shown = with_fake_agent("thinking = \"show\"", &["think"]);
    let hidden = with_fake_agent("thinking = \"hidden\"", &["think"]);

    assert!(
        shown.contains("Let me think.\nAlmost there\ndone"),
        "{shown}"
    );
    assert!(!hidden.contains("Let me think."), "{hidden}");
    assert!(hidden.contains("\ndone"), "{hidden}");
}
