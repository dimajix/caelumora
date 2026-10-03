//! What Caelumora keeps of an external agent (issue 179): an agent that runs
//! elsewhere and takes part in a channel through an `ExternalAgentActor`.
//! We cannot look into it - its context, its tools, its memory are its own -
//! so what we keep is what passed between it and us - in its
//! `AgentChannelState`, as for every agent (issue 206) - and the handles of
//! the protocol that tie the next exchange to the last one (`RemoteState`).

use crate::actor::EventContext;
use crate::message::MessageId;
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

/// What an external agent keeps of the remote side in one channel, as part
/// of its `AgentChannelState` (issue 206): how far it offered what it heard,
/// the remote conversation and the tasks that have not ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteState {
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

impl RemoteState {
    /// The task that asked back, if any: the next message to the remote
    /// agent answers it.
    pub fn asking(&self) -> Option<&TaskId> {
        self.open_tasks.iter().find(|t| t.state == OpenTaskState::InputRequired).map(|t| &t.task)
    }

    /// The tasks the remote agent is working on.
    pub fn working(&self) -> impl Iterator<Item = &TaskId> {
        self.open_tasks.iter().filter(|t| t.state == OpenTaskState::Working).map(|t| &t.task)
    }

    /// Records `task` as open, standing at `state`.
    pub fn open(&mut self, task: &TaskId, state: OpenTaskState) {
        match self.find_mut(task) {
            Some(open) => open.state = state,
            None => self.open_tasks.push(OpenTask { task: task.clone(), since: Utc::now(), state, unsaid: None, requested: None }),
        }
    }

    /// Forgets `task`: it ended.
    pub fn close(&mut self, task: &TaskId) {
        self.open_tasks.retain(|t| &t.task != task);
    }

    /// What came of the open `task` before and was not said yet.
    pub fn unsaid_of(&self, task: &TaskId) -> Option<String> {
        self.find(task).and_then(|t| t.unsaid.clone())
    }

    /// Keeps `unsaid` with the open `task`, to be said when it ends.
    pub fn set_unsaid(&mut self, task: &TaskId, unsaid: Option<String>) {
        if let Some(open) = self.find_mut(task) {
            open.unsaid = unsaid;
        }
    }

    /// The context of the answer to the request that began the open
    /// `task`, if it is kept.
    pub fn requested_in(&self, task: &TaskId) -> Option<EventContext> {
        self.find(task).and_then(|t| t.requested.clone())
    }

    /// Keeps `requested`, the context of the answer to the request that
    /// began or took up the open `task`, with it.
    pub fn set_requested(&mut self, task: &TaskId, requested: &EventContext) {
        if let Some(open) = self.find_mut(task) {
            open.requested = Some(requested.clone());
        }
    }

    fn find(&self, task: &TaskId) -> Option<&OpenTask> {
        self.open_tasks.iter().find(|t| &t.task == task)
    }

    fn find_mut(&mut self, task: &TaskId) -> Option<&mut OpenTask> {
        self.open_tasks.iter_mut().find(|t| &t.task == task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_part_keeps_each_open_task_once_and_forgets_it_when_it_ends() {
        let (one, two) = (TaskId("t-1".to_string()), TaskId("t-2".to_string()));
        let mut remote = RemoteState::default();
        remote.open(&one, OpenTaskState::Working);
        remote.open(&two, OpenTaskState::Working);
        remote.open(&one, OpenTaskState::InputRequired);
        assert_eq!(remote.open_tasks.len(), 2, "opened again, a task stands where it stands now");
        assert_eq!(remote.asking(), Some(&one));
        assert_eq!(remote.working().collect::<Vec<_>>(), [&two]);

        remote.set_unsaid(&two, Some("so far".to_string()));
        assert_eq!(remote.unsaid_of(&two).as_deref(), Some("so far"));
        assert_eq!(remote.unsaid_of(&one), None);
        let requested = EventContext::default();
        remote.set_requested(&one, &requested);
        assert!(remote.requested_in(&one).is_some() && remote.requested_in(&two).is_none());

        remote.close(&one);
        assert_eq!(remote.asking(), None);
        remote.set_unsaid(&one, Some("late".to_string()));
        assert_eq!(remote.open_tasks.len(), 1, "what is said of a closed task opens it not again");
    }
}
