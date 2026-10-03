//! Showing one agent turn: streamed text, tool calls and permission prompts.

use agent_client_protocol::schema::v1::{PermissionOption, PermissionOptionKind, StopReason};
use reedline::ExternalPrinter;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::acp::{AgentHandle, Block, Detail, Event, TurnId};
use crate::audit::{self, Actor, Audit, Handle, Kind, Record};
use crate::config::{LinkStyle, ThinkingDisplay};
use crate::form::{self, Answers, FieldKind, Form};
use crate::markdown::Markdown;
use crate::ui;
use crate::wrap::Wrap;

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
    /// The agent answered with an error, and keeps running.
    Failed,
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

/// How long the agent has to confirm a cancel before Parolsh stops waiting.
const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// Sends one message (text blocks, the user's text last) and prints the
/// agent's answer as it arrives.
///
/// Ctrl+C asks the agent to cancel. If it does not confirm within
/// `CANCEL_GRACE`, or on a second Ctrl+C, the turn ends on Parolsh's side and
/// the agent keeps running: its late events are dropped.
///
/// What the agent does and what you answer go to `audit`.
pub fn run(
    agent: &AgentHandle,
    blocks: Vec<Block>,
    display: Display,
    audit: &mut Audit,
) -> Outcome {
    let Some(turn) = agent.prompt_blocks(blocks) else {
        return Outcome::AgentStopped;
    };
    let started = Instant::now();
    let mut tools = ToolAudit::default();
    let mut said = Said::default();
    INTERRUPTED.store(false, Ordering::SeqCst);
    let ansi = ui::is_ansi();
    let markdown = (ansi && display.markdown).then(|| Markdown::new(display.links));
    let mut out = Output::new(ansi, display.thinking, markdown);
    if agent.abandoned().is_some() {
        out.pin("Waiting for the previous turn to stop…");
    }
    out.show();
    let mut cancelled: Option<Instant> = None;
    let activity = agent.activity();

    loop {
        if INTERRUPTED.swap(false, Ordering::SeqCst) {
            if cancelled.is_some() {
                said.flush(audit);
                return give_up(agent, turn, &mut out, audit);
            }
            agent.cancel();
            cancelled = Some(Instant::now());
            out.pin("Cancelling…");
        }
        if cancelled.is_some_and(|at| at.elapsed() >= CANCEL_GRACE) {
            said.flush(audit);
            return give_up(agent, turn, &mut out, audit);
        }

        out.background(activity.get().tasks);
        let event = match agent.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                out.tick();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                said.flush(audit);
                out.hide();
                out.end_line();
                return Outcome::AgentStopped;
            }
        };

        out.heard();
        // Until an abandoned turn ends, what arrives is still its own.
        let stale = agent.abandoned().is_some();
        match event {
            Event::TurnEnd(id, _) | Event::TurnFailed(id, _) if !agent.ended(id) => {
                if agent.abandoned().is_none() && cancelled.is_none() {
                    out.unpin("Thinking");
                }
            }
            Event::Text(_)
            | Event::Thought(_)
            | Event::Tool { .. }
            | Event::ToolUpdate { .. }
            | Event::Plan { .. }
                if stale => {}
            // Dropping the reply cancels the request.
            Event::Permission { .. } | Event::Form { .. } if stale => {}
            Event::Text(text) => {
                said.answer(&text, audit);
                out.text(&text);
            }
            Event::Thought(text) => {
                said.thought(&text, audit);
                out.thought(&text);
            }
            Event::Tool {
                id,
                title,
                kind,
                files,
            } => {
                said.flush(audit);
                tools.start(audit, id.clone(), title.clone(), kind, files);
                out.tool(id, title);
            }
            Event::ToolUpdate {
                id,
                title,
                finished,
            } => {
                tools.update(audit, &id, title.as_deref(), finished);
                out.tool_update(&id, title, finished);
            }
            Event::Plan { step, total, entry } => out.plan(step, total, &entry),
            Event::Activity | Event::Resumed(_) => {}
            request @ (Event::Permission { .. } | Event::Form { .. }) => {
                said.flush(audit);
                out.hide();
                out.end_line();
                out.interrupted();
                answer(request, audit);
                out.show();
            }
            Event::Notice(message) => out.line(&format!("parolsh: {message}")),
            Event::Error(message) => {
                said.flush(audit);
                out.line(&format!("parolsh: {message}"));
                out.hide();
                return Outcome::AgentStopped;
            }
            Event::TurnEnd(_, reason) => {
                said.flush(audit);
                out.finish();
                let how = describe(reason);
                if let Some(message) = how {
                    eprintln!("({message})");
                }
                audit.add(parolsh_event(format!(
                    "turn ended{} · {}s",
                    how.map(|how| format!(" ({how})")).unwrap_or_default(),
                    started.elapsed().as_secs()
                )));
                return Outcome::Finished;
            }
            Event::TurnFailed(_, message) => {
                said.flush(audit);
                out.finish();
                eprintln!("parolsh: {message}");
                audit.add(parolsh_event(format!(
                    "turn failed: {}",
                    audit::excerpt(&message, 80)
                )));
                return Outcome::Failed;
            }
        }
    }
}

