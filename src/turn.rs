//! Showing one agent turn: streamed text, tool calls and permission prompts.

use agent_client_protocol::schema::v1::{PermissionOption, PermissionOptionKind, StopReason};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::acp::{AgentHandle, Detail, Event};
use crate::config::{LinkStyle, ThinkingDisplay};
use crate::form::{self, Answers, FieldKind, Form};
use crate::markdown::Markdown;
use crate::ui;

/// Set by the SIGINT handler. The terminal is in cooked mode while a turn
/// runs, so Ctrl+C arrives as a signal instead of a key press.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Makes Ctrl+C cancel the running turn instead of killing Parolsh.
pub fn install_interrupt_handler() -> Result<(), ctrlc::Error> {
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst))
}

/// What happened to the agent during the turn.
pub enum Outcome {
    Finished,
    AgentStopped,
}

/// How a turn is shown, from the configuration.
#[derive(Debug, Clone, Copy)]
pub struct Display {
    pub thinking: ThinkingDisplay,
    /// Render markdown (on ANSI terminals only).
    pub markdown: bool,
    pub links: LinkStyle,
}

/// Sends one message (text blocks, the user's text last) and prints the
/// agent's answer as it arrives.
pub fn run(agent: &AgentHandle, blocks: Vec<String>, display: Display) -> Outcome {
    if !agent.prompt_blocks(blocks) {
        return Outcome::AgentStopped;
    }
    INTERRUPTED.store(false, Ordering::SeqCst);
    let ansi = ui::is_ansi();
    let markdown = (ansi && display.markdown).then(|| Markdown::new(display.links));
    let mut out = Output::new(ansi, display.thinking, markdown);
    out.show();

    loop {
        let event = match agent.events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                if INTERRUPTED.swap(false, Ordering::SeqCst) {
                    agent.cancel();
                }
                out.tick();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                out.hide();
                out.end_line();
                return Outcome::AgentStopped;
            }
        };

        match event {
            Event::Text(text) => out.text(&text),
            Event::Thought(text) => out.thought(&text),
            Event::Tool(title) => out.tool(&title),
            Event::Permission {
                title,
                details,
                options,
                reply,
            } => {
                out.hide();
                out.end_line();
                let _ = reply.send(ask_permission(&title, &details, &options));
                out.show();
            }
            Event::Form { form, reply } => {
                out.hide();
                out.end_line();
                let _ = reply.send(ask_form(&form));
                out.show();
            }
            Event::Notice(message) => out.line(&format!("parolsh: {message}")),
            Event::Error(message) => {
                out.line(&format!("parolsh: {message}"));
                out.hide();
                return Outcome::AgentStopped;
            }
            Event::TurnEnd(reason) => {
                out.finish();
                if let Some(message) = describe(reason) {
                    eprintln!("({message})");
                }
                return Outcome::Finished;
            }
        }
    }
}

/// Prints notices and errors that arrived while no turn was running, such as
/// an agent that failed to start. False when the agent stopped.
pub fn drain(agent: &AgentHandle) -> bool {
    while let Ok(event) = agent.events.try_recv() {
        match event {
            Event::Notice(message) => eprintln!("parolsh: {message}"),
            Event::Error(message) => {
                eprintln!("parolsh: {message}");
                return false;
            }
            _ => {}
        }
    }
    true
}

fn describe(reason: StopReason) -> Option<&'static str> {
    match reason {
        StopReason::EndTurn => None,
        StopReason::Cancelled => Some("cancelled"),
        StopReason::MaxTokens => Some("stopped: token limit reached"),
        StopReason::MaxTurnRequests => Some("stopped: request limit reached"),
        StopReason::Refusal => Some("the agent refused to continue"),
        _ => Some("stopped"),
    }
}

/// Lines of permission details shown before asking.
const MAX_DETAIL_LINES: usize = 40;

/// Shows what the agent wants to do, then a numbered choice of its options.
/// Anything else rejects once.
fn ask_permission(
    title: &str,
    details: &[Detail],
    options: &[PermissionOption],
) -> Option<agent_client_protocol::schema::v1::PermissionOptionId> {
    println!("Permission requested: {title}");
    for line in ui::details(details, ui::is_ansi(), MAX_DETAIL_LINES) {
        println!("  {line}");
    }
    for (i, option) in options.iter().enumerate() {
        println!("  [{}] {}", i + 1, option.name);
    }
    print!("Choose: ");
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    choose(options, answer.trim()).map(|option| option.option_id.clone())
}

