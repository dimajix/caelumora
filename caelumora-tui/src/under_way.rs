//! An answer of an agent under way in a channel, as the terminal keeps it from what the channel
//! shows (issue 569).

use caelumora_domain::actor::{ActorId, ActorState};
use caelumora_domain::message::Message;
use caelumora_domain::work::TurnId;
use std::time::Instant;

/// An answer an agent has begun and not yet ended - said, given up -, as the web UI keeps its
/// answers under way: where the agent stands in it, and what it streamed of the message it is
/// writing.
#[derive(Debug, Clone)]
pub(crate) struct UnderWay {
    /// The agent that answers.
    pub(crate) agent: ActorId,
    /// Its name, as the channel shows it.
    pub(crate) name: String,
    /// The turn it answers in.
    pub(crate) turn: TurnId,
    /// Where the agent stands in it, as the channel showed last; `None` until it shows it.
    pub(crate) state: Option<ActorState>,
    /// Since when the answer is known to be under way: as it began, or as the terminal learnt of
    /// it.
    pub(crate) since: Instant,
    /// What the agent streamed of its reasoning for the message it is writing.
    pub(crate) reasoning: String,
    /// What the agent streamed of the words of the message it is writing.
    pub(crate) content: String,
}

impl UnderWay {
    /// The answer `agent`, named `name`, began in `turn` just now, in the state `state` if known.
    pub(crate) fn began(
        agent: ActorId,
        name: String,
        turn: TurnId,
        state: Option<ActorState>,
    ) -> Self {
        let since = Instant::now();
        Self { agent, name, turn, state, since, reasoning: String::new(), content: String::new() }
    }

    /// Takes in what `partial` streamed of the message being written: the next of its reasoning
    /// and of its words.
    pub(crate) fn stream(&mut self, partial: &Message) {
        self.reasoning.push_str(partial.reasoning.as_deref().unwrap_or_default());
        self.content.push_str(partial.content.as_deref().unwrap_or_default());
    }

    /// Forgets what was streamed: the message is complete, and what streams next is the next.
    pub(crate) fn complete(&mut self) {
        self.reasoning.clear();
        self.content.clear();
    }
}
