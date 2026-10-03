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

    let version = String::from_utf8(output.stdout).unwrap();
    let rest = version
        .strip_prefix(&format!("parolsh {}", env!("CARGO_PKG_VERSION")))
        .unwrap_or_else(|| panic!("{version}"));

    // The release's tag, or work in progress: `-dev (<commit>[, dirty])`.
    let dev = rest
        .strip_prefix("-dev (")
        .and_then(|rest| rest.strip_suffix(")\n"))
        .map(|inside| inside.strip_suffix(", dirty").unwrap_or(inside));
    assert!(
        rest == "\n" || dev.is_some_and(|commit| commit.chars().all(|c| c.is_ascii_hexdigit())),
        "{version}"
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
    with_fake_agent_on(extra, lines, "dumb")
}

fn with_fake_agent_on(extra: &str, lines: &[&str], term: &str) -> String {
    with_fake_agent_flags(extra, "", lines, term)
}

/// Like `with_fake_agent_on`, with `flags` on Parolsh's command line.
fn with_fake_agent_flags(extra: &str, flags: &str, lines: &[&str], term: &str) -> String {
    let config = format!(
        "{extra}\ndefault_agent = \"fake\"\n[agents.fake]\ncommand = \"python3\"\nargs = [\"{FAKE_AGENT}\"]\n"
    );
    screen(&config, flags, lines, term)
}

/// Runs Parolsh with `config` and `flags`, and returns the screen after
/// typing `lines`.
fn screen(config: &str, flags: &str, lines: &[&str], term: &str) -> String {
    terminal_output(config, flags, lines, term, false)
}

/// What Parolsh wrote to the terminal, escape sequences included.
fn raw_output(lines: &[&str]) -> String {
    let config = format!(
        "default_agent = \"fake\"\n[agents.fake]\ncommand = \"python3\"\nargs = [\"{FAKE_AGENT}\"]\n"
    );
    terminal_output(&config, "", lines, "xterm-256color", true)
}

fn terminal_output(config: &str, flags: &str, lines: &[&str], term: &str, raw: bool) -> String {
    let home = config_home(config);
    run_in(home.path(), flags, lines, term, raw)
}

