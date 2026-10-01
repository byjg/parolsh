//! ACP client. The agent process and the protocol run on a background thread;
//! the input loop talks to it through two channels: commands in, events out.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, ContentChunk, CreateElicitationRequest,
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
    ElicitationFormCapabilities, ElicitationMode, InitializeRequest, NewSessionRequest,
    PermissionOption, PermissionOptionId, PermissionOptionKind, PlanEntryStatus, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
    SessionConfigSelectOptions, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TextContent, ToolCallContent,
    ToolCallStatus,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, JsonRpcMessage, JsonRpcRequest,
    LineDirection, UntypedMessage, is_incoming_transport_closed,
};
use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::config::{self, OptionValue};
use crate::form::{self, Answers, Form};
use std::collections::BTreeMap;

/// What the agent reports back to the input loop.
#[derive(Debug)]
pub enum Event {
    /// A piece of the agent's answer.
    Text(String),
    /// A piece of the agent's reasoning ("thinking"), before or between answers.
    Thought(String),
    /// The agent started a tool call.
    Tool { id: String, title: String },
    /// A tool call changed: a new title, or it finished.
    ToolUpdate {
        id: String,
        title: Option<String>,
        /// `Some(true)` when it completed, `Some(false)` when it failed.
        finished: Option<bool>,
    },
    /// The agent's plan, at its entry in progress: `step` of `total`.
    Plan {
        step: usize,
        total: usize,
        entry: String,
    },
    /// Anything else the agent sent: it is still working.
    Activity,
    /// The agent asks for permission. Reply with the chosen option, or
    /// `None` to cancel.
    Permission {
        title: String,
        /// What the permission is about: diffs, text, or the tool's input.
        details: Vec<Detail>,
        options: Vec<PermissionOption>,
        reply: oneshot::Sender<Option<PermissionOptionId>>,
    },
    /// The agent asks the user questions. Reply with the answers, or `None`
    /// to cancel.
    Form {
        form: Form,
        reply: oneshot::Sender<Option<Answers>>,
    },
    /// The prompt finished.
    TurnEnd(TurnId, StopReason),
    /// The agent answered the prompt with an error, and keeps running.
    TurnFailed(TurnId, String),
    /// Something worth telling the user, the agent keeps running.
    Notice(String),
    /// The agent stopped.
    Error(String),
}

/// The session modes the agent offers, as it reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionModes {
    pub current: String,
    pub available: Vec<String>,
}

/// A config option the agent offers (`effort`, `model`, ...), as it reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOption {
    pub id: String,
    pub name: String,
    pub current: String,
    /// The accepted values: choice ids, or `true`/`false`.
    pub values: Vec<String>,
}

impl AgentOption {
    fn from_acp(option: &SessionConfigOption) -> Option<Self> {
        let (current, values) = match &option.kind {
            SessionConfigKind::Select(select) => {
                let values = match &select.options {
                    SessionConfigSelectOptions::Ungrouped(options) => {
                        options.iter().map(|o| o.value.to_string()).collect()
                    }
                    SessionConfigSelectOptions::Grouped(groups) => groups
                        .iter()
                        .flat_map(|g| g.options.iter().map(|o| o.value.to_string()))
                        .collect(),
                    _ => Vec::new(),
                };
                (select.current_value.to_string(), values)
            }
            SessionConfigKind::Boolean(boolean) => (
                boolean.current_value.to_string(),
                vec!["true".to_string(), "false".to_string()],
            ),
            _ => return None,
        };
        Some(Self {
            id: option.id.to_string(),
            name: option.name.clone(),
            current,
            values,
        })
    }

    fn list(options: &[SessionConfigOption]) -> Vec<Self> {
        options.iter().filter_map(Self::from_acp).collect()
    }
}

/// What the agent reported about the current session.
#[derive(Debug, Default)]
struct SessionState {
    modes: Option<SessionModes>,
    options: Vec<AgentOption>,
}

type Shared = Arc<Mutex<SessionState>>;

/// The mode and options to set on every new conversation. `#options`
/// changes them for the rest of the run.
struct Wanted {
    mode: Option<String>,
    options: BTreeMap<String, OptionValue>,
}

/// Content attached to a permission request.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    Text(String),
    Diff {
        path: PathBuf,
        old: Option<String>,
        new: String,
    },
    /// The tool's raw input, when the agent attached no standard content
    /// (Qwen's questions to the user arrive this way).
    Input(serde_json::Value),
}

impl Detail {
    fn from_tool_call(
        content: Option<Vec<ToolCallContent>>,
        raw_input: Option<serde_json::Value>,
    ) -> Vec<Self> {
        let details: Vec<Self> = content
            .unwrap_or_default()
            .into_iter()
            .filter_map(|content| match content {
                ToolCallContent::Content(content) => match content.content {
                    ContentBlock::Text(text) => Some(Self::Text(text.text)),
                    _ => None,
                },
                ToolCallContent::Diff(diff) => Some(Self::Diff {
                    path: diff.path,
                    old: diff.old_text,
                    new: diff.new_text,
                }),
                _ => None,
            })
            .collect();
        if !details.is_empty() {
            return details;
        }
        raw_input
            .filter(|input| !input.is_null())
            .map(Self::Input)
            .into_iter()
            .collect()
    }
}

