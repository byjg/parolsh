//! Console look: the startup banner and the live status line. Both are only
//! used on an ANSI terminal; anywhere else Parolsh prints plain lines.

use std::borrow::Cow;
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use crate::acp::{Activity, ActivityWatch, Compacted, Context, Detail};
use crate::input::Mode;

/// The logo, a speech bubble with a prompt in it (`docs/images/logo.png`). The `>` is
/// white, the cursor `_` green, and the bubble in the gradient below.
const LOGO: [&str; 4] = ["╭─────────╮", "│  > _    │", "╰─┬───────╯", "  ╱"];
/// The logo's width, for the text beside it.
const LOGO_WIDTH: usize = 11;
/// The border's gradient, left to right: purple, blue, green.
const LOGO_GRADIENT: [Rgb; 3] = [(118, 24, 252), (0, 140, 253), (0, 237, 157)];

type Rgb = (u8, u8, u8);
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const BOLD: &str = "\x1b[1m";
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
    indicator: String,
    /// The agent whose running background tasks are shown on the right, and
    /// whether to dim them.
    background: Option<(ActivityWatch, bool)>,
}

/// What a prompt program may show: the last command's result and the agent.
pub struct PromptContext<'a> {
    /// Where shell commands run.
    pub cwd: &'a Path,
    /// The agent's directory, when shell commands moved away from it.
    pub agent_cwd: Option<&'a Path>,
    /// Exit code of the last `!` command or agent turn.
    pub status: i32,
    pub duration: Duration,
    /// Where plain text goes.
    pub mode: Mode,
    pub agent: Option<BannerAgent<'a>>,
}

impl Prompt {
    /// The agent that answers, the directory and where plain text goes, as
    /// in `[claude] ~/…/byjg/parolsh ✦ `: `✦` to the agent, `❯` to the
    /// shell. The symbol is red after a failure. When shell commands moved
    /// away from the agent's directory, it shows both:
    /// `[claude ~/…/byjg/parolsh] /tmp ❯ `.
    pub fn parolsh(context: &PromptContext, home: Option<&Path>, ansi: bool) -> Self {
        let name = context
            .agent
            .as_ref()
            .map_or("no agent", |agent| agent.name);
        let agent = match context.agent_cwd {
            Some(dir) => format!("{name} {}", short_path(dir, home)),
            None => name.to_string(),
        };
        let path = short_path(context.cwd, home);
        let left = if ansi {
            format!("{BLUE}[{agent}]{RESET} {GREEN}{path}{RESET}")
        } else {
            format!("[{agent}] {path}")
        };
        Self {
            left,
            right: String::new(),
            indicator: format!(" {}", indicator(context, ansi)),
            background: None,
        }
    }

    /// Only where plain text goes: `✦` or `❯`.
    pub fn minimal(context: &PromptContext, ansi: bool) -> Self {
        Self {
            left: String::new(),
            right: String::new(),
            indicator: indicator(context, ansi),
            background: None,
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
                .env_remove("PAROLSH_AGENT_DIR")
                .env_remove("PAROLSH_MODE")
                .env(
                    "PAROLSH_INPUT",
                    match context.mode {
                        Mode::Agent => "agent",
                        Mode::Shell => "shell",
                    },
                );
            if let Some(dir) = context.agent_cwd {
                command.env("PAROLSH_AGENT_DIR", dir);
            }
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
            indicator: String::new(),
            background: None,
        })
    }
}

impl Prompt {
    /// Shows on the right, before what is there, `agent`'s running
    /// background tasks (`⧗ 2 bg · 0:42`, with the time of the oldest) and
    /// its context when it is filling up (`ctx 82%`).
    pub fn with_background(mut self, agent: ActivityWatch, ansi: bool) -> Self {
        self.background = Some((agent, ansi));
        self
    }
}