/// Runs Parolsh with the configuration and state in `home`, which stays:
/// a second run sees what the first saved.
fn run_in(home: &std::path::Path, flags: &str, lines: &[&str], term: &str, raw: bool) -> String {
    let mut command = Command::new("python3");
    command
        .arg(JOB_SHELL)
        .arg(format!("{} {flags}", env!("CARGO_BIN_EXE_parolsh")))
        .args(lines)
        .arg("#exit")
        .env("TERM", term)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_STATE_HOME", home);
    if raw {
        command.env("JOB_SHELL_RAW", "1");
    }
    String::from_utf8(command.output().unwrap().stdout).unwrap()
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

/// On an ANSI terminal the answer's markdown is rendered while it streams:
/// markers are hidden even when a chunk cuts them in half. `markdown = false`
/// keeps the raw text.
#[test]
fn markdown_is_rendered_on_ansi_terminals() {
    let rendered = with_fake_agent_on("", &["md"], "xterm-256color");
    let raw = with_fake_agent_on("markdown = false", &["md"], "xterm-256color");

    // The status line is redrawn in place; this log keeps its text in front
    // of each line, so only the ends of the lines are compared.
    assert!(rendered.contains("• bold and code\n"), "{rendered}");
    assert!(rendered.contains("Title\n"), "{rendered}");
    assert!(
        !rendered.contains("**") && !rendered.contains("## "),
        "{rendered}"
    );
    assert!(raw.contains("- **bold** and `code`\n"), "{raw}");
    assert!(raw.contains("## Title\n"), "{raw}");
}

/// Plain text goes where `input`, or `--input` over it, says. `echo $((6*7))`
/// prints 42 only when the shell runs it.
#[test]
fn input_sets_where_plain_text_goes_at_start() {
    let line = "echo $((6*7))";
    let ran = |screen: String| screen.contains("\n42\n");

    assert!(!ran(with_fake_agent("", &[line])));
    assert!(ran(with_fake_agent("input = \"shell\"", &[line])));
    assert!(ran(with_fake_agent_flags(
        "",
        "--input=shell",
        &[line],
        "dumb"
    )));
    assert!(!ran(with_fake_agent_flags(
        "input = \"shell\"",
        "--input=agent",
        &[line],
        "dumb"
    )));
}

/// A `cd` in a shell command moves where the next ones run, not the
/// agent; `#cd .` brings the agent there.
#[test]
fn a_cd_moves_the_shell_and_hash_cd_brings_the_agent() {
    let screen = with_fake_agent("", &["!cd / && exit 3", "!pwd", "where", "#cd .", "here"]);

    let start = env!("CARGO_MANIFEST_DIR");
    assert!(screen.contains("exit 3\n"), "{screen}");
    assert!(screen.contains("\n/\n"), "{screen}");
    assert!(screen.contains(&format!("|{start}] where")), "{screen}");
    assert!(screen.contains("|/] here"), "{screen}");
}

/// Without an agent configured, plain text can only go to the shell.
#[test]
fn without_an_agent_plain_text_goes_to_the_shell() {
    let screen = screen("", "", &["echo $((6*7))"], "dumb");

    assert!(screen.contains("\n42\n"), "{screen}");
}

/// `!+command` runs the command, shows its output, and sends it with the
/// next message only.
#[test]
fn bang_plus_shares_the_output_with_the_next_message() {
    let screen = with_fake_agent(
        "",
        &["!+echo shared-marker; (exit 4)", "blocks", "blocks", "!+"],
    );

    assert!(screen.contains("\nshared-marker\n"), "{screen}");
    assert!(
        screen.contains("(output of `echo shared-marker; (exit 4)` goes with your next message)"),
        "{screen}"
    );
    // The first message carries the output, with the command and exit code.
    assert!(
        screen.contains("The user ran the shell command `echo shared-marker; (exit 4)`"),
        "{screen}"
    );
    assert!(screen.contains("(exit code 4)"), "{screen}");
    // Only the command's output, not what the shell printed while starting.
    assert!(
        screen.contains("Its output:\\n```\\nshared-marker\\n```"),
        "{screen}"
    );
    // The next one does not.
    assert!(screen.contains("\n[]\n"), "{screen}");
    assert!(screen.contains("usage: !+<command>"), "{screen}");
}

/// An agent reachable only through the PATH that ~/.profile sets up, as
/// with npm agents under nvm. `always`, and `auto` when not started from a
/// shell, import that PATH and the agent starts; `never`, and `auto` from a
/// shell, give a clear error instead of an internal one.
#[test]
fn the_shell_environment_makes_profile_agents_reachable() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("nvm-bin");
    std::fs::create_dir(&bin).unwrap();
    let agent = bin.join("fake-acp");
    std::fs::write(
        &agent,
        format!("#!/bin/sh\nexec python3 {FAKE_AGENT} \"$@\"\n"),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&agent).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&agent, permissions).unwrap();
    std::fs::write(
        home.path().join(".profile"),
        format!("export PATH=\"{}:$PATH\"\n", bin.display()),
    )
    .unwrap();
    // `launcher`: started directly, not from a shell.
    let run = |shell_env: &str, launcher: bool| {
        let config = config_home(&format!(
            "shell_env = \"{shell_env}\"\ndefault_agent = \"fake\"\n[agents.fake]\ncommand = \"fake-acp\"\n"
        ));
        let parolsh = env!("CARGO_BIN_EXE_parolsh");
        let start = if launcher {
            format!("exec:{parolsh}")
        } else {
            parolsh.to_string()
        };
        let output = Command::new("python3")
            .arg(JOB_SHELL)
            .arg(start)
            .arg("env PAROLSH_MARK")
            .arg("#exit")
            .env("TERM", "dumb")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", config.path())
            .env("XDG_STATE_HOME", config.path())
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    };

    let imported = run("always", false);
    let not_imported = run("never", false);
    let auto_from_launcher = run("auto", true);
    let auto_from_shell = run("auto", false);

    // The agent started and answered (the variable is not set: "<unset>").
    assert!(imported.contains("\n<unset>\n"), "{imported}");
    assert!(
        auto_from_launcher.contains("\n<unset>\n"),
        "{auto_from_launcher}"
    );
    // Started from a shell, `auto` trusts the environment it was given.
    assert!(
        auto_from_shell.contains("cannot start `fake-acp`: not found on PATH."),
        "{auto_from_shell}"
    );
    assert!(
        not_imported.contains("cannot start `fake-acp`: not found on PATH."),
        "{not_imported}"
    );
    assert!(!not_imported.contains("Internal error"), "{not_imported}");
}

