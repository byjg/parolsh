//! Pairing: a second agent answers what the agent you work with says when it
//! stops. A navigator (`#pair <agent>`) knows the issue and decides what the
//! agent asks; a reviewer (`#review <agent>`) comes after the work and says
//! what it sees in it. Neither knows how the agent got there.

use crate::acp::AgentHandle;

/// What the second agent answers when it has nothing more to say.
const DONE: &str = "DONE";

const NAVIGATOR: &str = "\
You are pairing with another coding agent, which works on the issue below in \
this directory. You do not see its reasoning or what it did: only what it says \
each time it stops, which I will send you.

Each time, look at the state of the work yourself (the files, `git diff`) and \
decide what it does next: answer its question, choose among the options it \
gives, or say what is wrong or missing against the issue. Do not change any \
file yourself.

What you answer is sent to that agent as its next message, once I agree: \
write it to the agent, short and direct. When the issue is solved and nothing \
is left to decide, answer only: DONE";

const REVIEWER: &str = "\
You are reviewing the work of another coding agent in this directory. You do \
not see its reasoning or what it did: only what it says each time it stops, \
which I will send you.

Each time, look at the work yourself (the files, `git diff`) and write what \
you observe: what is wrong, risky or missing, and what is worth knowing about \
it. Do not change any file yourself, and do not tell the agent what to do: it \
decides what to do with what you observed.

What you answer is sent to that agent as its next message, once I agree: \
write it to the agent, short and direct. When you have nothing to add, answer \
only: DONE";

/// What the second agent is there for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Navigator,
    Reviewer,
}

impl Role {
    /// What it does with the agent's answer: `codex decides`.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Navigator => "decides",
            Self::Reviewer => "reviews",
        }
    }

    /// What its `DONE` means.
    pub fn done(self) -> &'static str {
        match self {
            Self::Navigator => "has nothing left to decide",
            Self::Reviewer => "has nothing to add",
        }
    }
}

pub struct Pair {
    /// The second agent's name in the configuration.
    pub name: String,
    pub agent: AgentHandle,
    pub brief: Brief,
}

/// What the second agent is told.
pub struct Brief {
    pub role: Role,
    /// A navigator's issue, `None` until you give it: your first message
    /// after `#pair`. What a reviewer looks at, when you said.
    about: Option<String>,
    /// It has its role, and the issue.
    briefed: bool,
}

impl Brief {
    pub fn navigator() -> Self {
        Self {
            role: Role::Navigator,
            about: None,
            briefed: false,
        }
    }

    /// `focus` is what you wrote after `#review <agent>`, if anything.
    pub fn reviewer(focus: &str) -> Self {
        Self {
            role: Role::Reviewer,
            about: Some(focus.trim().to_string()),
            briefed: false,
        }
    }

    /// A navigator waits for its issue.
    pub fn wants_issue(&self) -> bool {
        self.about.is_none()
    }

    pub fn set_issue(&mut self, issue: String) {
        self.about = Some(issue);
    }

    /// It answered: it has its role, and the issue.
    pub fn briefed(&mut self) {
        self.briefed = true;
    }

    /// What the second agent gets when the agent stopped with `answer`:
    /// with its role and the issue or what to look at, until it answered
    /// once (`briefed`). `None` when there is nothing to ask: a navigator
    /// without its issue, or an agent that said nothing, except when you
    /// `asked` a reviewer, as the work may be older than the conversation.
    pub fn message(&self, answer: &str, asked: bool) -> Option<String> {
        let answer = answer.trim();
        if self.wants_issue() || (answer.is_empty() && !(asked && self.role == Role::Reviewer)) {
            return None;
        }
        let stopped = match answer {
            "" => "The agent has not said anything yet: look at the work as it is.".to_string(),
            answer => format!("The agent stopped with:\n{answer}"),
        };
        if self.briefed {
            return Some(stopped);
        }
        let about = self.about.as_deref().unwrap_or_default();
        Some(match self.role {
            Role::Navigator => format!("{NAVIGATOR}\n\nThe issue:\n{about}\n\n{stopped}"),
            Role::Reviewer if about.is_empty() => format!("{REVIEWER}\n\n{stopped}"),
            Role::Reviewer => format!("{REVIEWER}\n\nLook at:\n{about}\n\n{stopped}"),
        })
    }
}

/// The second agent has nothing more to say.
pub fn is_done(decision: &str) -> bool {
    decision.trim().trim_end_matches('.') == DONE
}

