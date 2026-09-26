//! ACP client. The agent process and the protocol run on a background thread;
//! the input loop talks to it through two channels: commands in, events out.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, ContentChunk, CreateElicitationRequest,
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
    ElicitationFormCapabilities, ElicitationMode, InitializeRequest, NewSessionRequest,
    PermissionOption, PermissionOptionId, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, StopReason, TextContent, ToolCallContent,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, ConnectionTo, JsonRpcMessage, JsonRpcRequest,
    UntypedMessage,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::config;
use crate::form::{self, Answers, Form};

/// What the agent reports back to the input loop.
#[derive(Debug)]
pub enum Event {
    /// A piece of the agent's answer.
    Text(String),
    /// A piece of the agent's reasoning ("thinking"), before or between answers.
    Thought(String),
    /// The agent started a tool call.
    Tool(String),
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
    TurnEnd(StopReason),
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

type SharedModes = Arc<Mutex<Option<SessionModes>>>;

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

enum Command {
    Prompt(String),
    Cancel,
    NewSession(PathBuf),
}

/// A running agent. Dropping it stops the agent process.
pub struct AgentHandle {
    commands: Option<tokio_mpsc::UnboundedSender<Command>>,
    pub events: mpsc::Receiver<Event>,
    modes: SharedModes,
    thread: Option<JoinHandle<()>>,
}

impl AgentHandle {
    /// Starts the agent and opens a session in `cwd`, in the background:
    /// this returns immediately, and prompts sent meanwhile wait for it.
    pub fn start(agent: &config::Agent, cwd: PathBuf) -> Self {
        let config = AcpAgentConfig::new(&agent.command)
            .args(agent.args.clone())
            .envs(agent.env.clone());
        let mode = agent.mode.clone();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::channel();
        let modes = SharedModes::default();
        let session_modes = modes.clone();

        let thread = std::thread::spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .build()
                .map_err(|e| e.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(serve(
                            config,
                            mode,
                            cwd,
                            commands_rx,
                            events_tx.clone(),
                            session_modes,
                        ))
                        .map_err(|e| e.to_string())
                });
            if let Err(e) = result {
                let _ = events_tx.send(Event::Error(e));
            }
        });

        Self {
            commands: Some(commands_tx),
            events: events_rx,
            modes,
            thread: Some(thread),
        }
    }

    /// The modes of the current session, once the agent reported them.
    /// `None` also for agents without session modes.
    pub fn modes(&self) -> Option<SessionModes> {
        self.modes.lock().ok().and_then(|modes| modes.clone())
    }

    /// Sends a prompt. False when the agent is no longer running.
    pub fn prompt(&self, text: String) -> bool {
        self.send(Command::Prompt(text))
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

async fn serve(
    config: AcpAgentConfig,
    mode: Option<String>,
    cwd: PathBuf,
    mut commands: tokio_mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<Event>,
    modes: SharedModes,
) -> Result<(), agent_client_protocol::Error> {
    let update_events = events.clone();
    let permission_events = events.clone();
    let form_events = events.clone();
    let updated_modes = modes.clone();

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                forward(&update_events, &updated_modes, notification.update);
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
        .connect_with(
            AcpAgent::new(config),
            async move |cx: ConnectionTo<Agent>| {
                // Forms let agents ask the user questions (elicitation).
                let capabilities = ClientCapabilities::new().elicitation(
                    ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
                );
                cx.send_request(
                    InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities),
                )
                .block_task()
                .await?;
                let mut session = open_session(&cx, &cwd, mode.as_deref(), &events, &modes).await?;

                while let Some(command) = commands.recv().await {
                    match command {
                        Command::Prompt(text) => {
                            let stop_reason = prompt(&cx, &session, text, &mut commands).await?;
                            let _ = events.send(Event::TurnEnd(stop_reason));
                        }
                        Command::NewSession(dir) => {
                            session =
                                open_session(&cx, &dir, mode.as_deref(), &events, &modes).await?;
                        }
                        Command::Cancel => {}
                    }
                }
                Ok(())
            },
        )
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
/// which then ends the turn with `StopReason::Cancelled`.
async fn prompt(
    cx: &ConnectionTo<Agent>,
    session: &SessionId,
    text: String,
    commands: &mut tokio_mpsc::UnboundedReceiver<Command>,
) -> Result<StopReason, agent_client_protocol::Error> {
    let turn = cx
        .send_request(PromptRequest::new(
            session.clone(),
            vec![ContentBlock::Text(TextContent::new(text))],
        ))
        .block_task();
    tokio::pin!(turn);

    loop {
        tokio::select! {
            response = &mut turn => return Ok(response?.stop_reason),
            Some(command) = commands.recv() => {
                if let Command::Cancel = command {
                    cx.send_notification(CancelNotification::new(session.clone()))?;
                }
            }
        }
    }
}

/// Creates a session and, when `mode` is set, switches the agent to it.
async fn open_session(
    cx: &ConnectionTo<Agent>,
    cwd: &Path,
    mode: Option<&str>,
    events: &mpsc::Sender<Event>,
    shared: &SharedModes,
) -> Result<SessionId, agent_client_protocol::Error> {
    let response = cx
        .send_request(NewSessionRequest::new(cwd))
        .block_task()
        .await?;

    let mut modes = response.modes.as_ref().map(|modes| SessionModes {
        current: modes.current_mode_id.to_string(),
        available: modes
            .available_modes
            .iter()
            .map(|mode| mode.id.to_string())
            .collect(),
    });

    match (mode, &mut modes) {
        (None, _) => {}
        (Some(mode), None) => {
            let _ = events.send(Event::Notice(format!(
                "the agent has no session modes, `mode = \"{mode}\"` is ignored"
            )));
        }
        (Some(mode), Some(modes)) if modes.current == mode => {}
        (Some(mode), Some(modes)) if modes.available.iter().any(|m| m == mode) => {
            cx.send_request(SetSessionModeRequest::new(
                response.session_id.clone(),
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

    if let Ok(mut shared) = shared.lock() {
        *shared = modes;
    }
    Ok(response.session_id)
}

fn forward(events: &mpsc::Sender<Event>, modes: &SharedModes, update: SessionUpdate) {
    let event = match update {
        SessionUpdate::CurrentModeUpdate(update) => {
            if let Ok(mut modes) = modes.lock()
                && let Some(modes) = modes.as_mut()
            {
                modes.current = update.current_mode_id.to_string();
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
        SessionUpdate::ToolCall(call) => Event::Tool(call.title),
        _ => return,
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
        assert!(agent.prompt(prompt.to_string()));
        let mut text = String::new();
        loop {
            match next(agent) {
                Event::Text(chunk) => text.push_str(&chunk),
                Event::Tool(title) => text.push_str(&format!("• {title}\n")),
                Event::TurnEnd(reason) => return (text, reason),
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
        assert!(agent.prompt("perm".to_string()));

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
        assert!(agent.prompt("ask".to_string()));

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
        assert!(agent.prompt("think".to_string()));

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
        assert!(agent.prompt(prompt.to_string()));
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
    fn an_unanswered_permission_request_is_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let agent = start(None, &[], dir.path());
        assert!(agent.prompt("perm".to_string()));

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
        assert!(agent.prompt("slow".to_string()));

        match next(&agent) {
            Event::Text(text) => assert_eq!(text, "working"),
            other => panic!("unexpected event: {other:?}"),
        }
        agent.cancel();

        match next(&agent) {
            Event::TurnEnd(reason) => assert_eq!(reason, StopReason::Cancelled),
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

        assert!(matches!(next(&agent), Event::Error(_)));
    }
}
