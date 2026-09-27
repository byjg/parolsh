//! Deterministic input routing: no classifier guesses what the user meant.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where plain text goes. `!` alone locks it to the shell, `?` alone
/// unlocks it back to the agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Agent,
    Shell,
}

/// The mode, shared with the line editor's colors, hints and completion.
#[derive(Debug, Clone, Default)]
pub struct SharedMode(Arc<AtomicBool>);

impl SharedMode {
    pub fn get(&self) -> Mode {
        if self.0.load(Ordering::Relaxed) {
            Mode::Shell
        } else {
            Mode::Agent
        }
    }

    pub fn set(&self, mode: Mode) {
        self.0.store(mode == Mode::Shell, Ordering::Relaxed);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Empty,
    /// Natural language and `/commands`, forwarded unchanged to the agent.
    Agent(String),
    /// `!command`, run by the configured shell.
    Shell(String),
    /// `!bash`, an interactive Bash session.
    Bash,
    /// `!+command`: run by the shell, its output sent with the next message.
    Share(String),
    /// `#name args`, handled by Parolsh itself.
    Control {
        name: String,
        args: String,
    },
    /// `!` or `?` alone: plain text goes to the shell, or to the agent.
    Lock(Mode),
}

pub fn route(line: &str, mode: Mode) -> Input {
    let line = line.trim();

    match line {
        "" => return Input::Empty,
        "!" => return Input::Lock(Mode::Shell),
        "?" => return Input::Lock(Mode::Agent),
        _ => {}
    }

    if let Some(command) = line.strip_prefix('!') {
        return shell(command);
    }

    if let Some(text) = line.strip_prefix('?') {
        return Input::Agent(text.trim().to_string());
    }

    if let Some(control) = line.strip_prefix('#') {
        let (name, args) = control
            .split_once(char::is_whitespace)
            .unwrap_or((control, ""));
        return Input::Control {
            name: name.to_string(),
            args: args.trim().to_string(),
        };
    }

    match mode {
        Mode::Agent => Input::Agent(line.to_string()),
        // Plain text is a `!` line.
        Mode::Shell => shell(line),
    }
}

/// The `#cd` that does what a plain `cd <dir>` command was meant to: each
/// command runs in its own shell, so its `cd` does not move Parolsh. `None`
/// for anything `#cd` cannot do the same way (`cd -`, `$VARS`, quotes,
/// several commands).
pub fn cd_suggestion(command: &str) -> Option<String> {
    let rest = command.trim().strip_prefix("cd")?;
    let dir = rest.trim();
    let plain = |c: char| !c.is_whitespace() && !";&|<>()$`'\"\\*?[]{}!#".contains(c);
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace))
        || dir.starts_with('-')
        || !dir.chars().all(plain)
    {
        return None;
    }
    Some(if dir.is_empty() {
        "#cd".to_string()
    } else {
        format!("#cd {dir}")
    })
}