/// An agent that ignores the cancel: after 5 s Parolsh stops waiting and the
/// prompt is back. The next message waits for that turn, then runs.
#[test]
fn a_cancel_the_agent_ignores_ends_the_turn_after_a_while() {
    let screen = with_fake_agent("", &["stuck", "key:\\x03", "wait:5", "hello"]);

    assert!(
        screen.contains("(cancelled — the agent did not confirm)"),
        "{screen}"
    );
    // The fake agent answers "[<session>|<mode>|<cwd>] <text>".
    assert!(screen.contains("] hello\n"), "{screen}");
}

/// A second Ctrl+C stops waiting at once.
#[test]
fn a_second_ctrl_c_stops_waiting_for_the_cancel() {
    let screen = with_fake_agent("", &["stuck", "key:\\x03", "key:\\x03"]);

    assert!(
        screen.contains("(cancelled — the agent did not confirm)"),
        "{screen}"
    );
}

/// Ctrl+C is seen even when the agent never pauses between events.
#[test]
fn ctrl_c_cancels_an_agent_that_streams_nonstop() {
    let screen = with_fake_agent("", &["stream", "key:\\x03"]);

    assert!(screen.contains("(cancelled)"), "{screen}");
    assert!(!screen.contains("did not confirm"), "{screen}");
}

/// `#audit` lists what ran, what was shared and sent, what the agent did and
/// what was answered, in order.
#[test]
fn audit_shows_what_happened_in_this_run() {
    let screen = with_fake_agent(
        "",
        &[
            "!echo local",
            "!+echo shared",
            "perm",
            "1",
            "tools",
            "#audit",
        ],
    );
    let audit = &screen[screen.rfind("#audit").unwrap()..];
    let expected = [
        "USER     !echo local · exit 0",
        "USER     !+echo shared · exit 0 · 7 B kept",
        "SHARED   1 output(s), 7 B → fake",
        "USER     → fake: perm",
        "AGENT    asks: Writing to notes.txt",
        "USER     → Allow once",
        "PAROLSH  turn ended · ",
        "USER     → fake: tools",
        "AGENT    read: Read a.rs [/tmp/a] · completed",
        "AGENT    Read b · failed",
    ];

    let mut rest = audit;
    for line in expected {
        let at = rest
            .find(line)
            .unwrap_or_else(|| panic!("missing or out of order: {line}\n{audit}"));
        rest = &rest[at + line.len()..];
    }
}

/// What the agent does after its turn ended (a background task woke it up)
/// is printed above the prompt, and its tool calls are in `#audit`.
#[test]
fn the_agent_is_heard_between_turns() {
    let screen = with_fake_agent("", &["later", "wait:2", "#audit"]);

    let between = &screen[screen.find("(the agent, between turns)").expect(&screen)..];
    for line in ["• Read log", "background done", "all good"] {
        assert!(between.contains(line), "missing {line}\n{screen}");
    }
    let audit = &screen[screen.rfind("#audit").unwrap()..];
    assert!(audit.contains("AGENT    Read log"), "{audit}");
}

/// A permission request between turns takes the prompt, is answered, and
/// the prompt comes back.
#[test]
fn the_agent_can_ask_between_turns() {
    let screen = with_fake_agent("", &["later perm", "wait:1", "1", "wait:1", "#audit"]);

    assert!(
        screen.contains("Permission requested: Writing to notes.txt"),
        "{screen}"
    );
    assert!(screen.contains("chose:allow-once"), "{screen}");
    let audit = &screen[screen.rfind("#audit").unwrap()..];
    assert!(audit.contains("USER     → Allow once"), "{audit}");
}