/// `session/request_permission`, answered with raw JSON instead of the typed
/// response: Qwen reads its users' answers from an `answers` field that
/// `RequestPermissionResponse` cannot carry.
#[derive(Debug, Clone)]
struct PermissionRequest(RequestPermissionRequest);

impl JsonRpcMessage for PermissionRequest {
    fn matches_method(method: &str) -> bool {
        RequestPermissionRequest::matches_method(method)
    }

    fn method(&self) -> &str {
        self.0.method()
    }

    fn to_untyped_message(&self) -> Result<UntypedMessage, agent_client_protocol::Error> {
        self.0.to_untyped_message()
    }

    fn parse_message(
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<Self, agent_client_protocol::Error> {
        RequestPermissionRequest::parse_message(method, params).map(Self)
    }
}

impl JsonRpcRequest for PermissionRequest {
    type Response = serde_json::Value;
}

/// Numbers the prompts, so the end of a turn Parolsh stopped waiting for is
/// told apart from the end of the next one.
pub type TurnId = u64;

enum Command {
    /// Text blocks of one message: context first, the user's text last.
    Prompt(TurnId, Vec<String>),
    Cancel,
    NewSession(PathBuf),
    SetOption(String, OptionValue),
}

/// A running agent. Dropping it stops the agent process.
pub struct AgentHandle {
    commands: Option<tokio_mpsc::UnboundedSender<Command>>,
    pub events: mpsc::Receiver<Event>,
    shared: Shared,
    thread: Option<JoinHandle<()>>,
    last_turn: Cell<TurnId>,
    /// The last turn Parolsh stopped waiting for, while the agent has not
    /// ended it yet.
    abandoned: Cell<Option<TurnId>>,
}

impl AgentHandle {
    /// Starts the agent and opens a session in `cwd`, in the background:
    /// this returns immediately, and prompts sent meanwhile wait for it.
    pub fn start(agent: &config::Agent, cwd: PathBuf) -> Self {
        let log = std::env::var_os(LOG_VAR)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        Self::spawn(agent, cwd, log)
    }

    /// `start`, logging the agent's stdio to `log`.
    fn spawn(agent: &config::Agent, cwd: PathBuf, log: Option<PathBuf>) -> Self {
        let missing = missing_command(&agent.command);
        let config = AcpAgentConfig::new(&agent.command)
            .args(agent.args.clone())
            .envs(agent.env.clone());
        let wanted = Wanted {
            mode: agent.mode.clone(),
            options: agent.options.clone(),
        };
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::channel();
        let mut acp_agent = AcpAgent::new(config);
        if let Some(path) = log {
            match open_log(&path) {
                Ok(log) => acp_agent = acp_agent.with_debug(log),
                Err(e) => {
                    let _ = events_tx.send(Event::Notice(format!(
                        "cannot write {LOG_VAR} to {}: {e}",
                        path.display()
                    )));
                }
            }
        }
        let shared = Shared::default();
        let session_state = shared.clone();

        let thread = std::thread::spawn(move || {
            if let Some(message) = missing {
                let _ = events_tx.send(Event::Error(message));
                return;
            }
            let result = tokio::runtime::Builder::new_current_thread()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(serve(
                            acp_agent,
                            wanted,
                            cwd,
                            commands_rx,
                            events_tx.clone(),
                            session_state,
                        ))
                        .map_err(|e| stop_reason(&e))
                });
            if let Err(e) = result {
                let _ = events_tx.send(Event::Error(e));
            }
        });

        Self {
            commands: Some(commands_tx),
            events: events_rx,
            shared,
            thread: Some(thread),
            last_turn: Cell::new(0),
            abandoned: Cell::new(None),
        }
    }

    /// The modes of the current session, once the agent reported them.
    /// `None` also for agents without session modes.
    pub fn modes(&self) -> Option<SessionModes> {
        self.shared
            .lock()
            .ok()
            .and_then(|state| state.modes.clone())
    }

    /// The config options of the current session, once the agent reported
    /// them. Empty for agents without options.
    pub fn options(&self) -> Vec<AgentOption> {
        self.shared
            .lock()
            .map(|state| state.options.clone())
            .unwrap_or_default()
    }

    /// Sets a config option now, and on every new conversation of this run.
    pub fn set_option(&self, id: String, value: OptionValue) -> bool {
        self.send(Command::SetOption(id, value))
    }

    /// Sends a prompt. `None` when the agent is no longer running.
    #[cfg(test)]
    pub fn prompt(&self, text: String) -> Option<TurnId> {
        self.prompt_blocks(vec![text])
    }

    /// Sends one message made of several text blocks. It waits for the
    /// previous turn, if the agent has not ended it yet.
    pub fn prompt_blocks(&self, blocks: Vec<String>) -> Option<TurnId> {
        let turn = self.last_turn.get() + 1;
        self.last_turn.set(turn);
        self.send(Command::Prompt(turn, blocks)).then_some(turn)
    }

    /// Stops waiting for `turn`: the agent did not confirm its cancel. Its
    /// events are dropped until it ends.
    pub fn abandon(&self, turn: TurnId) {
        self.abandoned.set(Some(turn));
    }

    /// The abandoned turn the agent has not ended yet.
    pub fn abandoned(&self) -> Option<TurnId> {
        self.abandoned.get()
    }

    /// Called with each turn end: false when it belongs to an abandoned turn.
    pub fn ended(&self, turn: TurnId) -> bool {
        match self.abandoned.get() {
            Some(abandoned) if turn <= abandoned => {
                if turn == abandoned {
                    self.abandoned.set(None);
                }
                false
            }
            _ => true,
        }
    }

    /// Cancels the running prompt, if any.
    pub fn cancel(&self) {
        self.send(Command::Cancel);
    }

    /// Replaces the conversation with a new one rooted in `cwd`.
    pub fn new_session(&self, cwd: PathBuf) -> bool {
        self.send(Command::NewSession(cwd))
    }

    fn send(&self, command: Command) -> bool {
        self.commands
            .as_ref()
            .is_some_and(|commands| commands.send(command).is_ok())
    }
}

