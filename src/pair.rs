//! Pairing (`#pair <agent>`): a second agent answers what the agent you work
//! with says when it stops. It knows the issue, or what you asked of it, and
//! what the agent said last: not how the agent got there.

use crate::acp::AgentHandle;

/// What the second agent answers when it has nothing more to say.
const DONE: &str = "DONE";

const ROLE: &str = "\
You are pairing with another agent, which works in this directory. You do not \
see its reasoning or what it did: only what it says each time it stops, which \
I will send you.

Each time, look at the state of the work yourself (the files, and `git diff` \
when it is code) and answer what the agent says. Unless I ask something else \
of you below, decide what it does next: answer its question, choose among the \
options it gives, or say what is wrong or missing. Do not change anything \
yourself.

What you answer is sent to that agent as its next message, once I agree: \
write it to the agent, short and direct. When you have nothing more to say, \
answer only: DONE";

pub struct Pair {
    /// The second agent's name in the configuration.
    pub name: String,
    pub agent: AgentHandle,
    pub brief: Brief,
}

/// What the second agent knows of the work.
enum About {
    /// Nothing yet: your next message is the issue, for both agents.
    Awaited,
    /// The issue the agent works on.
    Issue(String),
    /// What you asked of the second agent alone: `#pair <agent> <text>`.
    Asked(String),
}

/// What the second agent is told.
pub struct Brief {
    about: About,
    /// It answered once: it has its role, and what `about` says.
    briefed: bool,
}

impl Brief {
    /// `asked` is what you wrote after `#pair <agent>`, if anything.
    pub fn new(asked: &str) -> Self {
        let about = match asked.trim() {
            "" => About::Awaited,
            asked => About::Asked(asked.to_string()),
        };
        Self {
            about,
            briefed: false,
        }
    }

    pub fn wants_issue(&self) -> bool {
        matches!(self.about, About::Awaited)
    }

    pub fn set_issue(&mut self, issue: String) {
        self.about = About::Issue(issue);
    }

    /// It answered: it has its role, and the issue.
    pub fn briefed(&mut self) {
        self.briefed = true;
    }

    /// What the second agent gets when the agent stopped with `answer`:
    /// with its role and the issue, or what you asked of it, until it
    /// answered once (`briefed`). `None` when there is nothing to ask: it
    /// waits for the issue, or the agent said nothing, unless you `asked`
    /// for its answer, as the work may be older than the conversation.
    pub fn message(&self, answer: &str, asked: bool) -> Option<String> {
        let answer = answer.trim();
        if self.wants_issue() || (answer.is_empty() && !asked) {
            return None;
        }
        let stopped = match answer {
            "" => "The agent has not said anything yet: look at the work as it is.".to_string(),
            answer => format!("The agent stopped with:\n{answer}"),
        };
        if self.briefed {
            return Some(stopped);
        }
        Some(match &self.about {
            About::Awaited => return None,
            About::Issue(issue) => {
                format!("{ROLE}\n\nThe issue it works on:\n{issue}\n\n{stopped}")
            }
            About::Asked(asked) => format!("{ROLE}\n\nWhat I ask of you:\n{asked}\n\n{stopped}"),
        })
    }
}

/// The second agent has nothing more to say.
pub fn is_done(answer: &str) -> bool {
    answer.trim().trim_end_matches('.') == DONE
}

/// What you do with the second agent's answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// It goes to the working agent.
    Send,
    /// It does not: you answer the agent yourself.
    Keep,
    /// You wrote to the second agent instead, and it answers again.
    Ask(String),
}

/// Reads your answer to "Send this?": Enter or `y` sends, `n` keeps, and
/// anything else is for the second agent. `None` is the end of the input.
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
    fn the_second_agent_gets_its_role_and_the_issue_until_it_answered() {
        let mut brief = Brief::new("");
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

        assert!(first.starts_with("You are pairing with another agent"));
        assert!(first.ends_with(
            "answer only: DONE\n\n\
             The issue it works on:\nAdd a retry to the client\n\n\
             The agent stopped with:\nA or B? I recommend A."
        ));
        assert_eq!(second, "The agent stopped with:\nDone, the tests pass.");
    }

    #[test]
    fn it_is_asked_nothing_without_the_issue_or_an_answer() {
        let mut brief = Brief::new("  ");
        assert_eq!(brief.message("A or B?", true), None);

        brief.set_issue("Add a retry".to_string());
        // A turn of the agent that ended without an answer.
        assert_eq!(brief.message("", false), None);
        assert_eq!(brief.message(" \n", false), None);
        let first = brief.message("A or B?", false).unwrap();
        assert!(first.contains("The issue it works on:\nAdd a retry"));
    }

    #[test]
    fn what_you_ask_of_it_is_its_alone_and_not_the_issue() {
        let mut brief = Brief::new(" only review the tests ");
        assert!(!brief.wants_issue());

        let first = brief.message("Done.", false).unwrap();
        brief.briefed();

        assert!(first.ends_with(
            "answer only: DONE\n\n\
             What I ask of you:\nonly review the tests\n\n\
             The agent stopped with:\nDone."
        ));
        assert!(!first.contains("The issue it works on:"));
        assert_eq!(
            brief.message("Fixed it.", false).unwrap(),
            "The agent stopped with:\nFixed it."
        );
    }

    #[test]
    fn when_you_ask_it_looks_at_the_work_without_an_answer() {
        let mut brief = Brief::new("review the pairing of two agents");

        let first = brief.message("", true).unwrap();
        brief.briefed();

        assert!(first.ends_with(
            "What I ask of you:\nreview the pairing of two agents\n\n\
             The agent has not said anything yet: look at the work as it is."
        ));
        assert_eq!(
            brief.message("", true).unwrap(),
            "The agent has not said anything yet: look at the work as it is."
        );
    }

    #[test]
    fn done_is_the_whole_answer() {
        assert!(is_done("DONE"));
        assert!(is_done(" DONE.\n"));
        assert!(!is_done("DONE, but rename the function first"));
        assert!(!is_done("done"));
    }

    #[test]
    fn the_gate_sends_keeps_or_asks_the_second_agent() {
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
