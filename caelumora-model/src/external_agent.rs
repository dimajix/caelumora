//! What Caelumora keeps of an external agent (issue 179): an agent that runs
//! elsewhere and takes part in a channel through an `ExternalAgentActor`.
//! We cannot look into it - its context, its tools, its memory are its own -
//! so what we keep is what passed between it and us - in its
//! `AgentChannelState`, as for every agent (issue 206) - and the handles of
//! the protocol that tie the next exchange to the last one (`RemoteState`).

use crate::actor::EventContext;
use crate::confidentiality::Classification;
use crate::security::SecurityContext;
use crate::digest::Digest;
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
/// actor re-attaches to it, since it may have ended meanwhile. The external
/// agent's counterpart of a `TurnState` (issue 211): one piece of work
/// (`Work`), done as the request that began it, or took it up again,
/// says.
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
    /// The work it is: the contexts it is done in and what it writes as.
    /// The remote agent is called in them for as long as the task is open,
    /// to follow it, to cancel it, and what comes of it is said in them,
    /// classified as it. `None` for a task stored before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<Work>,
}

/// A piece of work of an external agent - one exchange with the remote
/// agent, and the task it may begin (issues 211, 212): the contexts it is
/// done in and what it writes as, what a `TurnState` holds of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// The envelope it answers, continued in the agent's name - as
    /// `TurnState::event_context`: what comes of it later is said in it,
    /// continued, so that it stays in the span and the chain between agents
    /// of the request (issue 45).
    pub event_context: EventContext,
    /// Whose authority it acts with - as `TurnState::security_context`.
    pub security_context: SecurityContext,
    /// The channel's classification when it began - as
    /// `TurnState::classification`: what it may write as at most.
    pub classification: Classification,
    /// The join of what it has seen - what the remote agent was given and
    /// what it brings in itself - within `classification` (`see`). As
    /// `TurnState::seen`.
    pub seen: Classification,
}

impl Work {
    /// Work begun in the contexts given, in a channel classified
    /// `classification`, which has seen nothing yet.
    pub fn new(event_context: EventContext, security_context: SecurityContext, classification: Classification) -> Self {
        Self { event_context, security_context, classification, seen: Classification::PUBLIC }
    }

    /// What it writes as: what it has seen so far (issue 168).
    pub fn label(&self) -> Classification {
        self.seen.clone()
    }

    /// Records that it has seen what is classified `classification`
    /// (`Classification::join_within`), as a turn does (`TurnState::see`).
    pub fn see(&mut self, classification: &Classification) {
        self.seen = self.seen.join_within(classification, &self.classification);
    }
}

/// What an external agent keeps of the remote side in one channel, as part
/// of its `AgentChannelState` (issue 206): how far its transcript was sent,
/// the remote conversation and the tasks that have not ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteState {
    /// How far the remote agent was given its transcript: the digest of
    /// the transcript as it stood when it was last sent. What came after it
    /// is what the remote agent has not heard yet, sent with the next call
    /// (issue 206); `None` if nothing was sent yet.
    pub sent_up_to: Option<Digest>,
    /// The remote agent's conversation with this channel, once it opened
    /// one.
    pub conversation: Option<ConversationId>,
    /// The tasks given to it that have not ended.
    pub open_tasks: Vec<OpenTask>,
}

impl RemoteState {
    /// Whether nothing is kept: so for an agent run here, and an external
    /// one before it was first asked.
    pub fn is_empty(&self) -> bool {
        self.sent_up_to.is_none() && self.conversation.is_none() && self.open_tasks.is_empty()
    }

    /// The task that asked back, if any: the next message to the remote
    /// agent answers it.
    pub fn asking(&self) -> Option<&TaskId> {
        self.open_tasks.iter().find(|t| t.state == OpenTaskState::InputRequired).map(|t| &t.task)
    }

    /// The tasks the remote agent is working on.
    pub fn working(&self) -> impl Iterator<Item = &TaskId> {
        self.open_tasks.iter().filter(|t| t.state == OpenTaskState::Working).map(|t| &t.task)
    }

    /// Records `task` as begun - or, asked back before, taken up again - by
    /// `work`, the exchange in answer to a request: working, and done as
    /// that work is from now on.
    pub fn begin(&mut self, task: &TaskId, work: &Work) {
        self.open(task, OpenTaskState::Working);
        if let Some(open) = self.find_mut(task) {
            open.work = Some(work.clone());
        }
    }

    /// Records `task` as open, standing at `state`.
    pub fn open(&mut self, task: &TaskId, state: OpenTaskState) {
        match self.find_mut(task) {
            Some(open) => open.state = state,
            None => self.open_tasks.push(OpenTask { task: task.clone(), since: Utc::now(), state, unsaid: None, work: None }),
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

    /// The work the open `task` is (`begin`), if it is kept: a task stored
    /// before it was, or not begun in answer to a request, has none.
    pub fn work_of(&self, task: &TaskId) -> Option<Work> {
        self.find(task)?.work.clone()
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
    fn work_writes_as_what_it_has_seen_never_above_its_channel() {
        let internal = Classification::general(crate::confidentiality::Confidentiality::Internal);
        let mut work = Work::new(EventContext::default(), SecurityContext::default(), internal.clone());
        assert_eq!(work.label(), Classification::PUBLIC, "nothing seen");
        work.see(&Classification::PUBLIC);
        assert_eq!(work.label(), Classification::PUBLIC);
        work.see(&internal);
        assert_eq!(work.label(), internal, "what it was given");
        work.see(&Classification::general(crate::confidentiality::Confidentiality::Confidential));
        assert_eq!(work.label(), internal, "never above its channel");
    }

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
        let bob = SecurityContext::new(crate::actor::ActorId::User(crate::user::UserId::new()), Default::default(), Default::default());
        remote.begin(&one, &Work::new(EventContext::default(), bob.clone(), Classification::CHANNEL_DEFAULT));
        assert_eq!(remote.working().collect::<Vec<_>>(), [&one, &two], "taken up again, it works");
        assert_eq!(remote.work_of(&one).map(|w| w.security_context.initiator_id), Some(bob.initiator_id), "in the contexts of the request");
        assert!(remote.work_of(&two).is_none(), "none kept");
        remote.open(&one, OpenTaskState::InputRequired);

        remote.close(&one);
        assert_eq!(remote.asking(), None);
        remote.set_unsaid(&one, Some("late".to_string()));
        assert_eq!(remote.open_tasks.len(), 1, "what is said of a closed task opens it not again");
    }
}