/// What follows `!`: a command, `+command` or `bash`.
fn shell(command: &str) -> Input {
    let command = command.trim();
    if let Some(shared) = command.strip_prefix('+') {
        return Input::Share(shared.trim().to_string());
    }
    match command {
        "bash" => Input::Bash,
        command => Input::Shell(command.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Routes in agent mode, the default.
    fn route(line: &str) -> Input {
        super::route(line, Mode::Agent)
    }

    fn shell_mode(line: &str) -> Input {
        super::route(line, Mode::Shell)
    }

    fn control(name: &str, args: &str) -> Input {
        Input::Control {
            name: name.to_string(),
            args: args.to_string(),
        }
    }

    #[test]
    fn natural_language_goes_to_the_agent() {
        assert_eq!(
            route("find files modified today"),
            Input::Agent("find files modified today".to_string())
        );
    }

    #[test]
    fn slash_commands_go_to_the_agent_unchanged() {
        assert_eq!(
            route("/review src/"),
            Input::Agent("/review src/".to_string())
        );
    }

    #[test]
    fn bang_runs_a_shell_command() {
        assert_eq!(
            route("!find . -mtime -1"),
            Input::Shell("find . -mtime -1".to_string())
        );
        assert_eq!(
            route("! git status "),
            Input::Shell("git status".to_string())
        );
    }

    #[test]
    fn bang_bash_opens_a_session() {
        assert_eq!(route("!bash"), Input::Bash);
        assert_eq!(route("! bash "), Input::Bash);
    }

    #[test]
    fn bash_with_arguments_is_a_plain_command() {
        assert_eq!(
            route("!bash script.sh"),
            Input::Shell("bash script.sh".to_string())
        );
    }

    #[test]
    fn bang_plus_shares_the_output() {
        assert_eq!(route("!+docker ps"), Input::Share("docker ps".to_string()));
        assert_eq!(
            route("!+ git log -3"),
            Input::Share("git log -3".to_string())
        );
        assert_eq!(route("!+"), Input::Share(String::new()));
    }

    #[test]
    fn hash_is_a_control_command() {
        assert_eq!(route("#new"), control("new", ""));
        assert_eq!(
            route("#cd  ~/projects/wallet "),
            control("cd", "~/projects/wallet")
        );
        assert_eq!(route("#agent list"), control("agent", "list"));
    }

    #[test]
    fn blank_input_is_empty() {
        assert_eq!(route(""), Input::Empty);
        assert_eq!(route("   "), Input::Empty);
        assert_eq!(shell_mode(""), Input::Empty);
    }

    #[test]
    fn bang_alone_locks_to_the_shell_and_question_mark_unlocks() {
        for line in ["!", " ! "] {
            assert_eq!(route(line), Input::Lock(Mode::Shell));
            assert_eq!(shell_mode(line), Input::Lock(Mode::Shell));
        }
        for line in ["?", " ? "] {
            assert_eq!(route(line), Input::Lock(Mode::Agent));
            assert_eq!(shell_mode(line), Input::Lock(Mode::Agent));
        }
    }

    #[test]
    fn in_shell_mode_plain_text_is_a_shell_command() {
        assert_eq!(shell_mode("git status"), Input::Shell("git status".into()));
        assert_eq!(
            shell_mode("/usr/bin/ls"),
            Input::Shell("/usr/bin/ls".into())
        );
        assert_eq!(shell_mode("bash"), Input::Bash);
        assert_eq!(shell_mode("!ls"), Input::Shell("ls".into()));
        assert_eq!(shell_mode("!+docker ps"), Input::Share("docker ps".into()));
        assert_eq!(shell_mode("#new"), control("new", ""));
    }

    #[test]
    fn a_plain_cd_suggests_hash_cd() {
        assert_eq!(cd_suggestion("cd src"), Some("#cd src".into()));
        assert_eq!(
            cd_suggestion(" cd  ~/projects/billing "),
            Some("#cd ~/projects/billing".into())
        );
        assert_eq!(cd_suggestion("cd ../x.y"), Some("#cd ../x.y".into()));
        assert_eq!(
            cd_suggestion("cd my-project"),
            Some("#cd my-project".into())
        );
        assert_eq!(cd_suggestion("cd"), Some("#cd".into()));
    }

    #[test]
    fn anything_else_suggests_nothing() {
        for command in [
            "cd -",
            "cd -P /tmp",
            "cd $HOME",
            "cd src && make",
            "cd src; ls",
            "cd My\\ Files",
            "cd 'My Files'",
            "cd a b",
            "cdrecord disc.iso",
            "ls",
        ] {
            assert_eq!(cd_suggestion(command), None, "{command}");
        }
    }

    #[test]
    fn question_mark_asks_the_agent_in_any_mode() {
        for route in [route, shell_mode] {
            assert_eq!(route("?why"), Input::Agent("why".into()));
            assert_eq!(
                route("? why did it fail"),
                Input::Agent("why did it fail".into())
            );
            assert_eq!(route("?/compact"), Input::Agent("/compact".into()));
        }
    }
}