/// The terminal's title names the conversation (the directory before the
/// agent titles it) and, while background tasks run, spins and counts them.
/// The prompt shows them on the right with their time. The previous title is
/// put back on exit.
#[test]
fn background_tasks_show_in_the_title_and_the_prompt() {
    let output = raw_output(&["bg", "wait:4"]);
    let titles: Vec<&str> = output
        .split("\x1b]2;")
        .skip(1)
        .filter_map(|rest| rest.split_once('\x07').map(|(title, _)| title))
        .collect();

    let saved = output.find("\x1b[22;0t").expect("title not saved");
    assert!(saved < output.find("\x1b]2;").unwrap(), "{output:?}");
    // The directory comes first; the end of a path is the same everywhere.
    assert!(
        titles[0].starts_with("parolsh · ") && titles[0].contains('/'),
        "{titles:?}"
    );
    let running = titles
        .iter()
        .position(|title| title.ends_with(" 1 bg · parolsh · Fake background work"))
        .expect("no title with the running task");
    // Then the task ends: no spinner, no count.
    assert_eq!(
        titles.last(),
        Some(&"parolsh · Fake background work"),
        "{titles:?}"
    );
    assert!(running < titles.len() - 1);
    assert!(output.contains("⧗ 1 bg · 0:0"), "{output:?}");
    let restored = output.rfind("\x1b[23;0t").expect("title not restored");
    assert!(restored > output.rfind("\x1b]2;").unwrap());
}

