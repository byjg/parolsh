//! `#audit`: what happened in this run of Parolsh. What ran locally, what
//! was sent to the agent, and what the agent did, with your answers, one
//! short line each, in memory: at most `limit` entries of at most `MAX_TEXT`
//! characters.
//!
//! Each entry is a `Record` (who, what kind, its full text, details), also
//! written to the history of sessions when there is one: there the full
//! text is kept, the agent's answers and reasoning too, and `!command` lines
//! only with `save_commands`. `#audit <session>` reads an earlier session
//! back from it.

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::history::{History, SessionStart, Stored};

/// The longest entry text kept: a tool title can hold a whole script.
const MAX_TEXT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// What you typed or answered.
    User,
    /// Output of `!+` sent to the agent.
    Shared,
    Agent,
    /// What Parolsh itself did: the end of a turn, a cancel.
    Parolsh,
}

impl Actor {
    const ALL: [Self; 4] = [Self::User, Self::Shared, Self::Agent, Self::Parolsh];

    fn name(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Shared => "SHARED",
            Self::Agent => "AGENT",
            Self::Parolsh => "PAROLSH",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|actor| actor.key() == name)
    }

    /// The name stored in the history.
    fn key(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Shared => "shared",
            Self::Agent => "agent",
            Self::Parolsh => "parolsh",
        }
    }
}

/// What an entry is. The text is the full content: the command, the
/// message, the output shared, the tool's title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `!command` or `!bash`; meta `exit`. Saved only with `save_commands`.
    Command,
    /// `!+command`; meta `exit`, `kept` (bytes), `truncated`.
    Capture,
    /// The `!+` outputs sent with a message; meta `outputs`, `bytes`, `agent`.
    Share,
    /// A message to the agent; meta `agent`, `files` (linked).
    Message,
    /// The agent's answer. History only.
    Answer,
    /// The agent's reasoning. History only.
    Thought,
    /// A tool call; meta `kind`, `files`, `finished`.
    Tool,
    /// A permission request, or questions (meta `questions`).
    Ask,
    /// Your answer to one.
    Reply,
    /// A `#command`; meta `failed`.
    Control,
    /// What Parolsh did: a turn's end, a cancel, the agent stopping.
    Event,
}

impl Kind {
    const ALL: [Self; 11] = [
        Self::Command,
        Self::Capture,
        Self::Share,
        Self::Message,
        Self::Answer,
        Self::Thought,
        Self::Tool,
        Self::Ask,
        Self::Reply,
        Self::Control,
        Self::Event,
    ];

    /// The name stored in the history.
    fn key(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Capture => "capture",
            Self::Share => "share",
            Self::Message => "message",
            Self::Answer => "answer",
            Self::Thought => "thought",
            Self::Tool => "tool",
            Self::Ask => "ask",
            Self::Reply => "reply",
            Self::Control => "control",
            Self::Event => "event",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == name)
    }

    /// Answers and reasoning stay out of `#audit` for the current run: they
    /// are in the scrollback.
    fn in_memory(self) -> bool {
        !matches!(self, Self::Answer | Self::Thought)
    }
}

/// One thing that happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub actor: Actor,
    pub kind: Kind,
    pub text: String,
    pub meta: Value,
}

