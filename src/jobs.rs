//! Turns sent to the background with Ctrl+Z. Each keeps the agent process
//! it runs in, and its conversation; the foreground goes on with another
//! process, in a copy of the conversation when the agent can make one.

use crate::acp::{AgentHandle, Event};
use crate::audit::{self, Audit};
use crate::turn::{self, Ended, Polled, Turn};
use crate::ui;

pub struct Job {
    /// What `#jobs` and `#fg` call it.
    pub number: usize,
    /// The message that started it.
    pub request: String,
    /// The conversation it was sent from, numbered by Parolsh: its answer
    /// only goes back there.
    pub conversation: u64,
    agent: AgentHandle,
    turn: Turn,
    asking: Option<Event>,
    ended: Option<Ended>,
}

impl Job {
    pub fn new(
        number: usize,
        request: String,
        conversation: u64,
        agent: AgentHandle,
        turn: Turn,
    ) -> Self {
        Self {
            number,
            request,
            conversation,
            agent,
            turn,
            asking: None,
            ended: None,
        }
    }

    /// Records what the agent did since the last call, until it asks
    /// something or the turn ends.
    pub fn poll(&mut self, audit: &mut Audit) {
        if self.wants_attention() {
            return;
        }
        match self.turn.poll(&self.agent, audit) {
            Polled::Running => {}
            Polled::Asks(request) => self.asking = Some(request),
            Polled::Ended(ended) => self.ended = Some(ended),
        }
    }

    /// It waits for an answer, or it ended: to deal with at the prompt.
    pub fn wants_attention(&self) -> bool {
        self.asks() || self.ended.is_some()
    }

    pub fn asks(&self) -> bool {
        self.asking.is_some()
    }

    pub fn ended(&self) -> Option<&Ended> {
        self.ended.as_ref()
    }

    /// Asks the user what the agent asked, set apart from the conversation
    /// here: it is another turn's.
    pub fn answer(&mut self, audit: &mut Audit) {
        if let Some(request) = self.asking.take() {
            let whose = format!(
                "background turn {}: {}",
                self.number,
                audit::excerpt(&self.request, 50)
            );
            println!("{}", ui::rule(&whose));
            turn::answer_in(request, audit, self.turn.session());
            let end = format!("end of background turn {}'s question", self.number);
            println!("{}", ui::rule(&end));
        }
    }

    /// Asks the agent to cancel the turn; it ends as cancelled.
    pub fn cancel(&self) {
        self.agent.cancel();
    }

    /// `[1] done: compare the code with version 0.6.0`.
    pub fn label(&self, state: &str) -> String {
        format!(
            "[{}] {state}: {}",
            self.number,
            audit::excerpt(&self.request, 60)
        )
    }

    /// What it is doing and for how long, as a turn's status line says it:
    /// `Running: cargo test (42s) · 10 tool calls · 5:51`. `waiting for you`
    /// when it asks, and how it ended.
    pub fn progress(&self) -> String {
        let doing = match (&self.ended, self.asks()) {
            (Some(ended), _) => outcome(ended).to_string(),
            (None, true) => "waiting for you".to_string(),
            (None, false) => self.turn.activity(),
        };
        let mut parts = vec![doing];
        if self.turn.tool_calls() > 0 {
            parts.push(ui::tool_calls(self.turn.tool_calls()));
        }
        parts.push(ui::clock(self.turn.started().elapsed()));
        parts.join(" · ")
    }

    pub fn turn(&self) -> &Turn {
        &self.turn
    }
}

/// One word for how a job ended.
pub fn outcome(ended: &Ended) -> &'static str {
    match ended {
        Ended::Finished(None) => "done",
        Ended::Finished(Some(how)) => how,
        Ended::Failed(_) => "failed",
        Ended::AgentStopped(_) => "stopped",
    }
}