/// Ends the turn without the agent's confirmation; the agent keeps running.
fn give_up(agent: &AgentHandle, turn: TurnId, out: &mut Output, audit: &mut Audit) -> Outcome {
    agent.abandon(turn);
    out.finish();
    eprintln!("(cancelled — the agent did not confirm)");
    audit.add(parolsh_event("cancelled, the agent did not confirm"));
    Outcome::Finished
}

fn parolsh_event(text: impl Into<String>) -> Record {
    Record::new(Actor::Parolsh, Kind::Event, text)
}

/// Asks the user the agent's permission request or form, and sends the
/// answer back. Other events are ignored.
pub fn answer(request: Event, audit: &mut Audit) {
    match request {
        Event::Permission {
            title,
            details,
            options,
            reply,
        } => {
            audit.add(Record::new(Actor::Agent, Kind::Ask, title.clone()));
            let choice = ask_permission(&title, &details, &options);
            let answer = choice
                .as_ref()
                .and_then(|id| options.iter().find(|option| &option.option_id == id))
                .map_or("no choice".to_string(), |option| option.name.clone());
            audit.add(Record::new(Actor::User, Kind::Reply, answer));
            let _ = reply.send(choice);
        }
        Event::Form { form, reply } => {
            let questions = form.fields.len();
            audit.add(
                Record::new(Actor::Agent, Kind::Ask, form.message.clone())
                    .meta(json!({"questions": questions})),
            );
            let answers = ask_form(&form);
            let reply_text = match &answers {
                None => "cancelled",
                Some(answers) if answers.is_empty() => "declined",
                Some(_) => "answered",
            };
            audit.add(Record::new(Actor::User, Kind::Reply, reply_text));
            let _ = reply.send(answers);
        }
        _ => {}
    }
}

/// The audit entry of each tool call, updated when it finishes.
#[derive(Default)]
struct ToolAudit(HashMap<String, (Handle, Record)>);

impl ToolAudit {
    fn start(
        &mut self,
        audit: &mut Audit,
        id: String,
        title: String,
        kind: String,
        files: Vec<PathBuf>,
    ) {
        let files: Vec<String> = files
            .iter()
            .map(|file| file.display().to_string())
            .collect();
        let record = Record::new(Actor::Agent, Kind::Tool, title)
            .meta(json!({"kind": kind, "files": files}));
        let handle = audit.add(record.clone());
        self.0.insert(id, (handle, record));
    }

    fn update(&mut self, audit: &mut Audit, id: &str, title: Option<&str>, finished: Option<bool>) {
        if let Some((handle, record)) = self.0.get_mut(id) {
            if let Some(title) = title {
                record.text = title.to_string();
            }
            if let Some(finished) = finished {
                record.meta["finished"] = json!(finished);
            }
            audit.update(*handle, record);
        }
    }
}

/// What the agent said since it last did something else: its reasoning and
/// its answer, each written to the history in one piece when it moves on (a
/// tool call, a question, the end of the turn), not chunk by chunk.
#[derive(Default)]
struct Said {
    thought: String,
    answer: String,
}

impl Said {
    fn answer(&mut self, text: &str, audit: &mut Audit) {
        if !self.thought.is_empty() {
            self.flush(audit);
        }
        self.answer.push_str(text);
    }