impl Record {
    pub fn new(actor: Actor, kind: Kind, text: impl Into<String>) -> Self {
        Self {
            actor,
            kind,
            text: text.into(),
            meta: json!({}),
        }
    }

    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = meta;
        self
    }

    /// The line `#audit` shows: `!git status · exit 0`, `→ claude: why…`.
    pub fn line(&self) -> String {
        let meta = &self.meta;
        let number = |key: &str| meta.get(key).and_then(Value::as_i64).unwrap_or(0);
        let flag = |key: &str| meta.get(key).and_then(Value::as_bool).unwrap_or(false);
        let string = |key: &str| meta.get(key).and_then(Value::as_str).unwrap_or("");
        match self.kind {
            Kind::Command => format!("!{} · exit {}", excerpt(&self.text, 120), number("exit")),
            Kind::Capture => format!(
                "!+{} · exit {} · {} kept{}",
                excerpt(&self.text, 120),
                number("exit"),
                size(number("kept") as usize),
                if flag("truncated") { " (its end)" } else { "" }
            ),
            Kind::Share => format!(
                "{} output(s), {} → {}",
                number("outputs"),
                size(number("bytes") as usize),
                string("agent")
            ),
            Kind::Message => {
                let linked = match number("files") {
                    0 => String::new(),
                    1 => " · 1 file linked".to_string(),
                    n => format!(" · {n} files linked"),
                };
                format!("→ {}: {}{linked}", string("agent"), excerpt(&self.text, 80))
            }
            Kind::Answer => format!("answer: {}", excerpt(&self.text, 120)),
            Kind::Thought => format!("thinking: {}", excerpt(&self.text, 120)),
            Kind::Tool => {
                let mut line = String::new();
                let kind = string("kind");
                // `other` is ACP's kind when the agent gives none.
                if !kind.is_empty() && kind != "other" {
                    line.push_str(&format!("{kind}: "));
                }
                line.push_str(&excerpt(&self.text, 120));
                let files: Vec<&str> = meta
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|files| files.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                if !files.is_empty() {
                    line.push_str(&format!(" [{}]", files.join(", ")));
                }
                match meta.get("finished").and_then(Value::as_bool) {
                    Some(true) => line.push_str(" · completed"),
                    Some(false) => line.push_str(" · failed"),
                    None => {}
                }
                line
            }
            Kind::Ask => match number("questions") {
                0 => format!("asks: {}", excerpt(&self.text, 120)),
                n => format!("asks {n} question(s): {}", self.text),
            },
            Kind::Reply => format!("→ {}", self.text),
            Kind::Control => format!(
                "#{}{}",
                self.text.trim_end(),
                if flag("failed") { " · failed" } else { "" }
            ),
            Kind::Event => self.text.clone(),
        }
    }
}

struct Entry {
    at: Duration,
    actor: Actor,
    text: String,
}

/// An entry to `update`: where it is in memory and in the history.
#[derive(Debug, Clone, Copy, Default)]
pub struct Handle {
    memory: Option<usize>,
    row: Option<i64>,
}

pub struct Audit {
    started: Instant,
    /// The newest entries, at most `limit`.
    entries: VecDeque<Entry>,
    limit: usize,
    /// Entries dropped to stay within `limit`: the number of the oldest kept.
    dropped: usize,
    history: Option<History>,
    /// Write `!command` lines to the history.
    save_commands: bool,
}

impl Audit {
    /// Keeps the last `limit` entries; 0 keeps none.
    pub fn new(limit: usize) -> Self {
        Self {
            started: Instant::now(),
            entries: VecDeque::new(),
            limit,
            dropped: 0,
            history: None,
            save_commands: false,
        }
    }

    /// Also writes to `history`; `!command` lines only with `save_commands`.
    pub fn with_history(mut self, history: History, save_commands: bool) -> Self {
        self.history = Some(history);
        self.save_commands = save_commands;
        self
    }

    pub fn history(&self) -> Option<&History> {
        self.history.as_ref()
    }

    pub fn history_mut(&mut self) -> Option<&mut History> {
        self.history.as_mut()
    }

    /// A new conversation: a new session in the history.
    pub fn begin(&mut self, start: SessionStart) {
        if let Some(history) = &mut self.history {
            history.begin(start);
        }
    }

    /// `#new private`: nothing goes to the history until the next `begin`.
    pub fn pause(&mut self) {
        if let Some(history) = &mut self.history {
            history.pause();
        }
    }