/// `@path` words that name a file go with the message as resource links
/// (absolute `file://` URIs), before the text; other `@words` stay text.
#[test]
fn mentioned_files_go_with_the_message_as_links() {
    let screen = with_fake_agent("", &["blocks @Cargo.toml and @nothing", "#audit"]);
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

    assert!(
        screen.contains(&format!(r#"["Cargo.toml file://{manifest}"]"#)),
        "{screen}"
    );
    let audit = &screen[screen.rfind("#audit").unwrap()..];
    assert!(
        audit.contains("→ fake: blocks @Cargo.toml and @nothing · 1 file linked"),
        "{audit}"
    );
}

fn fake_agent_home(extra: &str) -> tempfile::TempDir {
    config_home(&format!(
        "{extra}\ndefault_agent = \"fake\"\n[agents.fake]\ncommand = \"python3\"\nargs = [\"{FAKE_AGENT}\"]\n"
    ))
}

/// The lines `#audit 1` printed, without the times.
fn audit_of(screen: &str) -> Vec<String> {
    let from = screen.rfind("#audit 1").expect("no #audit 1");
    screen[from..]
        .lines()
        .skip(1)
        .take_while(|line| line.len() > 9 && line.as_bytes()[7] == b' ')
        .map(|line| line[9..].to_string())
        .collect()
}

/// What a run did is still there in the next one: `#sessions` lists it and
/// `#audit 1` shows it, with the agent's answers. `!command` lines are not
/// saved by default; `#forget` removes the session.
#[test]
fn the_history_keeps_sessions_across_runs() {
    let home = fake_agent_home("");
    run_in(
        home.path(),
        "",
        &["!echo local", "!+echo shared", "perm", "1", "tools"],
        "dumb",
        false,
    );

    let screen = run_in(home.path(), "", &["#sessions", "#audit 1"], "dumb", false);
    let sessions = &screen[screen.find("#sessions").unwrap()..];
    // Number, date, agent, entries, and the first message as its title.
    assert!(
        sessions
            .lines()
            .any(|line| line.trim_start().starts_with("1  ")
                && line.contains(" fake ")
                && line.ends_with("  perm")),
        "{sessions}"
    );
    let audit = audit_of(&screen);
    let expected = [
        "USER     !+echo shared · exit 0 · 7 B kept",
        "SHARED   1 output(s), 7 B → fake",
        "USER     → fake: perm",
        "AGENT    asks: Writing to notes.txt",
        "USER     → Allow once",
        "AGENT    answer: chose:allow-once",
        "USER     → fake: tools",
        "AGENT    read: Read a.rs [/tmp/a] · completed",
        "AGENT    Read b · failed",
        "AGENT    answer: done",
    ];
    for line in expected {
        assert!(
            audit.iter().any(|saved| saved == line),
            "missing {line}: {audit:#?}"
        );
    }
    assert!(
        !audit.iter().any(|line| line.contains("echo local")),
        "{audit:#?}"
    );

    let screen = run_in(home.path(), "", &["#forget 1", "#audit 1"], "dumb", false);
    assert!(
        screen.contains("Session 1 removed from the history."),
        "{screen}"
    );
    assert!(screen.contains("no session 1 in this project"), "{screen}");
}

/// With `save_commands = true`, `!command` lines are saved too, and the
/// banner says so.
#[test]
fn save_commands_saves_them_and_says_so() {
    let home = fake_agent_home("save_commands = true");
    let first = run_in(home.path(), "", &["!echo local"], "dumb", false);
    assert!(
        first.contains("!commands are saved to the history · save_commands = false to stop"),
        "{first}"
    );

    let screen = run_in(home.path(), "", &["#audit 1"], "dumb", false);
    assert_eq!(audit_of(&screen), ["USER     !echo local · exit 0"]);
}

/// `#new private` saves nothing of that conversation; the next `#new` saves
/// again.
#[test]
fn a_private_conversation_is_not_saved() {
    let home = fake_agent_home("");
    let screen = run_in(
        home.path(),
        "",
        &["#new private", "secret plan", "#new", "public plan"],
        "dumb",
        false,
    );
    assert!(
        screen.contains("Started a private conversation: it is not saved in the history."),
        "{screen}"
    );

    let screen = run_in(home.path(), "", &["#sessions"], "dumb", false);
    let sessions = &screen[screen.rfind("#sessions").unwrap()..];
    assert!(sessions.contains("public plan"), "{sessions}");
    assert!(!sessions.contains("secret"), "{sessions}");
}

/// A home whose fake agent gets `args` after the script.
fn fake_agent_home_with(args: &[&str]) -> tempfile::TempDir {
    let args: Vec<String> = std::iter::once(FAKE_AGENT)
        .chain(args.iter().copied())
        .map(|arg| format!("\"{arg}\""))
        .collect();
    config_home(&format!(
        "default_agent = \"fake\"\n[agents.fake]\ncommand = \"python3\"\nargs = [{}]\n",
        args.join(", ")
    ))
}

/// Rewrites the agent's arguments in `home`, keeping the history.
fn set_agent_args(home: &std::path::Path, args: &[&str]) {
    let fresh = fake_agent_home_with(args);
    std::fs::copy(
        fresh.path().join("parolsh/config.toml"),
        home.join("parolsh/config.toml"),
    )
    .unwrap();
}

/// `#resume <n>` goes back to the agent's conversation of session `n` (the
/// fake agent answers with its session id), and what follows is saved in
/// that session.
#[test]
fn resume_goes_back_to_the_agents_conversation() {
    let home = fake_agent_home_with(&[]);
    let first = run_in(home.path(), "", &["hello"], "dumb", false);
    assert!(first.contains("[s1|default|"), "{first}");

    // The next run's own conversation is s10: only a resume brings s1 back.
    set_agent_args(home.path(), &["--sessions-from", "10"]);
    let screen = run_in(home.path(), "", &["#resume 1", "who"], "dumb", false);
    assert!(
        screen.contains("Resumed session 1: the agent remembers that conversation."),
        "{screen}"
    );
    assert!(screen.contains("[s1|default|"), "{screen}");
    assert!(!screen.contains("[s10|"), "{screen}");

    let screen = run_in(home.path(), "", &["#audit 1"], "dumb", false);
    let audit = audit_of(&screen);
    for line in ["USER     → fake: hello", "USER     → fake: who"] {
        assert!(
            audit.iter().any(|saved| saved == line),
            "missing {line}: {audit:#?}"
        );
    }
}

/// An agent that only loads a conversation replays it: the replay is not
/// shown again. One that cannot resume says so.
#[test]
fn a_loaded_conversation_is_not_replayed_and_some_agents_cannot_resume() {
    let home = fake_agent_home_with(&[]);
    run_in(home.path(), "", &["hello"], "dumb", false);

    set_agent_args(home.path(), &["--load-only", "--sessions-from", "10"]);
    let screen = run_in(home.path(), "", &["#resume 1", "who"], "dumb", false);
    assert!(screen.contains("Resumed session 1"), "{screen}");
    assert!(screen.contains("[s1|default|"), "{screen}");
    assert!(!screen.contains("replayed old answer"), "{screen}");

    set_agent_args(home.path(), &["--no-resume"]);
    let screen = run_in(home.path(), "", &["#resume 1"], "dumb", false);
    assert!(
        screen.contains("cannot resume session 1: this agent cannot resume a conversation"),
        "{screen}"
    );
}

/// The agent gets the history as an MCP server, `parolsh mcp` for this
/// project, except in a private conversation.
#[test]
fn the_agent_gets_the_history_as_an_mcp_server() {
    let home = fake_agent_home_with(&[]);
    let screen = run_in(
        home.path(),
        "",
        &["mcp", "#new private", "mcp"],
        "dumb",
        false,
    );
    let db = home.path().join("parolsh/history.db");
    let expected = format!(
        r#"[{{"name": "parolsh-history", "args": ["mcp", "--db", "{}", "--project", "{}"]}}]"#,
        db.display(),
        env!("CARGO_MANIFEST_DIR")
    );

    assert!(screen.contains(&expected), "{screen}");
    let private = &screen[screen.find("Started a private conversation").unwrap()..];
    assert!(private.contains("\n[]\n"), "{private}");
}

/// `parolsh mcp` answers MCP over stdio: the tools, and a search in the
/// project's sessions.
#[test]
fn parolsh_mcp_serves_the_history() {
    use std::io::Write;
    let home = fake_agent_home_with(&[]);
    run_in(home.path(), "", &["why does it retry"], "dumb", false);

    let mut server = parolsh()
        .args(["mcp", "--db"])
        .arg(home.path().join("parolsh/history.db"))
        .args(["--project", env!("CARGO_MANIFEST_DIR")])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_history","arguments":{"query":"retry"}}}"#,
    ];
    writeln!(server.stdin.take().unwrap(), "{}", requests.join("\n")).unwrap();
    let output = server.wait_with_output().unwrap();
    let replies: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(
        replies[0]["result"]["serverInfo"]["name"],
        "parolsh-history"
    );
    let found = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(found.contains("why does it [retry]"), "{found}");
}

/// `--continue` goes back to the project's last session and `--resume N` to
/// session N, before the first prompt: the agent is in its old conversation.
#[test]
fn the_resume_and_continue_flags_go_back_at_start() {
    let home = fake_agent_home_with(&[]);
    run_in(home.path(), "", &["hello"], "dumb", false);
    // The next runs' own conversations start at s10: only a resume gives s1.
    set_agent_args(home.path(), &["--sessions-from", "10"]);

    for flags in ["--continue", "--resume 1"] {
        let screen = run_in(home.path(), flags, &["who"], "dumb", false);
        assert!(screen.contains("Resumed session 1"), "{flags}: {screen}");
        assert!(screen.contains("[s1|default|"), "{flags}: {screen}");
        assert!(!screen.contains("[s10|"), "{flags}: {screen}");
    }

    let screen = run_in(home.path(), "--resume 9", &[], "dumb", false);
    assert!(
        screen.contains("parolsh: no session 9 in this project"),
        "{screen}"
    );
    let empty = fake_agent_home_with(&[]);
    let screen = run_in(empty.path(), "--continue", &[], "dumb", false);
    assert!(
        screen.contains("parolsh: no session saved in this project yet"),
        "{screen}"
    );
}