impl Drop for AgentHandle {
    fn drop(&mut self) {
        // Closing the command channel ends the session loop, which closes the
        // connection and kills the agent's process group.
        self.commands = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why the agent stopped. The crate's errors carry the reason (the exit status
/// and the end of the agent's stderr) as text in `data`, next to where in the
/// crate they were raised: only that text is shown.
fn stop_reason(error: &agent_client_protocol::Error) -> String {
    if is_incoming_transport_closed(error) {
        return "the agent closed the connection".to_string();
    }
    let text = match &error.data {
        Some(serde_json::Value::String(text)) => text,
        Some(data) => match data.get("data") {
            Some(serde_json::Value::String(text)) => text,
            _ => return error.to_string(),
        },
        None => return error.to_string(),
    };
    text.trim_end().to_string()
}

/// Names the file that gets the agent's stdio, for diagnostics.
const LOG_VAR: &str = "PAROLSH_ACP_LOG";

/// Opens the log for appending, and returns the callback that writes each
/// line sent to the agent, received from it and printed on its stderr, after
/// the seconds since it started. The log holds the whole conversation, so it
/// is created readable by the user only.
fn open_log(path: &Path) -> std::io::Result<impl Fn(&str, LineDirection) + Send + Sync + 'static> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "--- agent started")?;
    let file = Mutex::new(file);
    let started = std::time::Instant::now();
    Ok(move |line: &str, direction: LineDirection| {
        let direction = match direction {
            LineDirection::Stdin => "send",
            LineDirection::Stdout => "recv",
            LineDirection::Stderr => "stderr",
        };
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(
                file,
                "{:10.3} {direction:<6} {line}",
                started.elapsed().as_secs_f64()
            );
        }
    })
}

/// Why `command` cannot be started, or `None` when it can be found.
fn missing_command(command: &str) -> Option<String> {
    let path = std::env::var("PATH").ok();
    if crate::shellenv::find_command(command, path.as_deref()).is_some() {
        return None;
    }
    let place = if command.contains('/') {
        "no executable file at that path"
    } else {
        "not found on PATH"
    };
    Some(format!(
        "cannot start `{command}`: {place}. Agents installed with npm under nvm are only on \
         the PATH your ~/.bashrc sets up: start Parolsh from a terminal, or set \
         shell_env = \"always\" in the configuration (see #config)."
    ))
}

async fn serve(
    agent: AcpAgent,
    mut wanted: Wanted,
    cwd: PathBuf,
    mut commands: tokio_mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<Event>,
    shared: Shared,
) -> Result<(), agent_client_protocol::Error> {
    let update_events = events.clone();
    let permission_events = events.clone();
    let form_events = events.clone();
    let updated_state = shared.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                forward(&update_events, &updated_state, notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |PermissionRequest(request): PermissionRequest, responder, _cx| {
                let response = match Form::from_qwen(request.tool_call.meta.as_ref()) {
                    Some(form) => answer_qwen(&permission_events, form, &request.options).await,
                    None => ask_permission(&permission_events, request).await,
                };
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CreateElicitationRequest, responder, _cx| {
                let action = match request.mode {
                    ElicitationMode::Form(mode) => {
                        let form = Form::from_elicitation(request.message, &mode.requested_schema);
                        match ask_form(&form_events, form).await {
                            None => ElicitationAction::Cancel,
                            Some(answers) if answers.is_empty() => ElicitationAction::Decline,
                            Some(answers) => ElicitationAction::Accept(
                                ElicitationAcceptAction::new()
                                    .content(form::elicitation_content(answers)),
                            ),
                        }
                    }
                    // Only forms are advertised.
                    _ => ElicitationAction::Decline,
                };
                responder.respond(CreateElicitationResponse::new(action))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            // Forms let agents ask the user questions (elicitation).
            let capabilities = ClientCapabilities::new().elicitation(
                ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
            );
            cx.send_request(
                InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities),
            )
            .block_task()
            .await?;
            let mut session = open_session(&cx, &cwd, &wanted, &events, &shared).await?;
            // Commands that arrived during a turn, run after it.
            let mut pending = VecDeque::new();

            loop {
                let command = match pending.pop_front() {
                    Some(command) => command,
                    None => match commands.recv().await {
                        Some(command) => command,
                        None => break,
                    },
                };
                match command {
                    Command::Prompt(turn, blocks) => {
                        let result =
                            prompt(&cx, &session, blocks, &mut commands, &mut pending, &events)
                                .await;
                        let event = match result {
                            Ok(stop_reason) => Event::TurnEnd(turn, stop_reason),
                            // The agent's output closed: it stopped, the
                            // turn did not fail.
                            Err(e) if is_incoming_transport_closed(&e) => return Err(e),
                            Err(e) => Event::TurnFailed(turn, e.to_string()),
                        };
                        let _ = events.send(event);
                    }
                    Command::NewSession(dir) => {
                        session = open_session(&cx, &dir, &wanted, &events, &shared).await?;
                    }
                    Command::SetOption(id, value) => {
                        wanted.options.insert(id.clone(), value.clone());
                        set_option(&cx, &session, &id, &value, &events, &shared).await;
                    }
                    Command::Cancel => {}
                }
            }
            Ok(())
        })
        .await
}

