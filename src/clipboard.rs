//! Puts text on the clipboard: with the program configured, else with the
//! one of the desktop Parolsh runs in, else through the terminal (OSC 52),
//! which also works over SSH with the terminals that take it.

use base64::Engine;
use std::io::Write;
use std::process::{Command, Stdio};

/// How the text got there, to say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// A program read it: its name.
    Program(String),
    /// The terminal was asked to take it. Terminals that do not know the
    /// request ignore it, and nothing says so.
    Terminal,
}

/// Copies `text`. `configured` is the `clipboard` setting: a command that
/// reads the text on its standard input.
pub fn copy(text: &str, configured: Option<&[String]>, ansi: bool) -> Result<How, String> {
    let path = std::env::var("PATH").ok();
    let found = |command: &&[&str]| crate::shellenv::find_command(command[0], path.as_deref());
    let set = |variable: &str| std::env::var_os(variable).is_some_and(|value| !value.is_empty());
    let program: Option<Vec<String>> = match configured {
        Some(command) => Some(command.to_vec()),
        None => candidates(set("WAYLAND_DISPLAY"), set("DISPLAY"))
            .iter()
            .find(|command| found(command).is_some())
            .map(|command| command.iter().map(|part| part.to_string()).collect()),
    };
    match program {
        Some(command) => feed(&command, text).map(|()| How::Program(command[0].clone())),
        None if ansi => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(text);
            print!("\x1b]52;c;{encoded}\x07");
            let _ = std::io::stdout().flush();
            Ok(How::Terminal)
        }
        None => Err(
            "no program to copy with: install xclip or wl-clipboard, or set `clipboard` \
             in the configuration"
                .to_string(),
        ),
    }
}

/// The programs to try, by what the session has: Wayland's, X11's, macOS's.
fn candidates(wayland: bool, x11: bool) -> Vec<&'static [&'static str]> {
    let mut programs: Vec<&[&str]> = Vec::new();
    if wayland {
        programs.push(&["wl-copy"]);
    }
    if x11 {
        programs.push(&["xclip", "-selection", "clipboard"]);
        programs.push(&["xsel", "--clipboard", "--input"]);
    }
    if cfg!(target_os = "macos") {
        programs.push(&["pbcopy"]);
    }
    programs
}

/// Runs `command` with `text` on its standard input. Its output is not
/// kept: xclip stays in the background to serve the clipboard, and would
/// hold a pipe open.
fn feed(command: &[String], text: &str) -> Result<(), String> {
    let name = &command[0];
    let mut child = Command::new(name)
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run `{name}`: {e}"))?;
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(text.as_bytes()),
        None => Ok(()),
    };
    let status = child.wait().map_err(|e| format!("`{name}`: {e}"))?;
    written.map_err(|e| format!("`{name}` did not read the text: {e}"))?;
    match status.success() {
        true => Ok(()),
        false => Err(format!("`{name}` failed ({status})")),
    }
}

/// The last fenced code block of `markdown`, without its fence.
pub fn last_code_block(markdown: &str) -> Option<String> {
    let mut last = None;
    let mut open: Option<(String, String)> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        match &mut open {
            None => {
                let fence: String = trimmed.chars().take_while(|c| *c == '`').collect();
                if fence.len() >= 3 {
                    open = Some((fence, String::new()));
                }
            }
            Some((fence, code)) => {
                if trimmed.starts_with(fence.as_str())
                    && trimmed.trim_end_matches('`').trim().is_empty()
                {
                    last = Some(std::mem::take(code));
                    open = None;
                } else {
                    code.push_str(line);
                    code.push('\n');
                }
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_program_reads_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clip");
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("cat > '{}'", file.display()),
        ];

        let how = copy("retry three times\n", Some(&command), false);

        assert_eq!(how, Ok(How::Program("sh".to_string())));
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "retry three times\n"
        );
    }

    #[test]
    fn a_program_that_fails_or_is_missing_is_said() {
        let fails = vec!["sh".to_string(), "-c".to_string(), "exit 3".to_string()];
        let error = copy("x", Some(&fails), false).unwrap_err();
        assert!(error.starts_with("`sh` failed"), "{error}");

        let missing = vec!["no-such-clipboard-program".to_string()];
        let error = copy("x", Some(&missing), false).unwrap_err();
        assert!(
            error.starts_with("cannot run `no-such-clipboard-program`"),
            "{error}"
        );
    }

    #[test]
    fn the_desktop_decides_which_programs_are_tried() {
        let names = |wayland, x11| -> Vec<&str> {
            candidates(wayland, x11)
                .iter()
                .map(|command| command[0])
                .collect()
        };

        if cfg!(not(target_os = "macos")) {
            assert_eq!(names(true, true), ["wl-copy", "xclip", "xsel"]);
            assert_eq!(names(false, true), ["xclip", "xsel"]);
            assert!(names(false, false).is_empty());
        }
    }

    #[test]
    fn the_last_code_block_is_taken_without_its_fence() {
        let answer =
            "Run this:\n\n```bash\ncargo test\n```\n\nThen:\n\n````text\na\n```\nb\n````\nDone.";

        assert_eq!(last_code_block(answer).as_deref(), Some("a\n```\nb\n"));
        assert_eq!(
            last_code_block("```\nonly one\n```").as_deref(),
            Some("only one\n")
        );
        assert_eq!(last_code_block("no code here"), None);
        // A block the agent did not close is not one.
        assert_eq!(last_code_block("```\nunfinished"), None);
    }
}
