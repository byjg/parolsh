//! Markdown for an ANSI terminal, rendered while the answer streams in.
//!
//! No line or block buffering: the only text held back is a run of marker
//! characters at the end of a chunk (`*`, `` ` ``, or `#`/`-` at the start of
//! a line) whose meaning depends on the next character. Handled: `**bold**`,
//! `` `code` ``, code fences, `#` headings and `-`/`*` bullets. Markers are
//! hidden. Styles never outlive a line, so an unclosed `**` only affects the
//! rest of its line.

/// Emphasis, code and heading state, turned into one SGR sequence on change.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Style {
    bold: bool,
    code: bool,
    heading: bool,
    /// Inside a fenced code block.
    fence: bool,
    /// The ```` ``` ```` line that opens or closes a fence.
    fence_marker: bool,
}

impl Style {
    /// Reset, then everything active: correct whatever was on before.
    fn sgr(self) -> String {
        let mut codes = String::from("\x1b[0");
        if self.heading {
            codes.push_str(";1;4");
        }
        if self.bold {
            codes.push_str(";1");
        }
        if self.fence_marker {
            codes.push_str(";2");
        }
        if self.code || self.fence {
            codes.push_str(";36");
        }
        codes.push('m');
        codes
    }
}

#[derive(Debug)]
pub struct Markdown {
    style: Style,
    /// The next character starts a line.
    line_start: bool,
    /// Marker characters whose meaning depends on the next chunk.
    pending: String,
    /// The current line closes a fence: it ends at the newline.
    closing_fence: bool,
}

impl Default for Markdown {
    fn default() -> Self {
        Self {
            style: Style::default(),
            line_start: true,
            pending: String::new(),
            closing_fence: false,
        }
    }
}

impl Markdown {
    /// Renders a chunk. A trailing marker run may be held back until the
    /// next chunk or `finish`.
    pub fn push(&mut self, chunk: &str) -> String {
        let input = std::mem::take(&mut self.pending) + chunk;
        self.render(&input, false)
    }

    /// Renders what was held back and closes every style: the end of the
    /// answer, or something else (a tool call, a question) is printed next.
    pub fn finish(&mut self) -> String {
        let input = std::mem::take(&mut self.pending);
        let mut out = self.render(&input, true);
        self.set(&mut out, Style::default());
        self.line_start = true;
        out
    }

    /// The current style again, for when something else (the status line)
    /// reset the terminal's attributes in between.
    pub fn resume(&self) -> String {
        if self.style == Style::default() {
            String::new()
        } else {
            self.style.sgr()
        }
    }

    /// True when text is held back: `finish` will print it (never a newline).
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// True when the cursor is at the start of a line (nothing held back
    /// has been printed yet).
    pub fn at_line_start(&self) -> bool {
        self.line_start
    }

    fn set(&mut self, out: &mut String, style: Style) {
        if style != self.style {
            self.style = style;
            out.push_str(&style.sgr());
        }
    }