/// Asks the user through the input loop, and answers the permission request.
async fn ask_permission(
    events: &mpsc::Sender<Event>,
    request: RequestPermissionRequest,
) -> serde_json::Value {
    let (reply, answer) = oneshot::channel();
    let fields = request.tool_call.fields;
    let event = Event::Permission {
        title: fields.title.unwrap_or_default(),
        details: Detail::from_tool_call(fields.content, fields.raw_input),
        options: request.options,
        reply,
    };
    let outcome = match events.send(event) {
        Ok(()) => answer.await.ok().flatten(),
        Err(_) => None,
    };
    let outcome = match outcome {
        Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
        None => RequestPermissionOutcome::Cancelled,
    };
    serde_json::to_value(RequestPermissionResponse::new(outcome)).unwrap_or_default()
}

/// Qwen's questions: submits the answers in the `answers` field Qwen reads,
/// or cancels when the user answered nothing.
async fn answer_qwen(
    events: &mpsc::Sender<Event>,
    form: Form,
    options: &[PermissionOption],
) -> serde_json::Value {
    let submit = options
        .iter()
        .find(|option| option.kind == PermissionOptionKind::AllowOnce)
        .or(options.first());
    match (ask_form(events, form).await, submit) {
        (Some(answers), Some(submit)) if !answers.is_empty() => serde_json::json!({
            "outcome": {"outcome": "selected", "optionId": submit.option_id.to_string()},
            "answers": form::qwen_answers(answers),
        }),
        _ => serde_json::json!({"outcome": {"outcome": "cancelled"}}),
    }
}

async fn ask_form(events: &mpsc::Sender<Event>, form: Form) -> Option<Answers> {
    let (reply, answers) = oneshot::channel();
    events.send(Event::Form { form, reply }).ok()?;
    answers.await.ok().flatten()
}

/// Runs one prompt turn. A `Cancel` received meanwhile is sent to the agent,
/// which then ends the turn with `StopReason::Cancelled`. Other commands are
/// kept in `pending`, to run after the turn; a new prompt means Parolsh
/// stopped waiting for this one, so it cancels it again, and a `Cancel`
/// drops the prompts waiting.
async fn prompt(
    cx: &ConnectionTo<Agent>,
    session: &SessionId,
    blocks: Vec<String>,
    commands: &mut tokio_mpsc::UnboundedReceiver<Command>,
    pending: &mut VecDeque<Command>,
    events: &mpsc::Sender<Event>,
) -> Result<StopReason, agent_client_protocol::Error> {
    let blocks = blocks
        .into_iter()
        .map(|text| ContentBlock::Text(TextContent::new(text)))
        .collect();
    let turn = cx
        .send_request(PromptRequest::new(session.clone(), blocks))
        .block_task();
    tokio::pin!(turn);

    loop {
        tokio::select! {
            response = &mut turn => return Ok(response?.stop_reason),
            Some(command) = commands.recv() => match command {
                Command::Cancel => {
                    pending.retain(|command| match command {
                        Command::Prompt(turn, _) => {
                            let _ = events.send(Event::TurnEnd(*turn, StopReason::Cancelled));
                            false
                        }
                        _ => true,
                    });
                    cx.send_notification(CancelNotification::new(session.clone()))?;
                }
                Command::Prompt(..) => {
                    pending.push_back(command);
                    cx.send_notification(CancelNotification::new(session.clone()))?;
                }
                command => pending.push_back(command),
            },
        }
    }
}