/// Asks each question of the form. Enter skips a question; `None` (cancel)
/// when the input closes. A skipped required question declines the form.
fn ask_form(form: &Form) -> Option<Answers> {
    println!("The agent asks (press Enter to skip a question):");
    if !form.message.is_empty() {
        println!("  {}", form.message);
    }
    let mut answers = Answers::new();
    for field in &form.fields {
        println!("  {}", field.title);
        if let Some(description) = &field.description {
            println!("  {description}");
        }
        if let FieldKind::Choice { options, .. } = &field.kind {
            for (i, option) in options.iter().enumerate() {
                match &option.description {
                    Some(description) => {
                        println!("    [{}] {} — {description}", i + 1, option.label)
                    }
                    None => println!("    [{}] {}", i + 1, option.label),
                }
            }
        }
        loop {
            print!("  {}: ", hint(&field.kind));
            let _ = std::io::stdout().flush();
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).unwrap_or(0) == 0 {
                return None;
            }
            match form::parse(&field.kind, &input) {
                Ok(Some(answer)) => {
                    answers.insert(field.key.clone(), answer);
                    break;
                }
                Ok(None) => break,
                Err(message) => println!("  ({message})"),
            }
        }
    }
    if form.missing_required(&answers) {
        println!("  A required question was skipped: the agent gets no answers.");
        return Some(Answers::new());
    }
    Some(answers)
}

fn hint(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "Answer",
        FieldKind::Number { .. } => "Number",
        FieldKind::Boolean => "y/n",
        FieldKind::Choice { multiple: true, .. } => "Choose numbers, separated by commas",
        FieldKind::Choice { other: true, .. } => "Choose a number, or type your own answer",
        FieldKind::Choice { .. } => "Choose a number",
    }
}

fn choose<'a>(options: &'a [PermissionOption], answer: &str) -> Option<&'a PermissionOption> {
    answer
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| options.get(i))
        .or_else(|| {
            options
                .iter()
                .find(|option| option.kind == PermissionOptionKind::RejectOnce)
        })
}

/// Prints the turn. On an ANSI terminal a status line (spinner, activity,
/// elapsed time) is redrawn in place on the cursor line while the agent works,
/// and tool calls only update it. Elsewhere tool calls are `• title` lines.
struct Output {
    /// The cursor is after text that did not end with a newline.
    mid_line: bool,
    status: Option<Status>,
    thinking: ThinkingDisplay,
    ansi: bool,
    /// The last thing printed was reasoning (`thinking = "show"`).
    in_thought: bool,
    /// Renders the answer's markdown, on ANSI terminals.
    markdown: Option<Markdown>,
}

struct Status {
    started: Instant,
    activity: String,
    /// The agent's reasoning since the last answer text or tool call.
    thoughts: String,
    frame: usize,
    tools: usize,
    shown: bool,
}

impl Output {
    fn new(ansi: bool, thinking: ThinkingDisplay, markdown: Option<Markdown>) -> Self {
        Self {
            markdown,
            thinking,
            ansi,
            in_thought: false,
            mid_line: false,
            status: ansi.then(|| Status {
                started: Instant::now(),
                activity: "Thinking".to_string(),
                thoughts: String::new(),
                frame: 0,
                tools: 0,
                shown: false,
            }),
        }
    }

