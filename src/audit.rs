//! `#audit`: what happened in this run of Parolsh, kept in memory only. What
//! ran locally, what was sent to the agent, and what the agent did, with your
//! answers. Commands and titles only: outputs and answers stay in the
//! scrollback. Bounded: at most `limit` entries, of at most `MAX_TEXT`
//! characters each.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

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
    fn name(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Shared => "SHARED",
            Self::Agent => "AGENT",
            Self::Parolsh => "PAROLSH",
        }
    }
}

struct Entry {
    at: Duration,
    actor: Actor,
    text: String,
}

pub struct Audit {
    started: Instant,
    /// The newest entries, at most `limit`.
    entries: VecDeque<Entry>,
    limit: usize,
    /// Entries dropped to stay within `limit`: the number of the oldest kept.
    dropped: usize,
}

impl Audit {
    /// Keeps the last `limit` entries; 0 keeps none.
    pub fn new(limit: usize) -> Self {
        Self {
            started: Instant::now(),
            entries: VecDeque::new(),
            limit,
            dropped: 0,
        }
    }

    /// Changes how many entries are kept, dropping the oldest beyond it.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.trim();
    }

    /// Adds an entry, and returns its number for `replace`. Only its first
    /// line is kept, at most `MAX_TEXT` characters.
    pub fn add(&mut self, actor: Actor, text: impl AsRef<str>) -> usize {
        self.entries.push_back(Entry {
            at: self.started.elapsed(),
            actor,
            text: clean(text.as_ref()),
        });
        self.trim();
        self.dropped + self.entries.len().saturating_sub(1)
    }

    /// Changes the text of entry `number`: a tool call that got a new title
    /// or finished. Nothing when it was already dropped.
    pub fn replace(&mut self, number: usize, text: impl AsRef<str>) {
        if let Some(entry) = number
            .checked_sub(self.dropped)
            .and_then(|i| self.entries.get_mut(i))
        {
            entry.text = clean(text.as_ref());
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
            .chain(self.entries.iter().map(|entry| {
                format!(
                    "{:>7}  {:<8} {}",
                    clock(entry.at),
                    entry.actor.name(),
                    entry.text
                )
            }))
            .map(|line| cut(&line, width))
            .collect()
    }

    fn trim(&mut self) {
        while self.entries.len() > self.limit {
            self.entries.pop_front();
            self.dropped += 1;
        }
    }
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

    #[test]
    fn lines_show_time_actor_and_text() {
        let mut audit = Audit::new(10);
        audit.add(Actor::User, "!git status · exit 0");
        let tool = audit.add(Actor::Agent, "edit: Write notes.txt");
        audit.replace(tool, "edit: Write notes.txt · completed");

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
        audit.add(Actor::Parolsh, "x".repeat(100));

        let line = &audit.lines(40)[0];

        assert_eq!(line.chars().count(), 40);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn an_entry_keeps_one_short_line() {
        let mut audit = Audit::new(10);
        audit.add(
            Actor::Agent,
            format!("Bash(cat <<EOF\n{}\nEOF)", "y".repeat(5000)),
        );
        audit.add(Actor::User, "x".repeat(5000));

        let entries: Vec<&str> = audit.entries.iter().map(|e| e.text.as_str()).collect();

        assert_eq!(entries[0], "Bash(cat <<EOF …");
        assert_eq!(entries[1].chars().count(), MAX_TEXT);
    }

    #[test]
    fn only_the_last_entries_are_kept() {
        let mut audit = Audit::new(2);
        let first = audit.add(Actor::User, "one");
        audit.add(Actor::User, "two");
        let third = audit.add(Actor::User, "three");

        // The first is gone: replacing it changes nothing.
        audit.replace(first, "changed");
        audit.replace(third, "three · completed");

        let lines = audit.lines(80);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "(1 older entries dropped, see audit_entries)");
        assert!(lines[1].ends_with("USER     two"));
        assert!(lines[2].ends_with("USER     three · completed"));
    }

    #[test]
    fn a_limit_of_zero_keeps_nothing_and_a_lower_limit_drops() {
        let mut audit = Audit::new(0);
        audit.add(Actor::User, "one");
        assert_eq!(
            audit.lines(80),
            ["(1 older entries dropped, see audit_entries)"]
        );

        let mut audit = Audit::new(5);
        for text in ["a", "b", "c"] {
            audit.add(Actor::User, text);
        }
        audit.set_limit(1);
        assert_eq!(audit.lines(80).len(), 2);
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