/// Creates a session, then sets the wanted mode and options.
async fn open_session(
    cx: &ConnectionTo<Agent>,
    cwd: &Path,
    wanted: &Wanted,
    events: &mpsc::Sender<Event>,
    shared: &Shared,
) -> Result<SessionId, agent_client_protocol::Error> {
    let response = cx
        .send_request(NewSessionRequest::new(cwd))
        .block_task()
        .await?;
    let session = response.session_id.clone();

    let mut modes = response.modes.as_ref().map(|modes| SessionModes {
        current: modes.current_mode_id.to_string(),
        available: modes
            .available_modes
            .iter()
            .map(|mode| mode.id.to_string())
            .collect(),
    });
    let options = AgentOption::list(response.config_options.as_deref().unwrap_or_default());
    let mode_option = options.iter().any(|option| option.id == "mode");

    match (wanted.mode.as_deref(), &mut modes) {
        (None, _) => {}
        // No session modes, but a `mode` config option (Kilo): set it below.
        (Some(_), None) if mode_option => {}
        (Some(mode), None) => {
            let _ = events.send(Event::Notice(format!(
                "the agent has no session modes, `mode = \"{mode}\"` is ignored"
            )));
        }
        (Some(mode), Some(modes)) if modes.current == mode => {}
        (Some(mode), Some(modes)) if modes.available.iter().any(|m| m == mode) => {
            cx.send_request(SetSessionModeRequest::new(
                session.clone(),
                mode.to_string(),
            ))
            .block_task()
            .await?;
            modes.current = mode.to_string();
        }
        (Some(mode), Some(modes)) => {
            let _ = events.send(Event::Notice(format!(
                "the agent has no `{mode}` mode (it offers {}), it stays in `{}`",
                modes.available.join(", "),
                modes.current
            )));
        }
    }

    if let Ok(mut state) = shared.lock() {
        *state = SessionState { modes, options };
    }

    if let Some(mode) = &wanted.mode
        && mode_option
        && response.modes.is_none()
    {
        let value = OptionValue::Text(mode.clone());
        set_option(cx, &session, "mode", &value, events, shared).await;
    }
    for (id, value) in &wanted.options {
        set_option(cx, &session, id, value, events, shared).await;
    }
    Ok(session)
}

/// Sets one config option, after checking the agent offers it. Problems are
/// notices: a wrong option never ends the session.
async fn set_option(
    cx: &ConnectionTo<Agent>,
    session: &SessionId,
    id: &str,
    value: &OptionValue,
    events: &mpsc::Sender<Event>,
    shared: &Shared,
) {
    let options = shared
        .lock()
        .map(|state| state.options.clone())
        .unwrap_or_default();
    let notice = |message: String| {
        let _ = events.send(Event::Notice(message));
    };
    let Some(option) = options.iter().find(|option| option.id == id) else {
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        return notice(if ids.is_empty() {
            format!("the agent has no options, `{id}` is ignored")
        } else {
            format!(
                "the agent has no `{id}` option (it offers {})",
                ids.join(", ")
            )
        });
    };
    let text = value.to_string();
    if option.current == text {
        return;
    }
    if !option.values.contains(&text) {
        return notice(format!(
            "`{id}` has no value `{text}` (it offers {}), it stays `{}`",
            option.values.join(", "),
            option.current
        ));
    }
    let value = match value {
        OptionValue::Bool(value) => SessionConfigOptionValue::boolean(*value),
        OptionValue::Text(value) => SessionConfigOptionValue::value_id(value.clone()),
    };
    let request = SetSessionConfigOptionRequest::new(session.clone(), id.to_string(), value);
    match cx.send_request(request).block_task().await {
        Ok(response) => {
            if let Ok(mut state) = shared.lock() {
                state.options = AgentOption::list(&response.config_options);
            }
        }
        Err(e) => notice(format!("the agent refused `{id} = {text}`: {e}")),
    }
}

