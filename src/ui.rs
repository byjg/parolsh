//! Console look: the startup banner and the live status line. Both are only
//! used on an ANSI terminal; anywhere else Parolsh prints plain lines.

use std::borrow::Cow;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use crate::acp::Detail;

const LOGO: [&str; 3] = [
    "┏━┓┏━┓┏━┓┏━┓╻  ┏━┓╻ ╻",
    "┣━┛┣━┫┣┳┛┃ ┃┃  ┗━┓┣━┫",
    "╹  ╹ ╹╹┗╸┗━┛┗━╸┗━┛╹ ╹",
];
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
/// Back to column 0 and erase the line.
pub const CLEAR_LINE: &str = "\r\x1b[K";

/// True when stdout is a terminal that understands ANSI escapes.
pub fn is_ansi() -> bool {
    std::io::stdout().is_terminal() && std::env::var("TERM").is_ok_and(|term| term != "dumb")
}

/// Terminal width in columns, 80 when unknown. A terminal can report 0
/// columns (a pseudo-terminal nobody sized yet), which also means unknown.
pub fn width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| columns as usize)
        .filter(|&columns| columns > 0)
        .unwrap_or(80)
}

/// The input prompt, built before each line is read.
#[derive(Debug, PartialEq, Eq)]
pub struct Prompt {
    left: String,
    right: String,
    indicator: &'static str,
}

/// What a prompt program may show: the last command's result and the agent.
pub struct PromptContext<'a> {
    pub cwd: &'a Path,
    /// Exit code of the last `!` command or agent turn.
    pub status: i32,
    pub duration: Duration,
    pub agent: Option<BannerAgent<'a>>,
}

impl Prompt {
    /// The directory name and `❯`, as in `wallet ❯ `.
    pub fn parolsh(label: String) -> Self {
        Self {
            left: label,
            right: String::new(),
            indicator: " ❯ ",
        }
    }

    /// Only `❯`.
    pub fn minimal() -> Self {
        Self {
            left: String::new(),
            right: String::new(),
            indicator: "❯ ",
        }
    }

