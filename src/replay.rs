//! `#redraw`: the last exchanges of a conversation, printed again from the
//! history of sessions, laid out at the terminal's width as when they
//! happened. It is a view of the conversation, not a copy of the screen: the
//! output of `!commands` was never kept.

use crate::audit::{self, Kind, Record};
use crate::history::Stored;
use crate::markdown::Markdown;
use crate::turn::{self, Display};
use crate::ui;
use crate::wrap::Wrap;

/// Clears the screen and the scrollback, and goes to the top: what is printed
/// next is all there is.
pub const WIPE: &str = "\x1b[H\x1b[2J\x1b[3J";

/// The entries from the `count`-th last message on: the last `count`
/// exchanges, each a message and what followed it. Everything when there are
/// fewer messages, nothing for a `count` of 0.
pub fn last_exchanges(entries: &[Stored], count: usize) -> &[Stored] {
    if count == 0 {
        return &[];
    }
    let messages: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind == "message")
        .map(|(i, _)| i)
        .collect();
    match messages.len().checked_sub(count) {
        Some(first) => &entries[messages[first]..],
        None => entries,
    }
}

/// `entries` as they are shown again: your messages after `prompt`, the
/// answers laid out in at most `width` columns (as they come when not
/// `ansi`), and the rest as the one line `#audit` has for it.
pub fn render(
    entries: &[Stored],
    prompt: &str,
    display: Display,
    ansi: bool,
    width: usize,
) -> String {
    let mut out = String::new();
    for record in entries.iter().filter_map(audit::stored_record) {
        match record.kind {
            Kind::Message => out.push_str(&format!("{prompt}{}\n", record.text.trim_end())),
            Kind::Answer => out.push_str(&answer(&record, display, ansi, width)),
            Kind::Thought => {}
            _ => {
                // Tool calls as where they are printed live: `• title`.
                let bullet = if record.kind == Kind::Tool {
                    "• "
                } else {
                    ""
                };
                let line = format!("{bullet}{}", record.line());
                out.push_str(&if ansi { ui::dim(&line) } else { line });
                out.push('\n');
            }
        }
    }
    out
}

/// An answer, with a blank line after it as before a turn's summary.
fn answer(record: &Record, display: Display, ansi: bool, width: usize) -> String {
    if !ansi {
        return format!("{}\n", record.text.trim_end());
    }
    let mut wrap = Wrap::new(width);
    let mut markdown = display.markdown.then(|| Markdown::new(display.links));
    let mut out = turn::laid_out(&mut wrap, &mut markdown, &record.text);
    if let Some(markdown) = &mut markdown {
        out.push_str(&wrap.feed(&markdown.finish(), true));
    }
    out.push_str(&wrap.end());
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LinkStyle, ThinkingDisplay};
    use serde_json::json;
    use std::time::Duration;

    fn entry(actor: &str, kind: &str, text: &str, meta: serde_json::Value) -> Stored {
        Stored {
            at: Duration::ZERO,
            actor: actor.to_string(),
            kind: kind.to_string(),
            text: text.to_string(),
            meta,
        }
    }

    fn conversation() -> Vec<Stored> {
        vec![
            entry(
                "user",
                "message",
                "first question",
                json!({"agent": "claude"}),
            ),
            entry("agent", "answer", "first answer", json!({})),
            entry("parolsh", "event", "turn ended · 2s", json!({})),
            entry(
                "user",
                "capture",
                "git diff",
                json!({"exit": 0, "kept": 2048}),
            ),
            entry(
                "shared",
                "share",
                "...",
                json!({"outputs": 1, "bytes": 2048, "agent": "claude"}),
            ),
            entry(
                "user",
                "message",
                "second question",
                json!({"agent": "claude"}),
            ),
            entry("agent", "thought", "let me think", json!({})),
            entry(
                "agent",
                "tool",
                "Read a.rs",
                json!({"kind": "read", "finished": true}),
            ),
            entry(
                "agent",
                "answer",
                "second answer goes on for a while",
                json!({}),
            ),
        ]
    }

    fn display() -> Display {
        Display {
            thinking: ThinkingDisplay::Status,
            markdown: true,
            links: LinkStyle::Both,
        }
    }

    /// Without the escape sequences.
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
    fn the_last_exchanges_start_at_a_message() {
        let entries = conversation();

        let last = last_exchanges(&entries, 1);
        assert_eq!(last[0].text, "second question");
        assert_eq!(last.len(), 4);
        // What came between the messages belongs to the exchange before.
        assert_eq!(last_exchanges(&entries, 2).len(), entries.len());
        assert_eq!(last_exchanges(&entries, 9).len(), entries.len());
        assert!(last_exchanges(&entries, 0).is_empty());
    }

    #[test]
    fn exchanges_are_shown_with_the_prompt_the_layout_and_one_line_for_the_rest() {
        let entries = conversation();

        let shown = plain(&render(&entries, "[claude] ~ ✦ ", display(), true, 24));

        assert_eq!(
            shown,
            "[claude] ~ ✦ first question\n\
             ✦ first answer\n\
             \n\
             turn ended · 2s\n\
             !+git diff · exit 0 · 2 KB kept\n\
             1 output(s), 2 KB → claude\n\
             [claude] ~ ✦ second question\n\
             • read: Read a.rs · completed\n\
             ✦ second answer goes on\n\
             \x20 for a while\n\
             \n"
        );
    }

    #[test]
    fn without_ansi_the_answers_are_as_they_came() {
        let entries = conversation();

        let shown = render(&entries[5..], "> ", display(), false, 24);

        assert_eq!(
            shown,
            "> second question\n• read: Read a.rs · completed\nsecond answer goes on for a while\n"
        );
    }
}
