//! What the terminal prints of the channels it follows and of itself, by its meaning, which each
//! way of drawing shows its own way (issue 569).

use caelumora_domain::approval::ApprovalRequest;
use caelumora_domain::content::ContentRef;
use caelumora_domain::question::QuestionRequest;
use caelumora_domain::tool::{ToolCall, ToolCallResult};
use caelumora_domain::value::name::Name;
use std::ops::Range;

/// One thing the terminal prints: what was said, asked or told, not yet how it looks - so that a
/// way of drawing for a screen reader can say in words what another shows by colour.
#[derive(Debug, Clone)]
pub(crate) enum Printed {
    /// What a participant said.
    Said {
        /// Who said it, by name.
        sender: String,
        /// What they said.
        said: Said,
        /// In which kind of message.
        saying: Saying,
    },
    /// That an agent began to answer.
    Answering {
        /// The agent, by name.
        sender: String,
    },
    /// A tool a participant calls, with its arguments - `Null` where they are not shown.
    ToolCall {
        /// Who calls it, by name.
        sender: String,
        /// The call.
        call: Box<ToolCall>,
    },
    /// How a tool call ended, with its result - empty where it is not shown.
    ToolResult(Box<ToolCallResult>),
    /// An approval requested, with how to answer it.
    Approval(Box<ApprovalRequest>),
    /// Questions a tool asks, with how to answer them if they are put to the user.
    Questions {
        /// The questions.
        request: Box<QuestionRequest>,
        /// Whether they are put to the user.
        put_to_user: bool,
    },
    /// What the terminal tells of itself and of the channels it follows: which one it talks in,
    /// that one is followed no more, that an approval ended.
    Notice(String),
    /// What went wrong, as the user may be told it.
    Error {
        /// How it went wrong.
        kind: ErrorKind,
        /// What went wrong.
        message: String,
    },
    /// That the terminal missed what a channel showed, so many events of it: what it knows it read
    /// anew.
    Missed {
        /// How many events were missed.
        events: u64,
    },
    /// A line as it is: what happened in a channel, what a command answered, a prompt.
    Plain(String),
    /// What `printed` tells of the channel `channel` while the user talks in another.
    Elsewhere {
        /// The channel it is of.
        channel: Name,
        /// What is told of it.
        printed: Box<Printed>,
    },
}

/// What a message says: its words and what it carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Said {
    /// Its words, if it has any.
    pub(crate) words: Option<Words>,
    /// What it carries.
    pub(crate) attachments: Vec<ContentRef>,
}

/// The words of a message, with where they mention a participant.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Words {
    /// The words as they were said.
    pub(crate) text: String,
    /// Where in `text` a participant is mentioned, in the order they stand.
    pub(crate) mentions: Vec<Range<usize>>,
}

/// The kind of message something was said in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Saying {
    /// A message complete, of an answer or of a person.
    Complete,
    /// A step of a plan under way, not the answer itself.
    Step,
    /// A message changed by its author.
    Edited,
}

/// How something went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// It failed: an operation of the user's, or an agent's answer.
    Failed,
    /// The channel did not let in what the user sent (issue 304).
    Rejected,
    /// The actor it went to did not take what the user sent (issue 567).
    Refused,
}

impl Printed {
    /// `printed` of the channel `channel`, told while the user talks in another.
    pub(crate) fn elsewhere(channel: &Name, printed: Vec<Printed>) -> Vec<Printed> {
        let of =
            |printed| Printed::Elsewhere { channel: channel.clone(), printed: Box::new(printed) };
        printed.into_iter().map(of).collect()
    }
}