    fn thought(&mut self, text: &str, audit: &mut Audit) {
        if !self.answer.is_empty() {
            self.flush(audit);
        }
        self.thought.push_str(text);
    }

    fn flush(&mut self, audit: &mut Audit) {
        for (kind, text) in [
            (Kind::Thought, std::mem::take(&mut self.thought)),
            (Kind::Answer, std::mem::take(&mut self.answer)),
        ] {
            if !text.trim().is_empty() {
                audit.add(Record::new(Actor::Agent, kind, text));
            }
        }
    }
}

/// How a resume (`AgentHandle::resume`) ended.
pub enum Resumed {
    Yes,
    /// The conversation stays the one it was: why.
    No(String),
    AgentStopped(String),
}

/// Waits for a resume to end, printing the agent's notices meanwhile.
pub fn wait_resumed(agent: &AgentHandle) -> Resumed {
    loop {
        match agent.recv_timeout(Duration::from_secs(120)) {
            Ok(Event::Resumed(Ok(()))) => return Resumed::Yes,
            Ok(Event::Resumed(Err(why))) => return Resumed::No(why),
            Ok(Event::Notice(message)) => eprintln!("parolsh: {message}"),
            Ok(Event::Error(message)) => return Resumed::AgentStopped(message),
            Ok(Event::TurnEnd(id, _) | Event::TurnFailed(id, _)) => {
                agent.ended(id);
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Resumed::No("the agent did not answer in 2 minutes".to_string());
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Resumed::AgentStopped("the agent stopped".to_string());
            }
        }
    }
}

/// Prints notices and errors that arrived while no turn was running, such as
/// an agent that failed to start. False when the agent stopped.
pub fn drain(agent: &AgentHandle) -> bool {
    while let Some(event) = agent.try_recv() {
        match event {
            Event::Notice(message) => eprintln!("parolsh: {message}"),
            Event::Error(message) => {
                eprintln!("parolsh: {message}");
                return false;
            }
            Event::TurnEnd(id, _) | Event::TurnFailed(id, _) => {
                agent.ended(id);
            }
            _ => {}
        }
    }
    true
}

/// How long the agent stays quiet before the line it is writing between turns
/// is shown unfinished.
const QUIET: Duration = Duration::from_millis(300);

/// What `watch` leaves to the input loop, once the prompt is gone.
#[derive(Default)]
pub struct Watched {
    /// Lines the prompt was gone before printing, in order.
    pub lines: Vec<String>,
    /// A permission request or form to answer: `answer` it.
    pub request: Option<Event>,
    /// The agent stopped.
    pub stopped: bool,
}

