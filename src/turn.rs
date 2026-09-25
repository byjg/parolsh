//! Showing one agent turn: streamed text, tool calls and permission prompts.

use agent_client_protocol::schema::v1::{PermissionOption, PermissionOptionKind, StopReason};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::acp::{AgentHandle, Event};
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

/// Sends `text` and prints the agent's answer as it arrives.
pub fn run(agent: &AgentHandle, text: String) -> Outcome {
    if !agent.prompt(text) {
        return Outcome::AgentStopped;
    }
    INTERRUPTED.store(false, Ordering::SeqCst);
    let mut out = Output::new(ui::is_ansi());
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
            Event::Tool(title) => out.tool(&title),
            Event::Permission {
                title,
                options,
                reply,
            } => {
                out.hide();
                out.end_line();
                let _ = reply.send(ask_permission(&title, &options));
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

/// Numbered choice of the agent's options. Anything else rejects once.
fn ask_permission(
    title: &str,
    options: &[PermissionOption],
) -> Option<agent_client_protocol::schema::v1::PermissionOptionId> {
    println!("Permission requested: {title}");
    for (i, option) in options.iter().enumerate() {
        println!("  [{}] {}", i + 1, option.name);
    }
    print!("Choose: ");
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    choose(options, answer.trim()).map(|option| option.option_id.clone())
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
}

struct Status {
    started: Instant,
    activity: String,
    frame: usize,
    tools: usize,
    shown: bool,
}

impl Output {
    fn new(ansi: bool) -> Self {
        Self {
            mid_line: false,
            status: ansi.then(|| Status {
                started: Instant::now(),
                activity: "Thinking".to_string(),
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
        print!("{text}");
        let _ = std::io::stdout().flush();
        self.mid_line = !text.ends_with('\n');
        if let Some(status) = &mut self.status {
            status.activity = "Writing".to_string();
        }
        self.show();
    }

    fn tool(&mut self, title: &str) {
        match &mut self.status {
            Some(status) => {
                status.tools += 1;
                status.activity = format!("Running: {title}");
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