    fn text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.hide();
        if self.in_thought {
            // The answer starts on its own line, after the reasoning.
            self.end_line();
            self.in_thought = false;
        }
        match &mut self.markdown {
            Some(markdown) => {
                // The status line resets the terminal's style: restore it.
                print!("{}{}", markdown.resume(), markdown.push(text));
                self.mid_line = !markdown.at_line_start();
            }
            None => {
                print!("{text}");
                self.mid_line = !text.ends_with('\n');
            }
        }
        let _ = std::io::stdout().flush();
        if let Some(status) = &mut self.status {
            status.activity = "Writing".to_string();
            status.thoughts.clear();
        }
        self.show();
    }

    /// Reasoning feeds the status line (`status`), or only shows "Thinking"
    /// (`hidden`), or is printed dim in the scrollback (`show`).
    fn thought(&mut self, text: &str) {
        match self.thinking {
            ThinkingDisplay::Status => {}
            ThinkingDisplay::Hidden => return,
            ThinkingDisplay::Show => return self.print_thought(text),
        }
        if let Some(status) = &mut self.status {
            status.thoughts.push_str(text);
            // Only the last line is shown: keep the buffer small.
            if status.thoughts.len() > 4096 {
                let keep = status.thoughts.len() - 1024;
                let cut = (keep..status.thoughts.len())
                    .find(|&i| status.thoughts.is_char_boundary(i))
                    .unwrap_or(status.thoughts.len());
                status.thoughts.drain(..cut);
            }
            let snippet = ui::thinking(&status.thoughts, ui::width().saturating_sub(24));
            status.activity = if snippet.is_empty() {
                "Thinking".to_string()
            } else {
                format!("Thinking: {snippet}")
            };
        }
        self.show();
    }

    fn print_thought(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.hide();
        if !self.in_thought {
            self.end_line();
            self.in_thought = true;
        }
        if self.ansi {
            print!("{}", ui::dim(text));
        } else {
            print!("{text}");
        }
        let _ = std::io::stdout().flush();
        self.mid_line = !text.ends_with('\n');
        self.show();
    }

    fn tool(&mut self, title: &str) {
        match &mut self.status {
            Some(status) => {
                status.tools += 1;
                status.activity = format!("Running: {title}");
                status.thoughts.clear();
                self.hide();
                self.end_line();
                self.show();
            }
            None => self.line(&format!("• {title}")),
        }
    }

    fn line(&mut self, line: &str) {
        self.hide();
        self.end_line();
        println!("{line}");
        self.show();
    }

    fn end_line(&mut self) {
        // Print what the markdown held back, and close its styles.
        if let Some(markdown) = &mut self.markdown {
            let held_back = markdown.has_pending();
            print!("{}", markdown.finish());
            if held_back {
                self.mid_line = true;
            }
        }
        if self.mid_line {
            println!();
            self.mid_line = false;
        }
    }

    /// Advances the spinner; called while waiting for the agent.
    fn tick(&mut self) {
        if let Some(status) = &mut self.status {
            status.frame += 1;
        }
        self.show();
    }

    /// Draws the status line on the cursor line, unless text is mid-line.
    fn show(&mut self) {
        if self.mid_line {
            return;
        }
        if let Some(status) = &mut self.status {
            let line = ui::status(
                status.frame,
                &status.activity,
                status.started.elapsed(),
                ui::width(),
            );
            print!("{}{line}", ui::CLEAR_LINE);
            let _ = std::io::stdout().flush();
            status.shown = true;
        }
    }

    fn hide(&mut self) {
        if let Some(status) = &mut self.status
            && status.shown
        {
            print!("{}", ui::CLEAR_LINE);
            let _ = std::io::stdout().flush();
            status.shown = false;
        }
    }

    /// Ends the turn: removes the status line and prints the summary.
    fn finish(&mut self) {
        self.hide();
        self.end_line();
        if let Some(status) = &self.status {
            println!("{}", ui::summary(status.tools, status.started.elapsed()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<PermissionOption> {
        vec![
            PermissionOption::new("allow", "Allow once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("always", "Allow always", PermissionOptionKind::AllowAlways),
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
        ]
    }

    fn chosen(answer: &str) -> Option<String> {
        choose(&options(), answer).map(|option| option.option_id.to_string())
    }

    #[test]
    fn a_number_picks_that_option() {
        assert_eq!(chosen("1").as_deref(), Some("allow"));
        assert_eq!(chosen("2").as_deref(), Some("always"));
    }

    #[test]
    fn anything_else_rejects_once() {
        assert_eq!(chosen("").as_deref(), Some("reject"));
        assert_eq!(chosen("0").as_deref(), Some("reject"));
        assert_eq!(chosen("9").as_deref(), Some("reject"));
        assert_eq!(chosen("yes").as_deref(), Some("reject"));
    }

    #[test]
    fn without_a_reject_option_nothing_is_chosen() {
        let allow_only = &options()[..1];

        assert!(choose(allow_only, "").is_none());
    }
}