    /// Changes how many entries are kept, dropping the oldest beyond it.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.trim();
    }

    /// Adds `record`, and returns where it went, for `update`. In memory
    /// only its first line is kept, at most `MAX_TEXT` characters.
    pub fn add(&mut self, record: Record) -> Handle {
        let memory = record.kind.in_memory().then(|| {
            self.entries.push_back(Entry {
                at: self.started.elapsed(),
                actor: record.actor,
                text: clean(&record.line()),
            });
            self.trim();
            self.dropped + self.entries.len().saturating_sub(1)
        });
        // A `#command` alone does not make a session: it is only kept in
        // one that has started.
        let saved = match record.kind {
            Kind::Command => self.save_commands,
            Kind::Control => self.history.as_ref().is_some_and(|h| h.current().is_some()),
            _ => true,
        };
        let row = if saved {
            self.write(|history| {
                history.add(
                    record.actor.key(),
                    record.kind.key(),
                    &record.text,
                    &record.meta,
                )
            })
            .flatten()
        } else {
            None
        };
        Handle { memory, row }
    }

    /// Changes the entry added as `handle`: a tool call that got a new title
    /// or finished. Nothing in memory when it was already dropped.
    pub fn update(&mut self, handle: Handle, record: &Record) {
        if let Some(entry) = handle
            .memory
            .and_then(|number| number.checked_sub(self.dropped))
            .and_then(|i| self.entries.get_mut(i))
        {
            entry.text = clean(&record.line());
        }
        if let Some(row) = handle.row {
            self.write(|history| history.update(row, &record.text, &record.meta));
        }
    }

    /// One line per entry, cut to `width`: the time since Parolsh started,
    /// who, and what. Starts with how many older entries were dropped.
    pub fn lines(&self, width: usize) -> Vec<String> {
        let dropped = (self.dropped > 0).then(|| {
            format!(
                "({} older entries dropped, see audit_entries)",
                self.dropped
            )
        });
        dropped
            .into_iter()
            .chain(
                self.entries
                    .iter()
                    .map(|entry| line(entry.at, entry.actor, &entry.text)),
            )
            .map(|line| cut(&line, width))
            .collect()
    }

    /// Runs `write` on the history. A failure turns the history off for the
    /// rest of the run, once said.
    fn write<T>(&mut self, write: impl FnOnce(&mut History) -> rusqlite::Result<T>) -> Option<T> {
        let history = self.history.as_mut()?;
        match write(history) {
            Ok(value) => Some(value),
            Err(e) => {
                eprintln!("parolsh: history: {e}; not saving this run any more");
                self.history = None;
                None
            }
        }
    }

    fn trim(&mut self) {
        while self.entries.len() > self.limit {
            self.entries.pop_front();
            self.dropped += 1;
        }
    }
}

/// The lines of an earlier session, as `#audit` shows the current one, cut
/// to `width`. Reasoning is left out; answers are shown as an excerpt.
pub fn stored_lines(entries: &[Stored], width: usize) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| {
            let actor = Actor::from_name(&entry.actor)?;
            let kind = Kind::from_name(&entry.kind)?;
            (kind != Kind::Thought).then(|| {
                let record = Record {
                    actor,
                    kind,
                    text: entry.text.clone(),
                    meta: entry.meta.clone(),
                };
                cut(&line(entry.at, actor, &clean(&record.line())), width)
            })
        })
        .collect()
}

fn line(at: Duration, actor: Actor, text: &str) -> String {
    format!("{:>7}  {:<8} {}", clock(at), actor.name(), text)
}

/// The first line of `text`, without control characters, at most
/// `MAX_TEXT` characters.
fn clean(text: &str) -> String {
    let line: String = excerpt(text, MAX_TEXT)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    line
}

/// `m:ss`, or `h:mm:ss` after an hour.
fn clock(at: Duration) -> String {
    let secs = at.as_secs();
    match secs / 3600 {
        0 => format!("{}:{:02}", secs / 60, secs % 60),
        hours => format!("{hours}:{:02}:{:02}", secs / 60 % 60, secs % 60),
    }
}

