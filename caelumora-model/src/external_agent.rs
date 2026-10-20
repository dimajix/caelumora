//! What Caelumora keeps of an external agent (issue 179): an agent that runs
//! elsewhere and takes part in a channel through an `ExternalAgentActor`.
//! We cannot look into it - its context, its tools, its memory are its own -
//! so what we keep is what passed between it and us - in its
//! `AgentChannelState`, as for every agent (issue 206) - and the handles of
//! the protocol that tie the next exchange to the last one (`RemoteState`).

use crate::confidentiality::Classification;
use crate::digest::Digest;
use crate::work::Work;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// What an external agent keeps of the remote side in one channel, as part
/// of its `AgentChannelState` (issue 206): the remote conversation and the
/// tasks that have not ended. How far it was given what was said needs no
/// mark: its transcript starts anew with every call (issue 205).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteState {
    /// The remote agent's conversation with this channel, once it opened
    /// one.
    pub conversation: Option<ConversationId>,
    /// The tasks given to it that have not ended.
    pub open_tasks: Vec<OpenTask>,
    /// What it was given last of each prompt part in its conversation
    /// (issue 214): the digest of the text, by the component the part came
    /// from. A part is given again only when it changed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub given_prompts: BTreeMap<String, Digest>,
    /// Every classification it was given in this channel, each once: of
    /// every message and every change of one it was given (issue 236), in
    /// any of its conversations. Recorded as it is given, so that what it
    /// was given before need not be read again (issue 376).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub given_classifications: Vec<Classification>,
}

impl RemoteState {
    /// Whether nothing is kept: so for an agent run here, and an external
    /// one before it was first asked.
    pub fn is_empty(&self) -> bool {
        self.conversation.is_none() && self.open_tasks.is_empty() && self.given_prompts.is_empty() && self.given_classifications.is_empty()
    }

    /// Whether the remote agent works on a task it was given (issue 388):
    /// one that asked something back waits for an answer instead.
    pub fn has_work_under_way(&self) -> bool {
        self.open_tasks.iter().any(|task| task.state == OpenTaskState::Working)
    }

    /// Records that the remote agent keeps its conversation as
    /// `conversation`. If it opened another than the one it had, what it was
    /// given of the prompt parts is no longer known to it.
    pub fn converse_in(&mut self, conversation: ConversationId) {
        if self.conversation.as_ref().is_some_and(|before| *before != conversation) {
            self.given_prompts.clear();
        }
        self.conversation = Some(conversation);
    }

    /// Whether the remote agent was given last of the part of `source` the
    /// text of `digest`.
    pub fn has_given(&self, source: &str, digest: &Digest) -> bool {
        self.given_prompts.get(source) == Some(digest)
    }

    /// Records that the remote agent was given the part of `source` with the
    /// text of `digest`.
    pub fn give(&mut self, source: String, digest: Digest) {
        self.given_prompts.insert(source, digest);
    }

    /// Records that the remote agent was given what is classified
    /// `classification`.
    pub fn give_classified(&mut self, classification: &Classification) {
        if !self.given_classifications.contains(classification) {
            self.given_classifications.push(classification.clone());
        }
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
    use crate::actor::{EventContext, ActorId};
    use crate::confidentiality::Classification;
    use crate::security::SecurityContext;
    use crate::digest;
    use crate::user::UserId;

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
        let bob = SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default());
        remote.begin(&one, &Work::new(EventContext::default(), bob.clone(), Classification::CHANNEL_DEFAULT));
        assert_eq!(remote.working().collect::<Vec<_>>(), [&one, &two], "taken up again, it works");
        assert_eq!(remote.work_of(&one).map(|w| w.security_context.initiator_id), Some(bob.initiator_id), "in the contexts of the request");
        assert!(remote.work_of(&two).is_none(), "none kept");
        remote.open(&one, OpenTaskState::InputRequired);

        remote.give("members".to_string(), digest::prompt("[Who is in this channel]"));
        assert!(remote.has_given("members", &digest::prompt("[Who is in this channel]")));
        remote.converse_in(ConversationId("c-1".to_string()));
        assert!(remote.has_given("members", &digest::prompt("[Who is in this channel]")), "the first conversation keeps what it was given");
        remote.converse_in(ConversationId("c-2".to_string()));
        assert!(remote.given_prompts.is_empty(), "another one has none of it");

        remote.close(&one);
        assert_eq!(remote.asking(), None);
        remote.set_unsaid(&one, Some("late".to_string()));
        assert_eq!(remote.open_tasks.len(), 1, "what is said of a closed task opens it not again");
    }
}
