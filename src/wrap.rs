//! Lays out the agent's answer for reading, while it streams: lines broken
//! between words at a reading width, `✦` before the first line and the others
//! indented under it, so the answer stands apart from a command's output.
//!
//! The text comes rendered (`markdown`): escape sequences take no room, and
//! the style in effect is put back after each break. Only the word being
//! received is held back. Code blocks and table rows are not wrapped, and a
//! list item wraps under its own text.

/// Before the first line of an answer, in the agent's color.
const MARK: &str = "\x1b[36m✦\x1b[0m ";
/// Before its other lines: as wide as the mark.
const INDENT: &str = "  ";
const INDENT_WIDTH: usize = 2;
const RESET: &str = "\x1b[0m";

#[derive(Debug, Default)]
pub struct Wrap {
    /// Columns a line may take, its indent included; 0 for no limit.
    width: usize,
    /// Columns written on the current line, its indent included; 0 before
    /// its first word.
    column: usize,
    /// The next line starts an answer: it gets the mark.
    start: bool,
    /// Spaces before the line's first word, kept: a nested list's indent.
    lead: usize,
    /// Where a wrapped line continues: under the text of a list item.
    hang: usize,
    /// Spaces after the last word written, not written yet: dropped at a
    /// break.
    spaces: usize,
    /// The word being received, with its escape sequences.
    word: String,
    /// The same without them, to know its width and what it is.
    plain: String,
    /// The last style (SGR) in `word`.
    word_style: Option<String>,
    /// The style in effect in what was written; empty for none.
    style: String,
    /// The line is not broken: a table row.
    verbatim: bool,
}

impl Wrap {
    /// Lines of at most `width` columns; 0 never breaks them.
    pub fn new(width: usize) -> Self {
        Self {
            width,
            start: true,
            ..Self::default()
        }
    }

    /// Follows the terminal: the lines written from now on take at most
    /// `width` columns.
    pub fn set_width(&mut self, width: usize) {
        self.width = width;
    }

    /// The next text starts a new answer, with the mark: something else was
    /// printed in between.
    pub fn restart(&mut self) {
        self.start = true;
    }

    /// True when the cursor is after text on its line.
    pub fn mid_line(&self) -> bool {
        self.column > 0
    }

    /// True when no line is being written and no word is held back.
    pub fn idle(&self) -> bool {
        self.column == 0 && self.word.is_empty()
    }

    /// The style in effect, to put it back after something else (the status
    /// line) reset the terminal's.
    pub fn resume(&self) -> &str {
        &self.style
    }

    /// Takes rendered text, and returns what to print now. `prose` is false
    /// in a code block: its lines are indented but never broken, and nothing
    /// is held back.
    pub fn feed(&mut self, text: &str, prose: bool) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => {
                    let sequence = escape(&mut chars);
                    if sequence.ends_with('m') && sequence.starts_with("\x1b[") {
                        self.word_style = Some(sequence.clone());
                    }
                    self.word.push_str(&sequence);
                }
                '\n' => {
                    self.write_word(&mut out, prose);
                    out.push('\n');
                    self.new_line();
                }
                ' ' | '\t' => {
                    self.write_word(&mut out, prose);
                    if self.column == 0 {
                        self.lead += 1;
                    } else {
                        self.spaces += 1;
                    }
                }
                _ => {
                    self.word.push(c);
                    self.plain.push(c);
                    if !prose {
                        self.write_word(&mut out, prose);
                    }
                }
            }
        }
        out
    }

    /// Writes what is held back and ends the line, if one is being written.
    pub fn end(&mut self) -> String {
        let mut out = String::new();
        self.write_word(&mut out, true);
        if self.column > 0 {
            out.push('\n');
        }
        self.new_line();
        out
    }

    fn new_line(&mut self) {
        self.column = 0;
        self.lead = 0;
        self.hang = 0;
        self.spaces = 0;
        self.verbatim = false;
    }

    /// Writes the word held back: on this line when it fits, on a new one
    /// otherwise.
    fn write_word(&mut self, out: &mut String, prose: bool) {
        let width = self.plain.chars().count();
        if width == 0 {
            // Only escape sequences: they take no room.
            out.push_str(&self.word);
        } else if self.column == 0 {
            out.push_str(if self.start { MARK } else { INDENT });
            self.start = false;
            out.push_str(&" ".repeat(self.lead));
            // Put back a style that goes on from the line before (a code
            // block's): the mark reset it.
            out.push_str(&self.style);
            out.push_str(&self.word);
            self.column = INDENT_WIDTH + self.lead + width;
            self.verbatim = self.plain.starts_with('|');
            if is_list_marker(&self.plain) {
                self.hang = self.lead + width + 1;
            }
        } else if prose
            && !self.verbatim
            && self.width > 0
            && self.column + self.spaces + width > self.width
        {
            if !self.style.is_empty() {
                out.push_str(RESET);
            }
            out.push('\n');
            out.push_str(INDENT);
            out.push_str(&" ".repeat(self.hang));
            out.push_str(&self.style);
            out.push_str(&self.word);
            self.column = INDENT_WIDTH + self.hang + width;
        } else {
            out.push_str(&" ".repeat(self.spaces));
            out.push_str(&self.word);
            self.column += self.spaces + width;
        }
        if width > 0 {
            self.spaces = 0;
        }
        if let Some(style) = self.word_style.take() {
            self.style = if style == RESET { String::new() } else { style };
        }
        self.word.clear();
        self.plain.clear();
    }
}