fn cut(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    let kept: String = line.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// The first line of a message, at most `max` characters, for an entry.
pub fn excerpt(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let more = text.trim().lines().count() > 1;
    let line = cut(line.trim(), max);
    if more && !line.ends_with('…') {
        format!("{line} …")
    } else {
        line
    }
}

/// A size in bytes as `n B` or `n KB`.
pub fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(line: &str) -> Record {
        Record::new(Actor::User, Kind::Command, line).meta(json!({"exit": 0}))
    }

    fn event(text: &str) -> Record {
        Record::new(Actor::Parolsh, Kind::Event, text)
    }

    #[test]
    fn lines_show_time_actor_and_text() {
        let mut audit = Audit::new(10);
        audit.add(command("git status"));
        let tool =
            Record::new(Actor::Agent, Kind::Tool, "Write notes.txt").meta(json!({"kind": "edit"}));
        let handle = audit.add(tool.clone());
        audit.update(
            handle,
            &tool.meta(json!({"kind": "edit", "finished": true})),
        );

        assert_eq!(
            audit.lines(80),
            [
                "   0:00  USER     !git status · exit 0",
                "   0:00  AGENT    edit: Write notes.txt · completed",
            ]
        );
    }

    #[test]
    fn a_long_entry_is_cut_to_the_width() {
        let mut audit = Audit::new(10);
        audit.add(event(&"x".repeat(100)));

        let line = &audit.lines(40)[0];

        assert_eq!(line.chars().count(), 40);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn an_entry_keeps_one_short_line() {
        let mut audit = Audit::new(10);
        audit.add(Record::new(
            Actor::Agent,
            Kind::Tool,
            format!("Bash(cat <<EOF\n{}\nEOF)", "y".repeat(5000)),
        ));
        audit.add(event(&"x".repeat(5000)));

        let entries: Vec<&str> = audit.entries.iter().map(|e| e.text.as_str()).collect();

        assert_eq!(entries[0], "Bash(cat <<EOF …");
        assert_eq!(entries[1].chars().count(), MAX_TEXT);
    }

    #[test]
    fn only_the_last_entries_are_kept() {
        let mut audit = Audit::new(2);
        let first = audit.add(event("one"));
        audit.add(event("two"));
        let third = audit.add(event("three"));

        // The first is gone: updating it changes nothing.
        audit.update(first, &event("changed"));
        audit.update(third, &event("three · completed"));

        let lines = audit.lines(80);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "(1 older entries dropped, see audit_entries)");
        assert!(lines[1].ends_with("PAROLSH  two"));
        assert!(lines[2].ends_with("PAROLSH  three · completed"));
    }

    #[test]
    fn a_limit_of_zero_keeps_nothing_and_a_lower_limit_drops() {
        let mut audit = Audit::new(0);
        audit.add(event("one"));
        assert_eq!(
            audit.lines(80),
            ["(1 older entries dropped, see audit_entries)"]
        );

        let mut audit = Audit::new(5);
        for text in ["a", "b", "c"] {
            audit.add(event(text));
        }
        audit.set_limit(1);
        assert_eq!(audit.lines(80).len(), 2);
    }

    #[test]
    fn a_tool_line_shows_kind_files_and_how_it_ended() {
        let tool = |meta| Record::new(Actor::Agent, Kind::Tool, "Write notes").meta(meta);

        assert_eq!(
            tool(json!({"kind": "edit", "files": ["a.txt", "b.txt"]})).line(),
            "edit: Write notes [a.txt, b.txt]"
        );
        assert_eq!(
            tool(json!({"kind": "edit", "files": ["a.txt"], "finished": true})).line(),
            "edit: Write notes [a.txt] · completed"
        );
        assert_eq!(
            tool(json!({"kind": "other", "finished": false})).line(),
            "Write notes · failed"
        );
    }

    #[test]
    fn each_kind_has_its_line() {
        let line = |actor, kind, text: &str, meta| Record::new(actor, kind, text).meta(meta).line();

        assert_eq!(
            line(
                Actor::User,
                Kind::Capture,
                "git diff",
                json!({"exit": 1, "kept": 2048, "truncated": true})
            ),
            "!+git diff · exit 1 · 2 KB kept (its end)"
        );
        assert_eq!(
            line(
                Actor::Shared,
                Kind::Share,
                "...",
                json!({"outputs": 2, "bytes": 7, "agent": "claude"})
            ),
            "2 output(s), 7 B → claude"
        );
        assert_eq!(
            line(
                Actor::User,
                Kind::Message,
                "why?\nmore",
                json!({"agent": "claude", "files": 2})
            ),
            "→ claude: why? … · 2 files linked"
        );
        assert_eq!(
            line(
                Actor::Agent,
                Kind::Ask,
                "Pick a color",
                json!({"questions": 2})
            ),
            "asks 2 question(s): Pick a color"
        );
        assert_eq!(
            line(Actor::Agent, Kind::Ask, "Write a.txt", json!({})),
            "asks: Write a.txt"
        );
        assert_eq!(
            line(Actor::User, Kind::Reply, "Allow once", json!({})),
            "→ Allow once"
        );
        assert_eq!(
            line(
                Actor::User,
                Kind::Control,
                "cd /nowhere",
                json!({"failed": true})
            ),
            "#cd /nowhere · failed"
        );
        assert_eq!(line(Actor::User, Kind::Control, "new ", json!({})), "#new");
    }

    fn with_history(save_commands: bool) -> (tempfile::TempDir, Audit) {
        let dir = tempfile::tempdir().unwrap();
        let history = History::open(&dir.path().join("history.db"), 90).unwrap();
        let mut audit = Audit::new(10).with_history(history, save_commands);
        audit.begin(SessionStart {
            project: "/p".to_string(),
            agent: Some("claude".to_string()),
            cwd: "/p".to_string(),
        });
        (dir, audit)
    }

    fn saved(audit: &Audit) -> Vec<String> {
        let history = audit.history().unwrap();
        let id = history.current().unwrap();
        stored_lines(&history.entries("/p", id).unwrap().unwrap(), 200)
            .iter()
            .map(|line| line[9..].to_string())
            .collect()
    }

    #[test]
    fn commands_reach_the_history_only_with_save_commands() {
        for save_commands in [false, true] {
            let (_dir, mut audit) = with_history(save_commands);
            audit.add(command("mysql -psecret"));
            audit.add(
                Record::new(Actor::User, Kind::Message, "hi").meta(json!({"agent": "claude"})),
            );

            // #audit of this run has the command either way.
            assert!(audit.lines(80)[0].ends_with("USER     !mysql -psecret · exit 0"));
            let expected: &[&str] = if save_commands {
                &["USER     !mysql -psecret · exit 0", "USER     → claude: hi"]
            } else {
                &["USER     → claude: hi"]
            };
            assert_eq!(saved(&audit), expected, "save_commands = {save_commands}");
        }
    }

    #[test]
    fn answers_are_saved_but_not_listed_for_this_run() {
        let (_dir, mut audit) = with_history(false);
        audit.add(Record::new(Actor::Agent, Kind::Thought, "Let me look"));
        audit.add(Record::new(Actor::Agent, Kind::Answer, "It retries twice."));

        assert!(audit.lines(80).is_empty());
        // Reasoning is left out of #audit of an earlier session too.
        assert_eq!(saved(&audit), ["AGENT    answer: It retries twice."]);
    }

    #[test]
    fn an_update_reaches_memory_and_history() {
        let (_dir, mut audit) = with_history(false);
        let tool = Record::new(Actor::Agent, Kind::Tool, "Read a").meta(json!({"kind": "read"}));
        let handle = audit.add(tool.clone());
        let mut done = tool;
        done.text = "Read a.rs".to_string();
        done.meta["finished"] = json!(true);
        audit.update(handle, &done);

        assert!(audit.lines(80)[0].ends_with("AGENT    read: Read a.rs · completed"));
        assert_eq!(saved(&audit), ["AGENT    read: Read a.rs · completed"]);
    }

    #[test]
    fn a_hash_command_alone_does_not_make_a_session() {
        let (_dir, mut audit) = with_history(false);
        let control = Record::new(Actor::User, Kind::Control, "sessions ");
        audit.add(control.clone());
        assert!(audit.history().unwrap().current().is_none());

        audit.add(Record::new(Actor::User, Kind::Message, "hi").meta(json!({"agent": "claude"})));
        audit.add(control);
        assert_eq!(
            saved(&audit),
            ["USER     → claude: hi", "USER     #sessions"]
        );
    }

    #[test]
    fn a_paused_session_writes_nothing() {
        let (_dir, mut audit) = with_history(true);
        audit.pause();
        audit.add(command("ls"));

        assert_eq!(audit.lines(80).len(), 1);
        assert!(audit.history().unwrap().current().is_none());
    }

    #[test]
    fn clock_counts_minutes_then_hours() {
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn excerpt_keeps_the_first_line() {
        assert_eq!(excerpt("why is it slow?", 40), "why is it slow?");
        assert_eq!(excerpt("\nfirst\nsecond", 40), "first …");
        assert_eq!(excerpt("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn sizes_in_bytes_or_kilobytes() {
        assert_eq!(size(900), "900 B");
        assert_eq!(size(12 * 1024), "12 KB");
        assert_eq!(size(1025), "2 KB");
    }
}