impl reedline::Prompt for Prompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    /// Read on each repaint: the background tasks' time keeps running.
    fn render_prompt_right(&self) -> Cow<'_, str> {
        let Some((agent, ansi)) = &self.background else {
            return Cow::Borrowed(&self.right);
        };
        let activity = agent.get();
        let tasks = background(&activity).map(|tasks| match ansi {
            true => format!("{DIM}{tasks}{RESET}"),
            false => tasks,
        });
        let parts: Vec<String> = [context_warning(&activity, *ansi), tasks]
            .into_iter()
            .flatten()
            .collect();
        if parts.is_empty() {
            return Cow::Borrowed(&self.right);
        }
        let agent = parts.join(" ");
        if self.right.is_empty() {
            Cow::Owned(agent)
        } else {
            Cow::Owned(format!("{agent} {}", self.right))
        }
    }

    fn render_prompt_indicator(&self, _mode: reedline::PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(&self.indicator)
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

/// The running background tasks and how long the oldest has run, as in
/// `⧗ 2 bg · 0:42`; `None` without any.
pub fn background(activity: &Activity) -> Option<String> {
    if activity.tasks == 0 {
        return None;
    }
    let mut text = format!("⧗ {} bg", activity.tasks);
    if let Some(oldest) = activity.oldest {
        text.push_str(&format!(" · {}", clock(oldest.elapsed())));
    }
    Some(text)
}

/// `0:42`, `12:03`, `1:02:03`.
fn clock(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The spinner's character at `frame`.
pub fn spinner(frame: usize) -> char {
    SPINNER[frame % SPINNER.len()]
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
    let agent = match agent {
        Some(BannerAgent {
            name,
            mode: Some(mode),
        }) => format!("agent: {name} ({mode})"),
        Some(BannerAgent { name, mode: None }) => format!("agent: {name}"),
        None => "no agent configured".to_string(),
    };
    let beside = [
        format!("{BOLD}Parolsh{RESET} {DIM}{version}{RESET}"),
        format!("{DIM}{agent}{RESET}"),
        format!("{DIM}project: {}{RESET}", short_path(cwd, home)),
        format!("{DIM}#help{RESET}"),
    ];
    let truecolor = truecolor();
    let mut out = String::new();
    for (art, text) in LOGO.iter().zip(beside) {
        out.push_str(&logo_line(art, truecolor));
        out.push_str(&" ".repeat(LOGO_WIDTH - art.chars().count() + 5));
        out.push_str(&text);
        out.push('\n');
    }
    out.push_str("\nSpeak to your terminal. Natural language first.\n");
    out
}

/// The terminal shows 24-bit colors (`COLORTERM`); otherwise the 256 colors
/// of xterm are used.
fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|value| value == "truecolor" || value == "24bit")
}

/// One row of the logo, each character in its color.
fn logo_line(art: &str, truecolor: bool) -> String {
    let mut line = String::new();
    for (x, ch) in art.chars().enumerate() {
        let (r, g, b) = match ch {
            ' ' => {
                line.push(' ');
                continue;
            }
            '>' => (240, 242, 248),
            '_' => (0, 240, 150),
            _ => gradient(x),
        };
        let color = if truecolor {
            format!("38;2;{r};{g};{b}")
        } else {
            format!("38;5;{}", xterm256((r, g, b)))
        };
        line.push_str(&format!("\x1b[{color}m{ch}{RESET}"));
    }
    line
}

/// The border's color in column `x`.
fn gradient(x: usize) -> Rgb {
    let steps = (LOGO_GRADIENT.len() - 1) as f32;
    let t = x as f32 / (LOGO_WIDTH - 1) as f32 * steps;
    let i = (t as usize).min(LOGO_GRADIENT.len() - 2);
    let (a, b) = (LOGO_GRADIENT[i], LOGO_GRADIENT[i + 1]);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * (t - i as f32)).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// The nearest color of xterm's 6×6×6 cube (16 to 231).
fn xterm256((r, g, b): Rgb) -> u8 {
    let level = |v: u8| -> u8 {
        // The cube's levels: 0, 95, 135, 175, 215, 255.
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            ((v - 35) / 40).min(5)
        }
    };
    16 + 36 * level(r) + 6 * level(g) + level(b)
}

/// Silence after which the status line says how long the agent has been quiet.
const QUIET: Duration = Duration::from_secs(15);
/// Silence after which it also says how to cancel.
const QUIET_HINT: Duration = Duration::from_secs(60);