    fn render(&mut self, input: &str, last: bool) -> String {
        let chars: Vec<char> = input.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let run = chars[i..].iter().take_while(|&&next| next == c).count();
            // A marker run at the end may continue in the next chunk.
            let open_ended = !last && i + run == chars.len();

            if c == '\n' {
                // No style outlives its line, except an open code fence.
                let style = Style {
                    fence: self.style.fence && !self.closing_fence,
                    ..Style::default()
                };
                self.closing_fence = false;
                self.set(&mut out, style);
                out.push('\n');
                self.line_start = true;
                i += 1;
                continue;
            }

            if self.line_start {
                match self.line_marker(&chars[i..], last) {
                    LineMarker::Wait => break,
                    LineMarker::Fence(skip) => {
                        // The fence stays on through its closing line.
                        self.closing_fence = self.style.fence;
                        let style = Style {
                            fence: true,
                            fence_marker: true,
                            ..Style::default()
                        };
                        self.set(&mut out, style);
                        out.push_str("```");
                        i += skip;
                        self.line_start = false;
                        continue;
                    }
                    LineMarker::Heading(skip) => {
                        let style = Style {
                            heading: true,
                            ..Style::default()
                        };
                        self.set(&mut out, style);
                        i += skip;
                        self.line_start = false;
                        continue;
                    }
                    LineMarker::Bullet(indent, skip) => {
                        out.push_str(&" ".repeat(indent));
                        out.push_str("• ");
                        i += skip;
                        self.line_start = false;
                        continue;
                    }
                    LineMarker::None => self.line_start = false,
                }
            }

            if self.style.fence {
                out.push(c);
                i += 1;
                continue;
            }

            match c {
                '`' => {
                    let style = Style {
                        code: !self.style.code,
                        ..self.style
                    };
                    self.set(&mut out, style);
                    i += 1;
                }
                '*' if !self.style.code && open_ended => break,
                '*' if !self.style.code && run >= 2 => {
                    let style = Style {
                        bold: !self.style.bold,
                        ..self.style
                    };
                    self.set(&mut out, style);
                    i += 2;
                }
                _ => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        self.pending = chars[i..].iter().collect();
        out
    }

    /// What the start of a line is, looking at no more than the line's
    /// first few characters.
    fn line_marker(&self, rest: &[char], last: bool) -> LineMarker {
        let indent = rest.iter().take_while(|&&c| c == ' ').count();
        let body = &rest[indent..];
        // Everything seen so far could still be a marker: wait for more.
        let need_more = |len: usize| !last && body.len() < len;

        if self.style.fence {
            // Inside a fence only the closing ``` matters.
            return match body {
                ['`', '`', '`', ..] => LineMarker::Fence(indent + fence_line_len(body)),
                _ if need_more(3) && body.iter().all(|&c| c == '`') => LineMarker::Wait,
                _ => LineMarker::None,
            };
        }
        match body {
            [] if !last => LineMarker::Wait,
            ['`', '`', '`', ..] => LineMarker::Fence(indent + fence_line_len(body)),
            ['`'] | ['`', '`'] if !last => LineMarker::Wait,
            ['#', ..] => {
                let hashes = body.iter().take_while(|&&c| c == '#').count();
                match body.get(hashes) {
                    Some(' ') if hashes <= 6 => LineMarker::Heading(indent + hashes + 1),
                    None if !last => LineMarker::Wait,
                    _ => LineMarker::None,
                }
            }
            ['-' | '*', ' ', ..] => LineMarker::Bullet(indent, indent + 2),
            ['-' | '*'] if !last => LineMarker::Wait,
            _ => LineMarker::None,
        }
    }
}

enum LineMarker {
    /// Not enough characters yet to decide.
    Wait,
    /// A code fence line: the characters that are just the fence (the
    /// language tag after it is shown).
    Fence(usize),
    /// A heading: the characters to hide (`#`s and the space).
    Heading(usize),
    /// A bullet: its indentation, and the characters to replace.
    Bullet(usize, usize),
    None,
}

/// The ``` of a fence line; what follows (the language) stays visible.
fn fence_line_len(body: &[char]) -> usize {
    body.iter().take_while(|&&c| c == '`').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SGR sequences as readable tags.
    fn tags(text: &str) -> String {
        text.replace("\x1b[0;1;4m", "<h>")
            .replace("\x1b[0;1;36m", "<bc>")
            .replace("\x1b[0;2;36m", "<fence>")
            .replace("\x1b[0;36m", "<c>")
            .replace("\x1b[0;1m", "<b>")
            .replace("\x1b[0m", "</>")
    }

    fn render(text: &str) -> String {
        let mut markdown = Markdown::default();
        let out = markdown.push(text) + &markdown.finish();
        tags(&out)
    }

    #[test]
    fn bold_and_code_hide_their_markers() {
        assert_eq!(
            render("o kernel vem do **HWE oficial**, pacote `linux-image`."),
            "o kernel vem do <b>HWE oficial</>, pacote <c>linux-image</>."
        );
    }

    #[test]
    fn code_wins_over_bold() {
        assert_eq!(render("use `a**b` here"), "use <c>a**b</> here");
    }

    #[test]
    fn bold_code_nests() {
        assert_eq!(render("**see `x`**"), "<b>see <bc>x<b></>");
    }

    #[test]
    fn an_unclosed_style_ends_with_its_line() {
        assert_eq!(render("2 **3\nnext"), "2 <b>3</>\nnext");
        assert_eq!(render("a `b\nc"), "a <c>b</>\nc");
    }

    #[test]
    fn a_single_star_is_text() {
        assert_eq!(render("2 * 3 = 6"), "2 * 3 = 6");
    }

    #[test]
    fn bullets_become_dots() {
        assert_eq!(
            render("- **Pacote:** um\n  * dois\n-3"),
            "• <b>Pacote:</> um\n  • dois\n-3"
        );
    }

    #[test]
    fn headings_lose_their_hashes() {
        assert_eq!(render("## Kernel\ntext"), "<h>Kernel</>\ntext");
        assert_eq!(render("#hashtag"), "#hashtag");
    }

    #[test]
    fn a_fence_is_one_code_block() {
        assert_eq!(
            render("```bash\napt **x** `y`\n```\nafter"),
            "<fence>```bash<c>\napt **x** `y`\n<fence>```</>\nafter"
        );
    }

    #[test]
    fn finish_closes_what_is_open() {
        assert_eq!(render("**cut"), "<b>cut</>");
        assert_eq!(render("trailing *"), "trailing *");
    }

    /// The real answer that motivated this: however the stream is cut, the
    /// output is the same.
    const SAMPLE: &str = "Confirmado: o kernel vem do **HWE (Hardware Enablement) oficial do Ubuntu**. Não é mainline nem PPA.\n\n\
- **Pacote:** `/boot/vmlinuz-7.0.0-31-generic` pertence ao `linux-image-7.0.0-31-generic`, versão `7.0.0-31.31~24.04.1`.\n\
- **Repositório:** o `apt-cache policy` mostra `noble-updates/main`.\n\
- **Metapacote:** subindo aos poucos (6.8 → 6.11 → 7.0), 2 * 3.\n\n\
## Próximo passo\n\n\
```bash\nsudo reboot # **agora**\n```\n\
Falta um `reboot`.";

    #[test]
    fn the_output_does_not_depend_on_how_the_stream_is_cut() {
        let whole = render(SAMPLE);
        let chars: Vec<char> = SAMPLE.chars().collect();

        for split in 0..=chars.len() {
            let (a, b): (String, String) = (
                chars[..split].iter().collect(),
                chars[split..].iter().collect(),
            );
            let mut markdown = Markdown::default();
            let out = markdown.push(&a) + &markdown.push(&b) + &markdown.finish();
            assert_eq!(tags(&out), whole, "split at {split}");
        }

        let mut markdown = Markdown::default();
        let mut out = String::new();
        for c in chars {
            out += &markdown.push(&c.to_string());
        }
        out += &markdown.finish();
        assert_eq!(tags(&out), whole, "one character at a time");
    }

    /// A real Qwen Code answer, fed one character at a time like a stream.
    #[test]
    fn a_real_answer_streamed_character_by_character() {
        let answer = "## Kernel\n\n- **Reboot required** `kernel`\n- **Action pending** `reboot`\n\n```bash\nsudo reboot\n```";
        let mut markdown = Markdown::default();
        let mut out = String::new();
        for c in answer.chars() {
            out += &markdown.push(&c.to_string());
        }
        out += &markdown.finish();

        assert_eq!(
            tags(&out),
            "<h>Kernel</>\n\n• <b>Reboot required</> <c>kernel</>\n• <b>Action pending</> <c>reboot</>\n\n<fence>```bash<c>\nsudo reboot\n<fence>```</>"
        );
    }

    #[test]
    fn the_sample_renders_as_expected() {
        let out = render(SAMPLE);

        assert!(out.starts_with(
            "Confirmado: o kernel vem do <b>HWE (Hardware Enablement) oficial do Ubuntu</>."
        ));
        assert!(out.contains("\n• <b>Pacote:</> <c>/boot/vmlinuz-7.0.0-31-generic</> pertence"));
        assert!(out.contains("(6.8 → 6.11 → 7.0), 2 * 3."));
        assert!(out.contains("\n<h>Próximo passo</>\n"));
        assert!(out.contains("<fence>```bash<c>\nsudo reboot # **agora**\n<fence>```</>\n"));
        assert!(out.ends_with("Falta um <c>reboot</>."));
    }
}
