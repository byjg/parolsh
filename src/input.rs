//! Deterministic input routing: no classifier guesses what the user meant.

#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Empty,
    /// Natural language and `/commands`, forwarded unchanged to the agent.
    Agent(String),
    /// `!command`, run by the configured shell.
    Shell(String),
    /// `!bash`, an interactive Bash session.
    Bash,
    /// `#name args`, handled by Parolsh itself.
    Control {
        name: String,
        args: String,
    },
}

pub fn route(line: &str) -> Input {
    let line = line.trim();

    if line.is_empty() {
        return Input::Empty;
    }

    if let Some(command) = line.strip_prefix('!') {
        return match command.trim() {
            "" => Input::Empty,
            "bash" => Input::Bash,
            command => Input::Shell(command.to_string()),
        };
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

    Input::Agent(line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(route("!"), Input::Empty);
    }
}