/// The rest of an escape sequence whose `ESC` was read: a CSI sequence
/// (`ESC [ ... letter`), or an OSC one (`ESC ] ... ST`, a hyperlink).
fn escape(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut sequence = String::from('\x1b');
    match chars.next() {
        Some('[') => {
            sequence.push('[');
            for c in chars.by_ref() {
                sequence.push(c);
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
        Some(']') => {
            sequence.push(']');
            while let Some(c) = chars.next() {
                sequence.push(c);
                if c == '\x07' {
                    break;
                }
                if c == '\x1b' && chars.peek() == Some(&'\\') {
                    sequence.push('\\');
                    chars.next();
                    break;
                }
            }
        }
        Some(c) => sequence.push(c),
        None => {}
    }
    sequence
}

/// `•`, `-`, `*`, or a number with its dot: the start of a list item.
fn is_list_marker(word: &str) -> bool {
    matches!(word, "•" | "-" | "*")
        || word
            .strip_suffix('.')
            .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a whole text looks like, without the mark's color.
    fn laid_out(width: usize, chunks: &[&str]) -> String {
        let mut wrap = Wrap::new(width);
        let mut out = String::new();
        for chunk in chunks {
            out.push_str(&wrap.feed(chunk, true));
        }
        out.push_str(&wrap.end());
        out.replace(MARK, "✦ ")
    }

    #[test]
    fn lines_break_between_words_under_the_mark() {
        let text = laid_out(20, &["one two three four five six seven"]);

        assert_eq!(text, "✦ one two three four\n  five six seven\n");
        assert!(text.lines().all(|line| line.chars().count() <= 20));
    }

    #[test]
    fn a_word_cut_between_chunks_is_not_broken() {
        let text = laid_out(14, &["alpha be", "ta gam", "ma delta"]);

        assert_eq!(text, "✦ alpha beta\n  gamma delta\n");
    }

    #[test]
    fn only_the_word_being_received_is_held_back() {
        let mut wrap = Wrap::new(40);

        assert_eq!(wrap.feed("Hello wor", true).replace(MARK, "✦ "), "✦ Hello");
        assert_eq!(wrap.feed("ld. ", true), " world.");
        assert!(wrap.mid_line());
        assert_eq!(wrap.end(), "\n");
        assert!(!wrap.mid_line());
    }

    #[test]
    fn paragraphs_keep_their_blank_line_and_only_the_first_is_marked() {
        let text = laid_out(40, &["First.\n\nSecond.\n"]);

        assert_eq!(text, "✦ First.\n\n  Second.\n");
    }

    #[test]
    fn a_new_answer_is_marked_again() {
        let mut wrap = Wrap::new(40);
        let mut out = wrap.feed("one\n", true);
        wrap.restart();
        out.push_str(&wrap.feed("two\n", true));

        assert_eq!(out.replace(MARK, "✦ "), "✦ one\n✦ two\n");
    }

    #[test]
    fn escape_sequences_take_no_room_and_the_style_goes_on_after_a_break() {
        let bold = "\x1b[0;1m";
        let text = laid_out(12, &[&format!("say {bold}very loud{RESET} now")]);

        // "✦ say very" is 10 columns: the codes are not counted.
        assert_eq!(
            text,
            format!("✦ say {bold}very{RESET}\n  {bold}loud{RESET} now\n")
        );
    }

    #[test]
    fn a_hyperlink_is_one_word_as_wide_as_its_text() {
        let link = "\x1b]8;;https://example.com/a/very/long/address\x1b\\docs\x1b]8;;\x1b\\";
        let text = laid_out(16, &[&format!("see the {link} now")]);

        assert_eq!(text, format!("✦ see the {link}\n  now\n"));
    }

    #[test]
    fn a_list_item_wraps_under_its_text() {
        let text = laid_out(
            18,
            &["• one two three four\n  • nested item here\n3. third item goes on\n"],
        );

        assert_eq!(
            text,
            "✦ • one two three\n    four\n    • nested item\n      here\n  3. third item\n     goes on\n"
        );
    }

    #[test]
    fn code_blocks_and_table_rows_are_not_broken() {
        let mut wrap = Wrap::new(16);
        let mut out = wrap.feed("let total = compute(a, b);\n", false);
        out.push_str(&wrap.feed("| name | value | notes |\n", true));
        out.push_str(&wrap.feed("after the table it wraps\n", true));

        assert_eq!(
            out.replace(MARK, "✦ "),
            "✦ let total = compute(a, b);\n  | name | value | notes |\n  after the\n  table it wraps\n"
        );
    }

    #[test]
    fn code_is_written_at_once_with_its_indentation() {
        let mut wrap = Wrap::new(40);
        wrap.feed("x\n", true);

        assert_eq!(wrap.feed("    retu", false), "      retu");
        assert_eq!(wrap.feed("rn 1", false), "rn 1");
    }

    #[test]
    fn a_style_that_goes_on_from_the_line_before_is_put_back() {
        let code = "\x1b[0;36m";
        let mut wrap = Wrap::new(40);
        let out = wrap.feed(&format!("{code}first\nsecond\n"), false);

        assert_eq!(
            out.replace(MARK, "✦ "),
            format!("✦ {code}first\n  {code}second\n")
        );
        assert_eq!(wrap.resume(), code);
    }

    #[test]
    fn without_a_width_lines_are_marked_and_indented_but_not_broken() {
        let text = laid_out(
            0,
            &["one two three four five six seven eight nine ten\nnext\n"],
        );

        assert_eq!(
            text,
            "✦ one two three four five six seven eight nine ten\n  next\n"
        );
    }
}