/// What you do with the navigator's decision.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// It goes to the working agent.
    Send,
    /// It does not: you answer the agent yourself.
    Keep,
    /// You wrote to the navigator instead, and it decides again.
    Ask(String),
}

/// Reads your answer to "Send this?": Enter or `y` sends, `n` keeps, and
/// anything else is for the navigator. `None` is the end of the input.
pub fn gate(answer: Option<&str>) -> Gate {
    let Some(answer) = answer.map(str::trim) else {
        return Gate::Keep;
    };
    match answer.to_lowercase().as_str() {
        "" | "y" | "yes" => Gate::Send,
        "n" | "no" => Gate::Keep,
        _ => Gate::Ask(answer.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_navigator_gets_its_role_and_the_issue_until_it_answered() {
        let mut brief = Brief::navigator();
        assert!(brief.wants_issue());
        brief.set_issue("Add a retry to the client".to_string());
        assert!(!brief.wants_issue());

        let first = brief.message("  A or B? I recommend A.\n", false).unwrap();
        // Not answered, as when its turn was cancelled: briefed again.
        assert_eq!(
            brief.message("  A or B? I recommend A.", true),
            Some(first.clone())
        );
        brief.briefed();
        let second = brief.message("Done, the tests pass.", false).unwrap();

        assert!(first.starts_with("You are pairing with another coding agent"));
        assert!(first.ends_with(
            "The issue:\nAdd a retry to the client\n\n\
             The agent stopped with:\nA or B? I recommend A."
        ));
        assert_eq!(second, "The agent stopped with:\nDone, the tests pass.");
    }

    #[test]
    fn a_navigator_is_asked_nothing_without_the_issue_or_an_answer() {
        let mut brief = Brief::navigator();
        assert_eq!(brief.message("A or B?", true), None);

        brief.set_issue("Add a retry".to_string());
        assert_eq!(brief.message("", true), None);
        assert_eq!(brief.message(" \n", false), None);
        assert!(
            brief
                .message("A or B?", false)
                .unwrap()
                .contains("The issue:\nAdd a retry")
        );
    }

    #[test]
    fn the_reviewer_needs_no_issue_and_gets_what_to_look_at_when_you_said() {
        let mut plain = Brief::reviewer("");
        let focused = Brief::reviewer(" the error handling ");
        assert!(!plain.wants_issue());

        let first = plain
            .message("Done: the amount is validated.", false)
            .unwrap();
        let with_focus = focused.message("Done.", false).unwrap();
        plain.briefed();

        assert!(first.starts_with("You are reviewing the work of another coding agent"));
        assert!(first.ends_with(
            "answer only: DONE\n\nThe agent stopped with:\nDone: the amount is validated."
        ));
        assert!(
            with_focus.ends_with("Look at:\nthe error handling\n\nThe agent stopped with:\nDone.")
        );
        assert_eq!(
            plain.message("Fixed it.", false).unwrap(),
            "The agent stopped with:\nFixed it."
        );
    }

    #[test]
    fn a_reviewer_you_ask_looks_at_the_work_without_an_answer() {
        let mut brief = Brief::reviewer("the pairing of two agents");

        let first = brief.message("", true).unwrap();
        brief.briefed();

        assert!(first.ends_with(
            "Look at:\nthe pairing of two agents\n\n\
             The agent has not said anything yet: look at the work as it is."
        ));
        assert_eq!(
            brief.message("", true).unwrap(),
            "The agent has not said anything yet: look at the work as it is."
        );
        // Not after a turn of the agent that ended without an answer.
        assert_eq!(brief.message("", false), None);
    }

    #[test]
    fn done_is_the_whole_answer() {
        assert!(is_done("DONE"));
        assert!(is_done(" DONE.\n"));
        assert!(!is_done("DONE, but rename the function first"));
        assert!(!is_done("done"));
    }

    #[test]
    fn the_gate_sends_keeps_or_asks_the_navigator() {
        assert_eq!(gate(Some("\n")), Gate::Send);
        assert_eq!(gate(Some("Y\n")), Gate::Send);
        assert_eq!(gate(Some("no\n")), Gate::Keep);
        assert_eq!(gate(None), Gate::Keep);
        assert_eq!(
            gate(Some(" Why not B?\n")),
            Gate::Ask("Why not B?".to_string())
        );
    }
}