/// The status line text: spinner, activity, elapsed time and, once the agent
/// has been silent for `QUIET`, for how long. Cut to fit `width` so it never
/// wraps (a wrapped line cannot be redrawn in place): the activity first,
/// then the hint and the quiet time.
pub fn status(
    frame: usize,
    activity: &str,
    elapsed: Duration,
    background: usize,
    quiet: Duration,
    width: usize,
) -> String {
    let spinner = spinner(frame);
    let mut parts = vec![format!(" · {}s", elapsed.as_secs())];
    if background > 0 {
        parts.push(format!(" · {background} bg"));
    }
    if quiet >= QUIET {
        parts.push(format!(" · quiet {}s", quiet.as_secs()));
    }
    if quiet >= QUIET_HINT {
        parts.push(" · Ctrl+C to cancel".to_string());
    }
    // Keep room for the spinner and a few characters of activity.
    while parts.len() > 1 && 2 + 10 + parts.concat().chars().count() >= width {
        parts.pop();
    }
    let tail = parts.concat();
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

/// `text` in the dim style used for secondary output.
pub fn dim(text: &str) -> String {
    format!("{DIM}{text}{RESET}")
}

/// The line printed when a turn ends: `✓ 2 tool calls · 18s`, then how full
/// the agent's context is and what the conversation cost, when it says:
/// `· 24k of 1M (2%) · $0.35`.
pub fn summary(tools: usize, elapsed: Duration, context: Option<&Context>) -> String {
    let tools = match tools {
        0 => String::new(),
        1 => "1 tool call · ".to_string(),
        n => format!("{n} tool calls · "),
    };
    let context = context
        .map(|context| format!(" · {}", usage(context)))
        .unwrap_or_default();
    format!("{DIM}✓ {tools}{}s{context}{RESET}", elapsed.as_secs())
}

/// `24k of 1M (2%)`, and ` · $0.35` with a cost.
fn usage(context: &Context) -> String {
    let mut text = format!(
        "{} of {} ({}%)",
        tokens(context.used),
        tokens(context.size),
        context.percent()
    );
    if let Some(cost) = &context.cost {
        text.push_str(&format!(" · {}", money(cost.amount, &cost.currency)));
    }
    text
}

/// A number of tokens, short: `950`, `24k`, `1.2M`.
pub fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..999_500 => format!("{}k", (count + 500) / 1_000),
        _ => {
            let millions = format!("{:.1}", count as f64 / 1_000_000.0);
            format!("{}M", millions.trim_end_matches(".0"))
        }
    }
}

/// `$0.35`, or `0.35 EUR` for another currency.
pub fn money(amount: f64, currency: &str) -> String {
    match currency {
        "USD" => format!("${amount:.2}"),
        currency => format!("{amount:.2} {currency}"),
    }
}

/// The line for a compaction: `Compacted: 28k → 5k tokens (6.8s)`.
pub fn compacted(compacted: &Compacted) -> String {
    let sizes = match (compacted.before, compacted.after) {
        (Some(before), Some(after)) => format!(": {} → {} tokens", tokens(before), tokens(after)),
        _ => String::new(),
    };
    format!("Compacted{sizes} ({:.1}s)", compacted.took.as_secs_f64())
}

/// From how full the context is said on the right of the prompt, and from
/// where in red.
const CONTEXT_WARNING: u64 = 75;
const CONTEXT_ALMOST_FULL: u64 = 90;