/// Shows what the agent does between turns, while the prompt waits for input:
/// a background task the agent started may end and wake it up. Its text and
/// tool calls are printed above the prompt through `printer`, until `stop` is
/// set. A permission request, a form or the agent stopping need the prompt
/// gone: `watch` sets `interrupt` (the prompt's break signal) and returns.
/// While background tasks run, `repaint` redraws the prompt every second, for
/// their time on the right.
pub fn watch(
    agent: &AgentHandle,
    display: Display,
    printer: &ExternalPrinter<String>,
    repaint: &(dyn Fn() + Sync),
    stop: &AtomicBool,
    interrupt: &AtomicBool,
    audit: &mut Audit,
) -> Watched {
    let ansi = ui::is_ansi();
    let markdown = (ansi && display.markdown).then(|| Markdown::new(display.links));
    let mut lines = Lines::new(ansi, markdown);
    let mut tools = ToolAudit::default();
    let mut said = Said::default();
    let mut watched = Watched::default();
    let mut heard = Instant::now();
    let sender = printer.sender();
    let activity = agent.activity();
    let mut tasks = 0;
    let mut painted = Instant::now();

    while !stop.load(Ordering::SeqCst) {
        let running = activity.get().tasks;
        if running != tasks || (running > 0 && painted.elapsed() >= Duration::from_secs(1)) {
            tasks = running;
            painted = Instant::now();
            repaint();
        }
        // The printer holds a few lines: the rest wait for the prompt to
        // print those.
        while let Some(line) = lines.ready.pop_front() {
            if let Err(full) = sender.try_send(line) {
                lines.ready.push_front(full.into_inner());
                break;
            }
        }
        let event = match agent.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                if heard.elapsed() >= QUIET {
                    lines.flush();
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                watched.stopped = true;
                interrupt.store(true, Ordering::SeqCst);
                break;
            }
        };
        heard = Instant::now();
        // Until an abandoned turn ends, what arrives is still its own.
        let stale = agent.abandoned().is_some();
        match event {
            Event::TurnEnd(id, _) | Event::TurnFailed(id, _) => {
                agent.ended(id);
            }
            // Dropping the reply cancels the request.
            Event::Text(_)
            | Event::Tool { .. }
            | Event::ToolUpdate { .. }
            | Event::Permission { .. }
            | Event::Form { .. }
                if stale => {}
            Event::Text(text) => {
                said.answer(&text, audit);
                lines.text(&text);
            }
            Event::Tool {
                id,
                title,
                kind,
                files,
            } => {
                said.flush(audit);
                tools.start(audit, id, title.clone(), kind, files);
                lines.line(format!("• {title}"));
            }
            Event::ToolUpdate {
                id,
                title,
                finished,
            } => tools.update(audit, &id, title.as_deref(), finished),
            Event::Notice(message) => lines.line(format!("parolsh: {message}")),
            Event::Error(message) => {
                lines.line(format!("parolsh: {message}"));
                watched.stopped = true;
                interrupt.store(true, Ordering::SeqCst);
                break;
            }
            request @ (Event::Permission { .. } | Event::Form { .. }) => {
                watched.request = Some(request);
                interrupt.store(true, Ordering::SeqCst);
                break;
            }
            // The status line is the prompt's now: reasoning and plans are
            // not shown between turns.
            Event::Thought(_) | Event::Plan { .. } | Event::Activity | Event::Resumed(_) => {}
        }
    }
    said.flush(audit);
    lines.flush();
    watched.lines = lines.ready.into();
    watched
}

/// The agent's text between turns, cut into the lines printed above the
/// prompt. The first line comes after a header saying where it comes from.
struct Lines {
    ansi: bool,
    markdown: Option<Markdown>,
    /// Lays the text out at the terminal's width, on ANSI terminals.
    wrap: Option<Wrap>,
    /// The start of the line being written.
    partial: String,
    /// Complete lines, to print.
    ready: VecDeque<String>,
    headed: bool,
}

impl Lines {
    fn new(ansi: bool, markdown: Option<Markdown>) -> Self {
        Self {
            ansi,
            markdown,
            wrap: ansi.then(|| Wrap::new(text_width())),
            partial: String::new(),
            ready: VecDeque::new(),
            headed: false,
        }
    }

    fn text(&mut self, text: &str) {
        // The terminal may have been resized since the last chunk.
        self.text_within(text, text_width());
    }

    /// `text`, in lines of at most `width` columns.
    fn text_within(&mut self, text: &str, width: usize) {
        let text = match &mut self.wrap {
            Some(wrap) => {
                wrap.set_width(width);
                laid_out(wrap, &mut self.markdown, text)
            }
            None => text.to_string(),
        };
        self.partial.push_str(&text);
        self.take_lines();
    }

    /// Makes the complete lines of `partial` ready.
    fn take_lines(&mut self) {
        while let Some(end) = self.partial.find('\n') {
            let line = self.partial[..end].to_string();
            self.partial.drain(..=end);
            self.push(line);
        }
    }

    /// A line of its own, after the text written so far.
    fn line(&mut self, line: String) {
        self.flush();
        self.push(line);
    }

    /// Makes the line being written ready, unfinished.
    fn flush(&mut self) {
        let held_back = self.markdown.as_ref().is_some_and(Markdown::has_pending);
        let writing = self.wrap.as_ref().is_some_and(|wrap| !wrap.idle());
        // Nothing unfinished: leave a style that spans lines (a code block's)
        // open.
        if self.partial.is_empty() && !held_back && !writing {
            return;
        }
        match &mut self.wrap {
            Some(wrap) => {
                let rest = self.markdown.as_mut().map(Markdown::finish);
                let rest = rest.map_or(String::new(), |rest| wrap.feed(&rest, true));
                self.partial.push_str(&rest);
                self.partial.push_str(&wrap.end());
                self.take_lines();
            }
            None => {
                let line = std::mem::take(&mut self.partial);
                self.push(line);
            }
        }
    }

