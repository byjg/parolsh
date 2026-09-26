//! Visual tips while typing: the line takes the color of where it will go
//! (shell, shared shell, Parolsh, agent command), and a bare prefix shows a
//! dim hint of what it does. Both use `input::route`, the same decision as
//! Enter, so they always match what will happen.

use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, Hinter, History, StyledText};

use crate::input::{Input, route};

/// Colors the marker (`!`, `!+`, `#`, `/`) in bold and the rest of the line
/// in the same color. Text for the agent keeps the terminal's color.
pub struct InputHighlighter;

impl Highlighter for InputHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        let Some(color) = color(line) else {
            styled.push((Style::new(), line.to_string()));
            return styled;
        };
        let indent = line.len() - line.trim_start().len();
        let marker = indent + marker(line.trim_start()).len();
        styled.push((Style::new(), line[..indent].to_string()));
        styled.push((color.bold(), line[indent..marker].to_string()));
        styled.push((color.normal(), line[marker..].to_string()));
        styled
    }
}

/// The color of the line's destination; `None` for text to the agent.
fn color(line: &str) -> Option<Color> {
    // A bare `!` runs nothing yet, but the line is already a shell command.
    if line.trim() == "!" {
        return Some(Color::Yellow);
    }
    match route(line) {
        Input::Shell(_) | Input::Bash => Some(Color::Yellow),
        Input::Share(_) => Some(Color::Green),
        Input::Control { .. } => Some(Color::Magenta),
        Input::Agent(text) if text.starts_with('/') => Some(Color::Blue),
        Input::Agent(_) | Input::Empty => None,
    }
}

/// The routing marker at the start of `line` (already trimmed).
fn marker(line: &str) -> &str {
    if line.starts_with("!+") {
        "!+"
    } else if line.starts_with(['!', '#', '/']) {
        &line[..1]
    } else {
        ""
    }
}

/// While the line is only a marker, says in dim text what it does. The hint
/// cannot be accepted into the line: it is a tip, not a completion.
#[derive(Default)]
pub struct PrefixHinter;

impl Hinter for PrefixHinter {
    fn handle(
        &mut self,
        line: &str,
        _pos: usize,
        _history: &dyn History,
        use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        let hint = hint(line);
        if hint.is_empty() || !use_ansi_coloring {
            return hint.to_string();
        }
        Style::new().fg(Color::DarkGray).paint(hint).to_string()
    }

    fn complete_hint(&self) -> String {
        String::new()
    }

    fn next_hint_token(&self) -> String {
        String::new()
    }
}

/// What a bare marker does.
fn hint(line: &str) -> &'static str {
    match line.trim() {
        "!" => "  run a shell command (!bash opens a Bash session)",
        "!+" => "  run a shell command and send its output with your next message",
        "#" => "  Parolsh command: #help lists them",
        "/" => "  command for the agent, sent as typed",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::FileBackedHistory;

    fn segments(line: &str) -> Vec<(Style, String)> {
        InputHighlighter
            .highlight(line, line.len())
            .buffer
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .collect()
    }

    #[test]
    fn the_line_takes_the_color_of_its_destination() {
        let yellow = Color::Yellow;
        assert_eq!(segments("!"), [(yellow.bold(), "!".into())]);
        assert_eq!(
            segments("!ls -la"),
            [
                (yellow.bold(), "!".into()),
                (yellow.normal(), "ls -la".into())
            ]
        );
        assert_eq!(
            segments("!+docker ps"),
            [
                (Color::Green.bold(), "!+".into()),
                (Color::Green.normal(), "docker ps".into())
            ]
        );
        assert_eq!(
            segments("  #agent list"),
            [
                (Style::new(), "  ".into()),
                (Color::Magenta.bold(), "#".into()),
                (Color::Magenta.normal(), "agent list".into())
            ]
        );
        assert_eq!(
            segments("/review"),
            [
                (Color::Blue.bold(), "/".into()),
                (Color::Blue.normal(), "review".into())
            ]
        );
    }

    #[test]
    fn text_for_the_agent_keeps_its_color() {
        assert_eq!(
            segments("why is ! here?"),
            [(Style::new(), "why is ! here?".into())]
        );
        assert!(segments("").is_empty());
    }

    #[test]
    fn a_bare_marker_shows_what_it_does() {
        let history = FileBackedHistory::new(10).unwrap();
        let mut hinter = PrefixHinter;
        let plain = |hinter: &mut PrefixHinter, line: &str| {
            hinter.handle(line, line.len(), &history, false, "/")
        };

        assert_eq!(
            plain(&mut hinter, "!+"),
            "  run a shell command and send its output with your next message"
        );
        assert!(plain(&mut hinter, "!").starts_with("  run a shell command"));
        assert!(plain(&mut hinter, "#").contains("#help"));
        assert_eq!(plain(&mut hinter, "!ls"), "");
        assert_eq!(plain(&mut hinter, "hello"), "");
    }

    #[test]
    fn a_hint_is_never_inserted_into_the_line() {
        let history = FileBackedHistory::new(10).unwrap();
        let mut hinter = PrefixHinter;
        hinter.handle("!+", 2, &history, true, "/");

        assert_eq!(hinter.complete_hint(), "");
        assert_eq!(hinter.next_hint_token(), "");
    }
}