/// `ctx 82%` when the agent's context is filling up: time to compact, or to
/// start a new conversation. `None` below that.
pub fn context_warning(activity: &Activity, ansi: bool) -> Option<String> {
    let percent = activity.context.as_ref()?.percent();
    if percent < CONTEXT_WARNING {
        return None;
    }
    let text = format!("ctx {percent}%");
    Some(match (ansi, percent >= CONTEXT_ALMOST_FULL) {
        (false, _) => text,
        (true, true) => format!("{RED}{text}{RESET}"),
        (true, false) => format!("{YELLOW}{text}{RESET}"),
    })
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

/// Where plain text goes: `✦ ` to the agent, `❯ ` to the shell. Red when
/// the last command failed.
fn indicator(context: &PromptContext, ansi: bool) -> String {
    let symbol = match context.mode {
        Mode::Agent => "✦ ",
        Mode::Shell => "❯ ",
    };
    if ansi && context.status != 0 {
        format!("{RED}{symbol}{RESET}")
    } else {
        symbol.to_string()
    }
}

/// `path` with `~` for the home directory, and only its last two
/// directories: `~/…/byjg/parolsh`.
pub fn short_path(path: &Path, home: Option<&Path>) -> String {
    let full = tilde(path, home);
    let (anchor, rest) = match full.strip_prefix("~/") {
        Some(rest) => ("~/", rest),
        None => match full.strip_prefix('/') {
            Some(rest) => ("/", rest),
            None => ("", full.as_str()),
        },
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() <= 2 {
        return full;
    }
    format!("{anchor}…/{}", parts[parts.len() - 2..].join("/"))
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
    fn banner_shows_the_logo_name_agent_and_project() {
        let text = banner(
            "x.y.z",
            Some(BannerAgent {
                name: "claude",
                mode: Some("auto"),
            }),
            Path::new("/home/joao/projects/opensource/wallet"),
            Some(Path::new("/home/joao")),
        );

        assert_eq!(
            plain(&text),
            "╭─────────╮     Parolsh x.y.z\n\
             │  > _    │     agent: claude (auto)\n\
             ╰─┬───────╯     project: ~/…/opensource/wallet\n\
             \x20 ╱             #help\n\
             \n\
             Speak to your terminal. Natural language first.\n"
        );
    }

    #[test]
    fn banner_without_agent_or_mode() {
        let no_mode = banner(
            "x.y.z",
            Some(BannerAgent {
                name: "kilo",
                mode: None,
            }),
            Path::new("/home/joao"),
            Some(Path::new("/home/joao")),
        );
        let no_agent = banner(
            "x.y.z",
            None,
            Path::new("/srv"),
            Some(Path::new("/home/joao")),
        );

        assert!(plain(&no_mode).contains("│     agent: kilo\n"));
        assert!(plain(&no_agent).contains("│     no agent configured\n"));
        assert!(plain(&no_agent).contains("╯     project: /srv\n"));
    }

    #[test]
    fn the_logo_is_drawn_in_24_bit_colors_or_xterm_256() {
        let true_color = logo_line(LOGO[1], true);
        let palette = logo_line(LOGO[1], false);

        // Purple border on the left, white `>`, green cursor.
        assert!(
            true_color.starts_with("\x1b[38;2;118;24;252m│"),
            "{true_color:?}"
        );
        assert!(
            true_color.contains("\x1b[38;2;240;242;248m>"),
            "{true_color:?}"
        );
        assert!(
            true_color.contains("\x1b[38;2;0;240;150m_"),
            "{true_color:?}"
        );
        assert!(!palette.contains(";2;"), "{palette:?}");
        assert!(palette.contains("\x1b[38;5;"), "{palette:?}");
    }

    #[test]
    fn the_logo_border_goes_from_purple_to_green() {
        assert_eq!(gradient(0), (118, 24, 252));
        assert_eq!(gradient(LOGO_WIDTH - 1), (0, 237, 157));
    }

    #[test]
    fn status_shows_spinner_activity_and_seconds() {
        let text = status(2, "Running: Terminal", Duration::from_secs(12), 0, ZERO, 80);

        assert_eq!(plain(&text), "⠹ Running: Terminal · 12s");
    }

    const ZERO: Duration = Duration::ZERO;

    #[test]
    fn status_counts_the_background_tasks() {
        let text = status(0, "Thinking", Duration::from_secs(4), 2, ZERO, 80);

        assert_eq!(plain(&text), "⠋ Thinking · 4s · 2 bg");
    }

    #[test]
    fn background_tasks_show_with_the_time_of_the_oldest() {
        let now = std::time::Instant::now();
        let running = |tasks, secs| Activity {
            tasks,
            oldest: now.checked_sub(Duration::from_secs(secs)),
            ..Default::default()
        };

        assert_eq!(background(&running(0, 0)), None);
        let one = background(&running(1, 42)).unwrap();
        assert!(one.starts_with("⧗ 1 bg · 0:4"), "{one}");
        assert_eq!(clock(Duration::from_secs(723)), "12:03");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn status_says_how_long_the_agent_has_been_quiet() {
        let secs = Duration::from_secs;
        let line = |quiet| plain(&status(0, "Thinking", secs(376), 0, quiet, 80));

        assert_eq!(line(secs(14)), "⠋ Thinking · 376s");
        assert_eq!(line(secs(15)), "⠋ Thinking · 376s · quiet 15s");
        assert_eq!(
            line(secs(340)),
            "⠋ Thinking · 376s · quiet 340s · Ctrl+C to cancel"
        );
    }

    #[test]
    fn a_narrow_status_drops_the_hint_then_the_quiet_time() {
        let line = |width| {
            plain(&status(
                0,
                "Thinking",
                Duration::from_secs(376),
                0,
                Duration::from_secs(340),
                width,
            ))
        };

        assert_eq!(line(40), "⠋ Thinking · 376s · quiet 340s");
        assert_eq!(line(24), "⠋ Thinking · 376s");
        assert!(line(24).chars().count() < 24);
    }

    #[test]
    fn status_never_wraps_and_keeps_one_line() {
        let long = "Terminal: find / -name '*.rs'\nsecond line";

        let text = plain(&status(0, long, Duration::from_secs(3), 0, ZERO, 24));

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
             echo \"$*|$PWD|$STARSHIP_SHELL|${PAROLSH_AGENT-unset}|${PAROLSH_MODE-unset}|$PAROLSH_INPUT|${PAROLSH_AGENT_DIR-unset}\"\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&fake, permissions).unwrap();
        let context = PromptContext {
            cwd: dir.path(),
            agent_cwd: Some(Path::new("/srv/wallet")),
            status: 3,
            duration: Duration::from_millis(1500),
            mode: Mode::Shell,
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
                "prompt --status=3 --cmd-duration=1500 --terminal-width={width}|{cwd}||codex|agent|shell|/srv/wallet\n"
            )
        );
        assert_eq!(
            prompt.right,
            format!(
                "prompt --right --status=3 --cmd-duration=1500 --terminal-width={width}|{cwd}||codex|agent|shell|/srv/wallet"
            )
        );
        assert_eq!(prompt.indicator, "");
    }

    fn context<'a>(cwd: &'a Path, status: i32, agent: Option<&'a str>) -> PromptContext<'a> {
        PromptContext {
            cwd,
            agent_cwd: None,
            status,
            duration: Duration::ZERO,
            mode: Mode::Agent,
            agent: agent.map(|name| BannerAgent { name, mode: None }),
        }
    }

    #[test]
    fn parolsh_prompt_shows_the_agent_and_the_short_path() {
        let home = Path::new("/home/me");
        let cwd = Path::new("/home/me/Projects/opensource/byjg/parolsh");

        let prompt = Prompt::parolsh(&context(cwd, 0, Some("claude")), Some(home), false);

        assert_eq!(prompt.left, "[claude] ~/…/byjg/parolsh");
        assert_eq!(prompt.indicator, " ✦ ");
    }

    #[test]
    fn the_symbol_says_where_plain_text_goes() {
        let cwd = Path::new("/tmp");
        let mut shell = context(cwd, 0, Some("claude"));
        shell.mode = Mode::Shell;

        assert_eq!(Prompt::parolsh(&shell, None, false).indicator, " ❯ ");
        assert_eq!(Prompt::minimal(&shell, false).indicator, "❯ ");
        let agent = context(cwd, 0, Some("claude"));
        assert_eq!(Prompt::minimal(&agent, false).indicator, "✦ ");
    }

    #[test]
    fn parolsh_prompt_shows_the_agent_directory_when_the_shell_moved() {
        let home = Path::new("/home/me");
        let mut moved = context(Path::new("/tmp"), 0, Some("claude"));
        moved.agent_cwd = Some(Path::new("/home/me/Projects/wallet"));

        let prompt = Prompt::parolsh(&moved, Some(home), false);

        assert_eq!(prompt.left, "[claude ~/Projects/wallet] /tmp");
    }

    #[test]
    fn parolsh_prompt_without_an_agent() {
        let prompt = Prompt::parolsh(&context(Path::new("/tmp"), 0, None), None, false);

        assert_eq!(prompt.left, "[no agent] /tmp");
    }

    #[test]
    fn prompts_turn_red_after_a_failure_on_ansi_terminals() {
        let cwd = Path::new("/tmp");
        let failed = Prompt::parolsh(&context(cwd, 1, Some("claude")), None, true);
        let fine = Prompt::parolsh(&context(cwd, 0, Some("claude")), None, true);

        assert_eq!(failed.indicator, format!(" {RED}✦ {RESET}"));
        assert_eq!(fine.indicator, " ✦ ");
        assert_eq!(plain(&failed.left), "[claude] /tmp");
        let failed = context(cwd, 2, None);
        assert_eq!(
            Prompt::minimal(&failed, true).indicator,
            format!("{RED}✦ {RESET}")
        );
        assert_eq!(Prompt::minimal(&failed, false).indicator, "✦ ");
    }

    #[test]
    fn short_path_keeps_the_last_two_directories() {
        let home = Some(Path::new("/home/me"));
        let short = |path: &str| short_path(Path::new(path), home);

        assert_eq!(short("/home/me"), "~");
        assert_eq!(short("/home/me/Projects/wallet"), "~/Projects/wallet");
        assert_eq!(short("/home/me/a/b/c"), "~/…/b/c");
        assert_eq!(short("/"), "/");
        assert_eq!(short("/etc/nginx"), "/etc/nginx");
        assert_eq!(short("/usr/share/doc/parolsh"), "/…/doc/parolsh");
        assert_eq!(short_path(Path::new("/home/me/x/y/z"), None), "/…/y/z");
    }

    #[test]
    fn starship_failures_are_errors() {
        let context = PromptContext {
            cwd: Path::new("/"),
            agent_cwd: None,
            status: 0,
            duration: Duration::ZERO,
            mode: Mode::Agent,
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

    fn filled(used: u64, size: u64, cost: Option<f64>) -> Context {
        Context {
            used,
            size,
            cost: cost.map(|amount| crate::acp::Cost {
                amount,
                currency: "USD".to_string(),
            }),
        }
    }

    #[test]
    fn the_summary_says_how_full_the_context_is_and_the_cost() {
        let line = |context: &Context| plain(&summary(0, Duration::from_secs(3), Some(context)));

        assert_eq!(
            line(&filled(24_300, 1_000_000, Some(0.351))),
            "✓ 3s · 24k of 1M (2%) · $0.35"
        );
        // Codex: no cost.
        assert_eq!(
            line(&filled(15_368, 258_400, None)),
            "✓ 3s · 15k of 258k (6%)"
        );
    }

    #[test]
    fn tokens_and_money_are_short() {
        let short: Vec<String> = [0, 950, 1_000, 5_061, 27_822, 999_499, 999_500, 1_250_000]
            .into_iter()
            .map(tokens)
            .collect();
        assert_eq!(short, ["0", "950", "1k", "5k", "28k", "999k", "1M", "1.2M"]);
        assert_eq!(money(4.309, "USD"), "$4.31");
        assert_eq!(money(4.309, "EUR"), "4.31 EUR");
    }

    #[test]
    fn a_compaction_says_the_sizes_when_they_are_known() {
        let took = Duration::from_millis(6807);
        let with = Compacted {
            before: Some(28_067),
            after: Some(5_061),
            took,
        };
        assert_eq!(compacted(&with), "Compacted: 28k → 5k tokens (6.8s)");
        let without = Compacted {
            before: None,
            after: None,
            took,
        };
        assert_eq!(compacted(&without), "Compacted (6.8s)");
    }

    #[test]
    fn the_context_is_on_the_right_of_the_prompt_only_when_it_fills_up() {
        let at = |percent| Activity {
            context: Some(filled(percent, 100, None)),
            ..Activity::default()
        };

        assert_eq!(context_warning(&Activity::default(), true), None);
        assert_eq!(context_warning(&at(74), true), None);
        assert_eq!(context_warning(&at(75), false).as_deref(), Some("ctx 75%"));
        let yellow = context_warning(&at(82), true).unwrap();
        assert_eq!(
            (plain(&yellow).as_str(), yellow.contains(YELLOW)),
            ("ctx 82%", true)
        );
        let red = context_warning(&at(95), true).unwrap();
        assert_eq!((plain(&red).as_str(), red.contains(RED)), ("ctx 95%", true));
    }

    #[test]
    fn summary_counts_tool_calls() {
        assert_eq!(plain(&summary(0, Duration::from_secs(2), None)), "✓ 2s");
        assert_eq!(
            plain(&summary(1, Duration::from_secs(5), None)),
            "✓ 1 tool call · 5s"
        );
        assert_eq!(
            plain(&summary(3, Duration::from_secs(18), None)),
            "✓ 3 tool calls · 18s"
        );
    }
}