    fn push(&mut self, line: String) {
        if !self.headed {
            self.headed = true;
            let header = "(the agent, between turns)";
            self.ready.push_back(if self.ansi {
                ui::dim(header)
            } else {
                header.to_string()
            });
        }
        self.ready.push_back(line);
    }
}

/// The columns the agent's text may take: the terminal's, less one. A line
/// that fills the terminal makes some of them add a blank line.
pub fn text_width() -> usize {
    ui::width().saturating_sub(1)
}

/// A chunk of the agent's text, rendered and laid out. It goes line by line:
/// the markdown says after each whether it is in a code block, which is not
/// wrapped.
pub fn laid_out(wrap: &mut Wrap, markdown: &mut Option<Markdown>, text: &str) -> String {
    let mut out = String::new();
    for piece in text.split_inclusive('\n') {
        match markdown {
            Some(markdown) => {
                let rendered = markdown.push(piece);
                out.push_str(&wrap.feed(&rendered, !markdown.in_fence()));
            }
            None => out.push_str(&wrap.feed(piece, true)),
        }
    }
    out
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
    /// Lays the answer out at the terminal's width, on ANSI terminals.
    wrap: Option<Wrap>,
    /// The agent wrote something in this turn.
    answered: bool,
}

struct Status {
    started: Instant,
    /// When the agent last sent anything.
    heard: Instant,
    /// What the agent does, unless tools are shown.
    activity: String,
    /// The tool calls started and not finished, oldest first.
    running: Vec<Tool>,
    /// The last thing the agent did concerns its tools: they are shown
    /// instead of `activity`.
    on_tools: bool,
    /// The agent's reasoning since the last answer text or tool call.
    thoughts: String,
    frame: usize,
    tools: usize,
    shown: bool,
    /// The activity is Parolsh's own ("Cancelling…"): the agent's events do
    /// not replace it.
    pinned: bool,
    /// The agent's background tasks running.
    background: usize,
}

struct Tool {
    id: String,
    title: String,
    started: Instant,
}

impl Status {
    fn set_activity(&mut self, activity: String) {
        if !self.pinned {
            self.activity = activity;
            self.on_tools = false;
        }
    }

    fn new() -> Self {
        Self {
            started: Instant::now(),
            heard: Instant::now(),
            activity: "Thinking".to_string(),
            running: Vec::new(),
            on_tools: false,
            thoughts: String::new(),
            frame: 0,
            tools: 0,
            shown: false,
            pinned: false,
            background: 0,
        }
    }

    fn start_tool(&mut self, id: String, title: String) {
        self.tools += 1;
        self.running.push(Tool {
            id,
            title,
            started: Instant::now(),
        });
        self.on_tools = true;
        self.thoughts.clear();
    }

    /// A tool call got a new title, or finished. A failure is shown until the
    /// agent does something else.
    fn update_tool(&mut self, id: &str, title: Option<String>, finished: Option<bool>) {
        let Some(i) = self.running.iter().position(|tool| tool.id == id) else {
            return;
        };
        if let Some(title) = title {
            self.running[i].title = title;
        }
        match finished {
            None => self.on_tools = true,
            Some(true) => {
                self.running.remove(i);
                if self.running.is_empty() {
                    self.set_activity("Thinking".to_string());
                } else {
                    self.on_tools = true;
                }
            }
            Some(false) => {
                let tool = self.running.remove(i);
                self.set_activity(format!("Failed: {}", tool.title));
            }
        }
    }

    /// The activity shown: Parolsh's own, the running tools, or what the
    /// agent does.
    fn activity(&self) -> String {
        if self.pinned || !self.on_tools {
            return self.activity.clone();
        }
        match self.running.as_slice() {
            [] => self.activity.clone(),
            [tool] => format!(
                "Running: {} ({}s)",
                tool.title,
                tool.started.elapsed().as_secs()
            ),
            tools => format!("{} tools running", tools.len()),
        }
    }
}