    /// The user's Starship prompt, from `program prompt` (normally
    /// `starship`). Starship draws its own `❯`. Fails when the program is
    /// missing or exits with an error.
    pub fn starship(program: &str, context: &PromptContext) -> std::io::Result<Self> {
        let run = |right: bool| -> std::io::Result<String> {
            let mut command = std::process::Command::new(program);
            command
                .arg("prompt")
                .args(right.then_some("--right"))
                .arg(format!("--status={}", context.status))
                .arg(format!("--cmd-duration={}", context.duration.as_millis()))
                .arg(format!("--terminal-width={}", width()))
                .current_dir(context.cwd)
                .env("PWD", context.cwd)
                // Empty: plain ANSI, without the markers bash or zsh need.
                .env("STARSHIP_SHELL", "")
                .env_remove("PAROLSH_AGENT")
                .env_remove("PAROLSH_MODE");
            if let Some(agent) = &context.agent {
                command.env("PAROLSH_AGENT", agent.name);
                if let Some(mode) = agent.mode {
                    command.env("PAROLSH_MODE", mode);
                }
            }
            let output = command.output()?;
            if !output.status.success() {
                return Err(std::io::Error::other(format!(
                    "{program} prompt failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        };

        Ok(Self {
            left: run(false)?,
            right: run(true)?.trim_end().to_string(),
            indicator: "",
        })
    }
}

impl reedline::Prompt for Prompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.right)
    }

    fn render_prompt_indicator(&self, _mode: reedline::PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(self.indicator)
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("  ⋮ ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        search: reedline::PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Owned(format!(" (search: {}) ❯ ", search.term))
    }
}

/// The agent shown in the banner: its name and configured mode.
pub struct BannerAgent<'a> {
    pub name: &'a str,
    pub mode: Option<&'a str>,
}

pub fn banner(
    version: &str,
    agent: Option<BannerAgent>,
    cwd: &Path,
    home: Option<&Path>,
) -> String {
    let mut out = String::new();
    for (i, line) in LOGO.iter().enumerate() {
        let version = if i == LOGO.len() - 1 {
            format!("  {DIM}{version}{RESET}")
        } else {
            String::new()
        };
        out.push_str(&format!("  {CYAN}{line}{RESET}{version}\n"));
    }
    out.push_str("  Speak to your terminal. Natural language first.\n");

    let agent = match agent {
        Some(BannerAgent {
            name,
            mode: Some(mode),
        }) => format!("agent: {name} ({mode})"),
        Some(BannerAgent { name, mode: None }) => format!("agent: {name}"),
        None => "no agent configured".to_string(),
    };
    out.push_str(&format!(
        "  {DIM}{agent} · {} · #help{RESET}\n",
        tilde(cwd, home)
    ));
    out
}

/// The status line text: spinner, activity and elapsed time, cut to fit
/// `width` so it never wraps (a wrapped line cannot be redrawn in place).
pub fn status(frame: usize, activity: &str, elapsed: Duration, width: usize) -> String {
    let spinner = SPINNER[frame % SPINNER.len()];
    let tail = format!(" · {}s", elapsed.as_secs());
    let room = width.saturating_sub(2 + tail.chars().count() + 1);
    let activity = one_line(activity);
    let activity: String = if activity.chars().count() > room {
        let cut: String = activity.chars().take(room.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        activity
    };
    format!("{DIM}{spinner} {activity}{tail}{RESET}")
}

/// The end of `text`'s last non-empty line, at most `max` characters: what
/// the agent is thinking right now, for the status line.
pub fn thinking(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let count = line.chars().count();
    if count <= max {
        return line.to_string();
    }
    let tail: String = line.chars().skip(count - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// The lines shown under a permission request: diffs, text, or the tool's
/// input. At most `max_lines`, then a note with how many were left out.
pub fn details(details: &[Detail], ansi: bool, max_lines: usize) -> Vec<String> {
    let paint = |color: &str, line: String| {
        if ansi {
            format!("{color}{line}{RESET}")
        } else {
            line
        }
    };
    let mut lines = Vec::new();
    for detail in details {
        match detail {
            Detail::Text(text) => lines.extend(text.lines().map(str::to_string)),
            Detail::Diff { path, old, new } => {
                lines.push(paint(DIM, format!("{}", path.display())));
                let old = old.as_deref().unwrap_or_default();
                let diff = similar::TextDiff::from_lines(old, new.as_str());
                for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
                    lines.push(paint(DIM, hunk.header().to_string()));
                    for change in hunk.iter_changes() {
                        let text = change.value().trim_end_matches('\n');
                        lines.push(match change.tag() {
                            similar::ChangeTag::Delete => paint(RED, format!("-{text}")),
                            similar::ChangeTag::Insert => paint(GREEN, format!("+{text}")),
                            similar::ChangeTag::Equal => format!(" {text}"),
                        });
                    }
                }
            }
            Detail::Input(value) => render_value(value, 0, &mut lines),
        }
    }
    if lines.len() > max_lines {
        let hidden = lines.len() - max_lines;
        lines.truncate(max_lines);
        lines.push(paint(DIM, format!("… {hidden} more lines")));
    }
    lines
}

/// JSON as indented `key: value` lines, readable in a terminal.
fn render_value(value: &serde_json::Value, indent: usize, lines: &mut Vec<String>) {
    use serde_json::Value;
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match value {
                    Value::Object(_) | Value::Array(_) => {
                        lines.push(format!("{pad}{key}:"));
                        render_value(value, indent + 2, lines);
                    }
                    scalar => lines.push(format!("{pad}{key}: {}", scalar_text(scalar))),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Object(_) | Value::Array(_) => {
                        // The first line of the item carries the dash.
                        let start = lines.len();
                        render_value(item, indent + 2, lines);
                        if let Some(first) = lines.get_mut(start) {
                            first.replace_range(indent..indent + 2, "- ");
                        }
                    }
                    scalar => lines.push(format!("{pad}- {}", scalar_text(scalar))),
                }
            }
        }
        scalar => lines.push(format!("{pad}{}", scalar_text(scalar))),
    }
}

fn scalar_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.replace('\n', " "),
        other => other.to_string(),
    }
}

/// The line printed when a turn ends.
pub fn summary(tools: usize, elapsed: Duration) -> String {
    let tools = match tools {
        0 => String::new(),
        1 => "1 tool call · ".to_string(),
        n => format!("{n} tool calls · "),
    };
    format!("{DIM}✓ {tools}{}s{RESET}", elapsed.as_secs())
}