fn forward(events: &mpsc::Sender<Event>, shared: &Shared, update: SessionUpdate) {
    let event = match update {
        SessionUpdate::CurrentModeUpdate(update) => {
            if let Ok(mut state) = shared.lock()
                && let Some(modes) = state.modes.as_mut()
            {
                modes.current = update.current_mode_id.to_string();
            }
            return;
        }
        SessionUpdate::ConfigOptionUpdate(update) => {
            if let Ok(mut state) = shared.lock() {
                state.options = AgentOption::list(&update.config_options);
            }
            return;
        }
        SessionUpdate::AgentMessageChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) => Event::Text(text.text),
        SessionUpdate::AgentThoughtChunk(ContentChunk {
            content: ContentBlock::Text(text),
            ..
        }) => Event::Thought(text.text),
        SessionUpdate::ToolCall(call) => Event::Tool {
            id: call.tool_call_id.to_string(),
            title: call.title,
        },
        SessionUpdate::ToolCallUpdate(update) => Event::ToolUpdate {
            id: update.tool_call_id.to_string(),
            title: update.fields.title,
            finished: match update.fields.status {
                Some(ToolCallStatus::Completed) => Some(true),
                Some(ToolCallStatus::Failed) => Some(false),
                _ => None,
            },
        },
        SessionUpdate::Plan(plan) => {
            let total = plan.entries.len();
            match plan
                .entries
                .into_iter()
                .enumerate()
                .find(|(_, entry)| entry.status == PlanEntryStatus::InProgress)
            {
                Some((i, entry)) => Event::Plan {
                    step: i + 1,
                    total,
                    entry: entry.content,
                },
                None => Event::Activity,
            }
        }
        _ => Event::Activity,
    };
    let _ = events.send(event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const FAKE_AGENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_agent.py");

    fn start(mode: Option<&str>, extra: &[&str], cwd: &Path) -> AgentHandle {
        let mut args = vec![FAKE_AGENT.to_string()];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        AgentHandle::start(&fake_agent(mode, args), cwd.to_path_buf())
    }

    fn fake_agent(mode: Option<&str>, args: Vec<String>) -> config::Agent {
        config::Agent {
            command: "python3".to_string(),
            args,
            env: Default::default(),
            mode: mode.map(str::to_string),
            options: Default::default(),
        }
    }

    fn next(agent: &AgentHandle) -> Event {
        agent
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("no event from the agent")
    }

    /// Text of the turn, with tool calls as `• title` lines, until it ends.
    fn turn(agent: &AgentHandle, prompt: &str) -> (String, StopReason) {
        assert!(agent.prompt(prompt.to_string()).is_some());
        let mut text = String::new();
        loop {
            match next(agent) {
                Event::Text(chunk) => text.push_str(&chunk),
                Event::Tool { title, .. } => text.push_str(&format!("• {title}\n")),
                Event::TurnEnd(_, reason) => return (text, reason),
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    #[test]
    fn streams_the_answer_in_the_session_directory_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(Some("auto"), &[], dir.path());

        let (text, reason) = turn(&agent, "hello");

        let expected = format!("• Read README.md\n[s1|auto|{}] hello", dir.path().display());
        assert_eq!(text, expected);
        assert_eq!(reason, StopReason::EndTurn);
    }

    #[test]
    fn the_configured_mode_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(Some("plan"), &[], dir.path());

        let (text, _) = turn(&agent, "hi");

        assert!(text.contains("[s1|plan|"), "{text}");
    }

    #[test]
    fn a_missing_mode_is_reported_and_the_session_keeps_working() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(Some("auto"), &["--no-auto"], dir.path());

        match next(&agent) {
            Event::Notice(message) => {
                assert_eq!(
                    message,
                    "the agent has no `auto` mode (it offers default, plan), it stays in `default`"
                )
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let (text, _) = turn(&agent, "hi");
        assert!(text.contains("[s1|default|"), "{text}");
    }

    #[test]
    fn permission_requests_are_answered_with_the_chosen_option() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("perm".to_string()).is_some());

        match next(&agent) {
            Event::Permission {
                title,
                details,
                options,
                reply,
            } => {
                assert_eq!(title, "Writing to notes.txt");
                assert_eq!(
                    details,
                    [Detail::Diff {
                        path: "/tmp/notes.txt".into(),
                        old: Some("a\n".to_string()),
                        new: "a\nb\n".to_string(),
                    }]
                );
                assert_eq!(options.len(), 2);
                reply.send(Some(options[0].option_id.clone())).unwrap();
            }
            other => panic!("unexpected event: {other:?}"),
        }

        match next(&agent) {
            Event::Text(text) => assert_eq!(text, "chose:allow-once"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn raw_input_is_the_detail_when_there_is_no_content() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("ask".to_string()).is_some());

        match next(&agent) {
            Event::Permission { details, reply, .. } => {
                assert_eq!(
                    details,
                    [Detail::Input(serde_json::json!({
                        "questions": [{"question": "Which color?"}]
                    }))]
                );
                drop(reply);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_arrives_as_thoughts_before_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("think".to_string()).is_some());

        let mut thoughts = String::new();
        loop {
            match next(&agent) {
                Event::Thought(text) => thoughts.push_str(&text),
                Event::Text(text) => {
                    assert_eq!(text, "done");
                    break;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(thoughts, "Let me think.\nAlmost there");
    }

    /// Sends `prompt`, answers the form it triggers with `answers`, and
    /// returns what the agent says it received.
    fn answer_form(prompt: &str, answers: Option<form::Answers>) -> (Form, String) {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt(prompt.to_string()).is_some());
        let form = match next(&agent) {
            Event::Form { form, reply } => {
                reply.send(answers).unwrap();
                form
            }
            other => panic!("unexpected event: {other:?}"),
        };
        match next(&agent) {
            Event::Text(text) => (form, text),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn forms_are_advertised_to_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());

        let (text, _) = turn(&agent, "caps");

        assert_eq!(text, r#"{"form": {}}"#);
    }

    #[test]
    fn an_elicitation_form_is_accepted_with_the_answers() {
        let answers =
            form::Answers::from([("color".to_string(), form::Answer::Text("blue".to_string()))]);

        let (form, result) = answer_form("form", Some(answers));

        assert_eq!(form.message, "Pick a color");
        assert_eq!(form.fields[0].title, "Color");
        assert_eq!(
            result,
            r#"{"action": "accept", "content": {"color": "blue"}}"#
        );
    }

    #[test]
    fn an_elicitation_form_without_answers_is_declined_or_cancelled() {
        let (_, declined) = answer_form("form", Some(form::Answers::new()));
        let (_, cancelled) = answer_form("form", None);

        assert_eq!(declined, r#"{"action": "decline"}"#);
        assert_eq!(cancelled, r#"{"action": "cancel"}"#);
    }

    #[test]
    fn qwen_questions_are_answered_in_its_answers_field() {
        let answers =
            form::Answers::from([("0".to_string(), form::Answer::Text("Blue".to_string()))]);

        let (form, result) = answer_form("qwen", Some(answers));

        assert_eq!(form.fields[0].title, "Which color?");
        assert_eq!(
            result,
            r#"{"answers": {"0": "Blue"}, "outcome": {"optionId": "proceed_once", "outcome": "selected"}}"#
        );
    }

    #[test]
    fn unanswered_qwen_questions_are_cancelled() {
        let (_, result) = answer_form("qwen", Some(form::Answers::new()));

        assert_eq!(result, r#"{"outcome": {"outcome": "cancelled"}}"#);
    }

    #[test]
    fn a_message_can_carry_several_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(
            agent
                .prompt_blocks(vec![
                    "output one".to_string(),
                    "output two".to_string(),
                    "blocks".to_string(),
                ])
                .is_some()
        );

        match next(&agent) {
            Event::Text(text) => assert_eq!(text, r#"["output one", "output two"]"#),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn an_unanswered_permission_request_is_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("perm".to_string()).is_some());

        match next(&agent) {
            Event::Permission { reply, .. } => drop(reply),
            other => panic!("unexpected event: {other:?}"),
        }

        match next(&agent) {
            Event::Text(text) => assert_eq!(text, "chose:cancelled"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn cancel_ends_the_running_turn() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("slow".to_string()).is_some());

        match next(&agent) {
            Event::Text(text) => assert_eq!(text, "working"),
            other => panic!("unexpected event: {other:?}"),
        }
        agent.cancel();

        match next(&agent) {
            Event::TurnEnd(_, reason) => assert_eq!(reason, StopReason::Cancelled),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    fn turn_end(agent: &AgentHandle) -> (TurnId, StopReason) {
        match next(agent) {
            Event::TurnEnd(turn, reason) => (turn, reason),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    /// A prompt sent while the agent still runs a turn Parolsh gave up on
    /// cancels that turn again, and runs once it ends: it is not lost.
    #[test]
    fn a_prompt_waits_for_the_turn_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        let stuck = agent.prompt("stuck".to_string()).unwrap();
        assert!(matches!(next(&agent), Event::Text(text) if text == "working"));
        agent.cancel(); // ignored by the agent

        let hello = agent.prompt("hello".to_string()).unwrap();

        assert_eq!(turn_end(&agent), (stuck, StopReason::Cancelled));
        let mut text = String::new();
        let end = loop {
            match next(&agent) {
                Event::Text(chunk) => text.push_str(&chunk),
                Event::Tool { .. } => {}
                Event::TurnEnd(turn, reason) => break (turn, reason),
                other => panic!("unexpected event: {other:?}"),
            }
        };
        assert_eq!(end, (hello, StopReason::EndTurn));
        assert!(text.ends_with("hello"), "{text}");
    }

    /// Cancel while a prompt waits for the turn before it: the waiting
    /// prompt is dropped at once, and never reaches the agent.
    #[test]
    fn cancel_drops_the_waiting_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        let stuck = agent.prompt("stuck".to_string()).unwrap();
        assert!(matches!(next(&agent), Event::Text(text) if text == "working"));
        let waiting = agent.prompt("hello".to_string()).unwrap();

        agent.cancel();

        assert_eq!(turn_end(&agent), (waiting, StopReason::Cancelled));
        assert_eq!(turn_end(&agent), (stuck, StopReason::Cancelled));
        let (text, _) = turn(&agent, "again");
        assert!(text.ends_with("again"), "{text}");
    }

    #[test]
    fn the_log_has_what_was_sent_received_and_printed_on_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("acp.log");
        let agent = AgentHandle::spawn(
            &fake_agent(None, vec![FAKE_AGENT.to_string()]),
            dir.path().to_path_buf(),
            Some(log.clone()),
        );
        let (text, _) = turn(&agent, "warn");
        assert_eq!(text, "warned");
        drop(agent);

        let text = std::fs::read_to_string(&log).unwrap();
        let has = |direction: &str, part: &str| {
            text.lines()
                .any(|line| line.contains(&format!(" {direction} ")) && line.contains(part))
        };
        assert!(text.starts_with("--- agent started\n"), "{text}");
        assert!(has("send", r#""method":"session/prompt""#), "{text}");
        assert!(has("recv", "warned"), "{text}");
        assert!(has("stderr", "fake warning"), "{text}");
        let mode = std::fs::metadata(&log).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn tool_updates_plans_and_other_updates_are_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("tools".to_string()).is_some());

        let mut events = Vec::new();
        loop {
            match next(&agent) {
                Event::TurnEnd(..) => break,
                event => events.push(format!("{event:?}")),
            }
        }

        assert_eq!(
            events,
            [
                r#"Tool { id: "t1", title: "Read a" }"#,
                r#"Tool { id: "t2", title: "Read b" }"#,
                r#"ToolUpdate { id: "t1", title: Some("Read a.rs"), finished: None }"#,
                r#"ToolUpdate { id: "t1", title: None, finished: Some(true) }"#,
                r#"ToolUpdate { id: "t2", title: None, finished: Some(false) }"#,
                r#"Plan { step: 2, total: 3, entry: "Write tests" }"#,
                "Activity",
                r#"Text("done")"#,
            ]
        );
    }

    #[test]
    fn the_end_of_an_abandoned_turn_is_not_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());

        agent.abandon(3);

        assert!(!agent.ended(2));
        assert_eq!(agent.abandoned(), Some(3));
        assert!(!agent.ended(3));
        assert_eq!(agent.abandoned(), None);
        assert!(agent.ended(4));
    }

    #[test]
    fn an_error_answer_fails_the_turn_and_the_session_keeps_working() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("fail".to_string()).is_some());

        match next(&agent) {
            Event::TurnFailed(_, message) => {
                assert!(message.contains("loop protection"), "{message}")
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let (text, _) = turn(&agent, "again");

        assert!(text.contains("[s1|default|"), "{text}");
    }

    #[test]
    fn an_agent_that_exits_during_a_turn_stops() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("die".to_string()).is_some());

        // Whichever the crate notices first: the exit, or the closed output.
        match next(&agent) {
            Event::Error(message) => assert!(
                message == "Process exited with exit status: 1: boom"
                    || message == "the agent closed the connection",
                "{message}"
            ),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn new_session_starts_over_in_another_directory() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let agent = start(None, &[], first.path());
        turn(&agent, "one");

        assert!(agent.new_session(second.path().to_path_buf()));
        let (text, _) = turn(&agent, "two");

        let expected = format!("[s2|default|{}] two", second.path().display());
        assert!(text.ends_with(&expected), "{text}");
    }

    #[test]
    fn env_reaches_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let mut agent = fake_agent(None, vec![FAKE_AGENT.to_string()]);
        agent
            .env
            .insert("PAROLSH_TEST_MODEL".to_string(), "gpt-test".to_string());
        let agent = AgentHandle::start(&agent, dir.path().to_path_buf());

        let (text, _) = turn(&agent, "env PAROLSH_TEST_MODEL");

        assert_eq!(text, "gpt-test");
    }

    #[test]
    fn without_a_mode_the_agent_keeps_its_default() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());

        let (text, _) = turn(&agent, "hi");

        assert!(text.contains("[s1|default|"), "{text}");
        assert_eq!(
            agent.modes(),
            Some(SessionModes {
                current: "default".to_string(),
                available: vec!["default".into(), "plan".into(), "auto".into()],
            })
        );
    }

    fn start_with_options(
        mode: Option<&str>,
        options: &[(&str, OptionValue)],
        extra: &[&str],
        cwd: &Path,
    ) -> AgentHandle {
        let mut args = vec![FAKE_AGENT.to_string()];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        let mut agent = fake_agent(mode, args);
        agent.options = options
            .iter()
            .map(|(id, value)| (id.to_string(), value.clone()))
            .collect();
        AgentHandle::start(&agent, cwd.to_path_buf())
    }

    fn text(value: &str) -> OptionValue {
        OptionValue::Text(value.to_string())
    }

    #[test]
    fn configured_options_are_set_on_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start_with_options(
            None,
            &[("effort", text("low")), ("fast", OptionValue::Bool(true))],
            &[],
            dir.path(),
        );

        let (reply, _) = turn(&agent, "opts");

        assert_eq!(
            reply,
            r#"{"effort": "low", "fast": true, "mode": "default", "model": "one"}"#
        );
        let effort = agent
            .options()
            .into_iter()
            .find(|o| o.id == "effort")
            .unwrap();
        assert_eq!(effort.current, "low");
        assert_eq!(effort.values, ["low", "high"]);
    }

    #[test]
    fn unknown_options_and_values_are_notices() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start_with_options(
            None,
            &[("speed", text("max")), ("model", text("three"))],
            &[],
            dir.path(),
        );

        let mut notices = Vec::new();
        for _ in 0..2 {
            match next(&agent) {
                Event::Notice(message) => notices.push(message),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        notices.sort();

        assert_eq!(
            notices,
            [
                "`model` has no value `three` (it offers one, two), it stays `one`",
                "the agent has no `speed` option (it offers effort, model, fast)",
            ]
        );
        let (reply, _) = turn(&agent, "opts");
        assert!(reply.contains(r#""model": "one""#), "{reply}");
    }

    #[test]
    fn an_option_set_during_the_run_applies_now_and_to_new_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start_with_options(None, &[], &[], dir.path());
        turn(&agent, "hi");

        assert!(agent.set_option("model".to_string(), text("two")));
        let (reply, _) = turn(&agent, "opts");
        assert!(reply.contains(r#""model": "two""#), "{reply}");

        assert!(agent.new_session(dir.path().to_path_buf()));
        let (reply, _) = turn(&agent, "opts");
        assert!(reply.contains(r#""model": "two""#), "{reply}");
    }

    #[test]
    fn option_changes_by_the_agent_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start_with_options(None, &[("effort", text("low"))], &[], dir.path());

        turn(&agent, "bump");

        let effort = agent
            .options()
            .into_iter()
            .find(|o| o.id == "effort")
            .unwrap();
        assert_eq!(effort.current, "high");
    }

    #[test]
    fn mode_is_set_through_a_mode_option_when_there_are_no_session_modes() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start_with_options(Some("ask"), &[], &["--kilo"], dir.path());

        let (reply, _) = turn(&agent, "opts");

        assert!(reply.contains(r#""mode": "ask""#), "{reply}");
        assert!(agent.modes().is_none());
    }

    #[test]
    fn an_agent_that_cannot_start_reports_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let agent = AgentHandle::start(
            &config::Agent {
                command: "/nonexistent/agent".to_string(),
                ..fake_agent(None, vec![])
            },
            dir.path().to_path_buf(),
        );

        match next(&agent) {
            Event::Error(message) => assert!(
                message.starts_with(
                    "cannot start `/nonexistent/agent`: no executable file at that path."
                ),
                "{message}"
            ),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