impl Output {
    fn new(ansi: bool, thinking: ThinkingDisplay, markdown: Option<Markdown>) -> Self {
        Self {
            markdown,
            thinking,
            ansi,
            in_thought: false,
            mid_line: false,
            status: ansi.then(Status::new),
            wrap: ansi.then(|| Wrap::new(text_width())),
            answered: false,
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
        self.answered = true;
        match &mut self.wrap {
            Some(wrap) => {
                // The terminal may have been resized since the last chunk.
                wrap.set_width(text_width());
                // The status line resets the terminal's style: restore it.
                let resumed = wrap.resume().to_string();
                print!("{resumed}{}", laid_out(wrap, &mut self.markdown, text));
                self.mid_line = wrap.mid_line();
            }
            None => {
                print!("{text}");
                self.mid_line = !text.ends_with('\n');
            }
        }
        let _ = std::io::stdout().flush();
        if let Some(status) = &mut self.status {
            status.set_activity("Writing".to_string());
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
            status.set_activity(if snippet.is_empty() {
                "Thinking".to_string()
            } else {
                format!("Thinking: {snippet}")
            });
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

    fn tool(&mut self, id: String, title: String) {
        match &mut self.status {
            Some(status) => {
                status.start_tool(id, title);
                self.hide();
                self.end_line();
                self.show();
            }
            None => self.line(&format!("• {title}")),
        }
    }

    fn tool_update(&mut self, id: &str, title: Option<String>, finished: Option<bool>) {
        if let Some(status) = &mut self.status {
            status.update_tool(id, title, finished);
        }
        self.show();
    }

    /// The agent's plan, at its entry in progress.
    fn plan(&mut self, step: usize, total: usize, entry: &str) {
        if let Some(status) = &mut self.status {
            status.set_activity(format!("Plan {step}/{total}: {entry}"));
        }
        self.show();
    }

    /// How many background tasks the agent runs, shown from the next redraw.
    fn background(&mut self, tasks: usize) {
        if let Some(status) = &mut self.status {
            status.background = tasks;
        }
    }

    /// The agent sent something: it is not quiet.
    fn heard(&mut self) {
        if let Some(status) = &mut self.status {
            status.heard = Instant::now();
        }
    }

    fn line(&mut self, line: &str) {
        self.hide();
        self.end_line();
        self.interrupted();
        println!("{line}");
        self.show();
    }

    fn end_line(&mut self) {
        match &mut self.wrap {
            // Print what the markdown and the wrapping held back, close the
            // styles, and end the line.
            Some(wrap) => {
                let rest = self.markdown.as_mut().map(Markdown::finish);
                let rest = rest.map_or(String::new(), |rest| wrap.feed(&rest, true));
                print!("{rest}{}", wrap.end());
            }
            None if self.mid_line => println!(),
            None => {}
        }
        self.mid_line = false;
    }

    /// Something else is printed (a question, a notice): the answer that
    /// goes on after it is marked again.
    fn interrupted(&mut self) {
        if let Some(wrap) = &mut self.wrap {
            wrap.restart();
        }
    }

    /// Shows Parolsh's own activity, which the agent's events do not replace.
    fn pin(&mut self, activity: &str) {
        if let Some(status) = &mut self.status {
            status.activity = activity.to_string();
            status.pinned = true;
            status.on_tools = false;
        }
        self.show();
    }

    /// Lets the agent's events set the activity again.
    fn unpin(&mut self, activity: &str) {
        if let Some(status) = &mut self.status {
            status.pinned = false;
            status.set_activity(activity.to_string());
        }
        self.show();
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
                &status.activity(),
                status.started.elapsed(),
                status.background,
                status.heard.elapsed(),
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
            // A blank line sets the answer apart from the summary.
            if self.answered {
                println!();
            }
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

    fn tool(status: &mut Status, id: &str, title: &str) {
        status.start_tool(id.to_string(), title.to_string());
    }

    #[test]
    fn one_running_tool_shows_its_title_and_time() {
        let mut status = Status::new();
        tool(&mut status, "t1", "Read a");

        assert_eq!(status.activity(), "Running: Read a (0s)");
        status.update_tool("t1", Some("Read a.rs".to_string()), None);
        assert_eq!(status.activity(), "Running: Read a.rs (0s)");
    }

    #[test]
    fn several_running_tools_are_counted_until_they_finish() {
        let mut status = Status::new();
        tool(&mut status, "t1", "Read a");
        tool(&mut status, "t2", "Read b");

        assert_eq!(status.activity(), "2 tools running");
        status.update_tool("t1", None, Some(true));
        assert_eq!(status.activity(), "Running: Read b (0s)");
        status.update_tool("t2", None, Some(true));
        assert_eq!(status.activity(), "Thinking");
        assert_eq!(status.tools, 2);
    }

    #[test]
    fn a_failed_tool_is_shown_until_the_agent_does_something_else() {
        let mut status = Status::new();
        tool(&mut status, "t1", "Fetch url");
        status.update_tool("t1", None, Some(false));

        assert_eq!(status.activity(), "Failed: Fetch url");
        status.set_activity("Writing".to_string());
        assert_eq!(status.activity(), "Writing");
    }

    #[test]
    fn text_while_a_tool_runs_shows_the_text_until_the_tool_changes() {
        let mut status = Status::new();
        tool(&mut status, "t1", "Read a");
        status.set_activity("Writing".to_string());

        assert_eq!(status.activity(), "Writing");
        status.update_tool("t1", None, None);
        assert_eq!(status.activity(), "Running: Read a (0s)");
    }

    #[test]
    fn a_pinned_activity_stays_over_tools() {
        let mut status = Status::new();
        status.activity = "Cancelling…".to_string();
        status.pinned = true;
        tool(&mut status, "t1", "Read a");
        status.update_tool("t1", None, Some(false));

        assert_eq!(status.activity(), "Cancelling…");
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
    fn an_answer_wraps_its_prose_but_not_its_code_blocks() {
        let mut wrap = Wrap::new(24);
        let mut markdown = Some(Markdown::default());
        let answer = "Run this to **rebuild everything** now:\n\
                      ```\ncargo build --release --locked --all-targets\n```\nThen try it again please.\n";
        // In chunks that cut words and markers, as an agent streams.
        let mut out = String::new();
        for chunk in answer.as_bytes().chunks(7) {
            let chunk = std::str::from_utf8(chunk).unwrap();
            out.push_str(&laid_out(&mut wrap, &mut markdown, chunk));
        }
        out.push_str(&wrap.end());

        assert_eq!(
            plain(&out),
            "✦ Run this to rebuild\n  everything now:\n  ```\n  cargo build --release --locked --all-targets\n  ```\n  Then try it again\n  please.\n"
        );
    }

    #[test]
    fn text_between_turns_is_wrapped_under_the_mark() {
        let mut lines = Lines::new(true, None);
        lines.text_within("the batch finished without any error\n", 20);

        let shown: Vec<String> = lines.ready.iter().skip(1).map(|line| plain(line)).collect();
        assert_eq!(shown, ["✦ the batch finished", "  without any error"]);
    }

    fn ready(lines: &Lines) -> Vec<&str> {
        lines.ready.iter().map(String::as_str).collect()
    }

    #[test]
    fn text_between_turns_is_printed_by_whole_lines_after_a_header() {
        let mut lines = Lines::new(false, None);
        lines.text("background ");
        assert!(lines.ready.is_empty());

        lines.text("done\nall ");
        lines.text("good");
        assert_eq!(
            ready(&lines),
            ["(the agent, between turns)", "background done"]
        );

        lines.flush();
        assert_eq!(ready(&lines)[2..], ["all good"]);
        lines.flush();
        assert_eq!(lines.ready.len(), 3);
    }

    #[test]
    fn a_tool_call_between_turns_ends_the_line_being_written() {
        let mut lines = Lines::new(false, None);
        lines.text("Checking");
        lines.line("• Read log".to_string());

        assert_eq!(
            ready(&lines),
            ["(the agent, between turns)", "Checking", "• Read log"]
        );
    }

    #[test]
    fn markdown_held_back_between_turns_is_printed_when_the_agent_pauses() {
        let mut lines = Lines::new(false, Some(Markdown::default()));
        lines.text("[docs](https://exa");
        assert!(lines.ready.is_empty());

        lines.flush();
        assert_eq!(lines.ready.len(), 2, "{:?}", lines.ready);
        assert!(
            lines.ready[1].contains("[docs](https://exa"),
            "{:?}",
            lines.ready
        );
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