/// First line of `text`, without control characters.
fn one_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|c| c.is_ascii_alphabetic());
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn banner_shows_the_logo_version_agent_and_directory() {
        let text = banner(
            "0.1.0",
            Some(BannerAgent {
                name: "claude",
                mode: Some("auto"),
            }),
            Path::new("/home/joao/projects/wallet"),
            Some(Path::new("/home/joao")),
        );

        assert_eq!(
            plain(&text),
            "  ┏━┓┏━┓┏━┓┏━┓╻  ┏━┓╻ ╻\n\
             \x20 ┣━┛┣━┫┣┳┛┃ ┃┃  ┗━┓┣━┫\n\
             \x20 ╹  ╹ ╹╹┗╸┗━┛┗━╸┗━┛╹ ╹  0.1.0\n\
             \x20 Speak to your terminal. Natural language first.\n\
             \x20 agent: claude (auto) · ~/projects/wallet · #help\n"
        );
    }

    #[test]
    fn banner_without_agent_or_mode() {
        let no_mode = banner(
            "0.1.0",
            Some(BannerAgent {
                name: "kilo",
                mode: None,
            }),
            Path::new("/home/joao"),
            Some(Path::new("/home/joao")),
        );
        let no_agent = banner(
            "0.1.0",
            None,
            Path::new("/srv"),
            Some(Path::new("/home/joao")),
        );

        assert!(plain(&no_mode).ends_with("agent: kilo · ~ · #help\n"));
        assert!(plain(&no_agent).ends_with("no agent configured · /srv · #help\n"));
    }

    #[test]
    fn status_shows_spinner_activity_and_seconds() {
        let text = status(2, "Running: Terminal", Duration::from_secs(12), 80);

        assert_eq!(plain(&text), "⠹ Running: Terminal · 12s");
    }

    #[test]
    fn status_never_wraps_and_keeps_one_line() {
        let long = "Terminal: find / -name '*.rs'\nsecond line";

        let text = plain(&status(0, long, Duration::from_secs(3), 24));

        assert_eq!(text, "⠋ Terminal: find … · 3s");
        assert!(text.chars().count() < 24);
    }

    #[test]
    fn starship_gets_the_status_duration_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        // Stands in for starship: prints its arguments and environment.
        let fake = dir.path().join("starship");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             echo \"$*|$PWD|$STARSHIP_SHELL|${PAROLSH_AGENT-unset}|${PAROLSH_MODE-unset}\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        let context = PromptContext {
            cwd: dir.path(),
            status: 3,
            duration: Duration::from_millis(1500),
            agent: Some(BannerAgent {
                name: "codex",
                mode: Some("agent"),
            }),
        };

        let prompt = Prompt::starship(fake.to_str().unwrap(), &context).unwrap();

        let cwd = dir.path().display();
        let width = width();
        assert_eq!(
            prompt.left,
            format!(
                "prompt --status=3 --cmd-duration=1500 --terminal-width={width}|{cwd}||codex|agent\n"
            )
        );
        assert_eq!(
            prompt.right,
            format!(
                "prompt --right --status=3 --cmd-duration=1500 --terminal-width={width}|{cwd}||codex|agent"
            )
        );
        assert_eq!(prompt.indicator, "");
    }

    #[test]
    fn starship_failures_are_errors() {
        let context = PromptContext {
            cwd: Path::new("/"),
            status: 0,
            duration: Duration::ZERO,
            agent: None,
        };

        assert!(Prompt::starship("/nonexistent/starship", &context).is_err());
        assert!(Prompt::starship("false", &context).is_err());
    }

    #[test]
    fn thinking_shows_the_end_of_the_last_line() {
        assert_eq!(thinking("First idea.\nSecond idea\n\n", 40), "Second idea");
        assert_eq!(thinking("write exactly LAST LINE HERE", 12), "…T LINE HERE");
        assert_eq!(thinking("", 10), "");
    }

    #[test]
    fn a_diff_shows_the_path_and_changed_lines() {
        let detail = Detail::Diff {
            path: "/tmp/settings.json".into(),
            old: Some("{\n  \"a\": 1\n}\n".to_string()),
            new: "{\n  \"a\": 1,\n  \"enable_thinking\": false\n}\n".to_string(),
        };

        let lines = details(&[detail], false, 40);

        assert_eq!(
            lines,
            [
                "/tmp/settings.json",
                "@@ -1,3 +1,4 @@",
                " {",
                "-  \"a\": 1",
                "+  \"a\": 1,",
                "+  \"enable_thinking\": false",
                " }",
            ]
        );
    }

    #[test]
    fn a_new_file_is_all_added_lines() {
        let detail = Detail::Diff {
            path: "/tmp/test.txt".into(),
            old: None,
            new: "hello".to_string(),
        };

        assert_eq!(
            details(&[detail], false, 40),
            ["/tmp/test.txt", "@@ -0,0 +1 @@", "+hello"]
        );
    }

    #[test]
    fn tool_input_is_shown_as_indented_lines() {
        let input = serde_json::json!({
            "questions": [{
                "question": "Which color do you prefer?",
                "options": [{"label": "Red"}, {"label": "Blue"}]
            }]
        });

        assert_eq!(
            details(&[Detail::Input(input)], false, 40),
            [
                "questions:",
                "  - question: Which color do you prefer?",
                "    options:",
                "      - label: Red",
                "      - label: Blue",
            ]
        );
    }

    #[test]
    fn details_are_capped() {
        let text = (1..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let lines = details(&[Detail::Text(text)], false, 3);

        assert_eq!(lines, ["1", "2", "3", "… 7 more lines"]);
    }

    #[test]
    fn summary_counts_tool_calls() {
        assert_eq!(plain(&summary(0, Duration::from_secs(2))), "✓ 2s");
        assert_eq!(
            plain(&summary(1, Duration::from_secs(5))),
            "✓ 1 tool call · 5s"
        );
        assert_eq!(
            plain(&summary(3, Duration::from_secs(18))),
            "✓ 3 tool calls · 18s"
        );
    }
}
