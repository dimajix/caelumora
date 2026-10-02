//! What Caelumora keeps of an external agent (issue 179): an agent that runs
//! elsewhere and takes part in a channel through an `ExternalAgentActor`.
//! We cannot look into it - its context, its tools, its memory are its own -
//! so what we keep is what passed between it and us, and the handles of the
//! protocol that tie the next exchange to the last one.

use crate::actor::EventContext;
use crate::chain::Stored;
use crate::message::MessageId;
use crate::message_history::{HistoryEntry, MessageHistory};
use crate::store::Journaled;
use crate::transcript::{Transcript, TranscriptEntry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The remote agent's handle for its conversation with one channel - an
/// A2A `contextId`. Opaque: only the remote agent reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The remote agent's handle for one piece of work it was given - an A2A
/// task `id`. Opaque like `ConversationId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a task that has not ended stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenTaskState {
    /// The remote agent is working on it.
    Working,
    /// The remote agent asked something back; the next message to it
    /// continues the task.
    InputRequired,
}

/// A task given to the remote agent that has not ended: after a restart the
/// actor re-attaches to it, since it may have ended meanwhile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTask {
    pub task: TaskId,
    /// When it was given.
    pub since: DateTime<Utc>,
    pub state: OpenTaskState,
    /// What came of it while it was asked, if the answer broke off before
    /// the task ended: not said yet, but said - once - with the rest when
    /// the task ends (issue 195).
    pub unsaid: Option<String>,
    /// The context of the answer to the request that began it, or took it
    /// up again: what comes of it later is said in that context continued,
    /// so that it stays in the span and the chain between agents of the
    /// request (issue 45). `None` for a task stored before it was kept.
    #[serde(default)]
    pub requested: Option<EventContext>,
}

/// An external agent in one channel, as its `ExternalAgentActor` keeps it.
/// Not an `AgentChannelState`: there is no turn, no notes - nothing of what
/// we cannot see (issue 179). Stored under a key of its own. Its history and
/// its transcript are only ever appended to, for the record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalAgentChannelState {
    /// The conversation of the channel as the agent took part in it: all
    /// that was offered to it, and all it answered (issues 06, 196).
    pub history: MessageHistory,
    /// What the remote agent was given, in the order it was given - what
    /// passed the agent's perceive processors (issue 196), all we know of
    /// what it knows.
    pub transcript: Transcript,
    /// The last message of the history that was offered to the perceive
    /// processors on the way to the remote agent; what came after it is
    /// offered next. Kept apart from the transcript, since the processors
    /// may drop, change or add messages (issue 196).
    pub offered_up_to: Option<MessageId>,
    /// The remote agent's conversation with this channel, once it opened
    /// one.
    pub conversation: Option<ConversationId>,
    /// The tasks given to it that have not ended.
    pub open_tasks: Vec<OpenTask>,
}

/// A change to an [`ExternalAgentChannelState`] since it was last stored
/// (issue 193): a variant per part of the state, the growing ones as what
/// was added, the small ones whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAgentChannelStateChange {
    History(Stored<HistoryEntry>),
    Transcript(Stored<TranscriptEntry>),
    OfferedUpTo(Option<MessageId>),
    Conversation(Option<ConversationId>),
    OpenTasks(Vec<OpenTask>),
}

impl Journaled for ExternalAgentChannelState {
    type Change = ExternalAgentChannelStateChange;

    fn changes(&self) -> Vec<Self::Change> {
        let mut changes: Vec<Self::Change> = Vec::new();
        changes.extend(self.history.changes().map(ExternalAgentChannelStateChange::History));
        changes.extend(self.transcript.changes().map(ExternalAgentChannelStateChange::Transcript));
        changes.push(ExternalAgentChannelStateChange::OfferedUpTo(self.offered_up_to));
        changes.push(ExternalAgentChannelStateChange::Conversation(self.conversation.clone()));
        changes.push(ExternalAgentChannelStateChange::OpenTasks(self.open_tasks.clone()));
        changes
    }

    fn mark_stored(&mut self) {
        self.history.mark_stored();
        self.transcript.mark_stored();
    }

    fn apply(&mut self, change: Self::Change) -> Result<(), String> {
        match change {
            ExternalAgentChannelStateChange::History(change) => self.history.apply(change)?,
            ExternalAgentChannelStateChange::Transcript(change) => self.transcript.apply(change)?,
            ExternalAgentChannelStateChange::OfferedUpTo(last) => self.offered_up_to = last,
            ExternalAgentChannelStateChange::Conversation(conversation) => self.conversation = conversation,
            ExternalAgentChannelStateChange::OpenTasks(tasks) => self.open_tasks = tasks,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{replayed, stored};
    use crate::message::tests::{id, make_message};

    #[test]
    fn what_changed_in_an_external_agents_state_replays_onto_what_was_stored() {
        let mut state = ExternalAgentChannelState::default();
        state.history.extend(vec![make_message(id(1))]);
        let mut copy = stored(&mut state);

        state.history.extend(vec![make_message(id(2))]);
        state.transcript.extend(vec![make_message(id(1)), make_message(id(2))]);
        state.offered_up_to = Some(id(2));
        state.conversation = Some(ConversationId("c-1".to_string()));
        state.open_tasks.push(OpenTask { task: TaskId("t-1".to_string()), since: Utc::now(), state: OpenTaskState::Working, unsaid: Some("so far".to_string()), requested: None });
        assert_eq!(state.changes().len(), 2 + 3, "the history's, the transcript's, the three small parts");
        replayed(&state, &mut copy);

        state.mark_stored();
        state.open_tasks.clear();
        assert_eq!(state.changes().len(), 3, "the small parts alone");
        replayed(&state, &mut copy);
        assert!(copy.open_tasks.is_empty());
    }
}
