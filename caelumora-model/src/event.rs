use crate::agent::{AgentId, AgentUpdateEvent};
use crate::channel::{ChannelId, ChannelUpdateEvent};
use crate::session::{SessionId, SessionUpdateEvent};
use crate::user::{UserId, UserProfile, UserUpdateEvent};
use crate::workspace::{WorkspaceId, WorkspaceUpdateEvent};
use crate::skill::{SkillId, SkillUpdateEvent};
use crate::artifact::ArtifactId;
use crate::group::{GroupId, GroupUpdateEvent};
use crate::confidentiality::Classification;
use crate::security::SecurityAttributes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalEvent {
    SessionCreated(SessionId),
    SessionUpdated(SessionId, SessionUpdateEvent),
    SessionDeleted(SessionId),
    /// A deleted session was restored (issue 164): it is there again, as it
    /// was, and is started when it is opened.
    SessionRestored(SessionId),
    SessionStarted(SessionId),
    SessionStopped(SessionId),
    ChannelCreated(SessionId, ChannelId),
    ChannelUpdated(SessionId, ChannelId, ChannelUpdateEvent),
    ChannelDeleted(SessionId, ChannelId),
    /// A deleted channel was restored into its session (issue 164).
    ChannelRestored(SessionId, ChannelId),
    ChannelStarted(SessionId, ChannelId),
    ChannelStopped(SessionId, ChannelId),
    /// About an agent itself, which belongs to no session: one of user or
    /// global scope takes part in several (issue 121).
    AgentCreated(AgentId),
    AgentUpdated(AgentId, AgentUpdateEvent),
    AgentDeleted(AgentId),
    /// A deleted agent was restored (issue 164); it takes part in no
    /// channel until it is added again.
    AgentRestored(AgentId),
    AgentChannelJoined(SessionId, ChannelId, AgentId),
    AgentChannelLeft(SessionId, ChannelId, AgentId),
    /// The agent's own state changed - its memory (issue 62). Published with
    /// the agent's attributes once the change is stored; without content,
    /// since the state is private: whoever may see it fetches it.
    AgentStateChanged(AgentId),
    /// The state of an agent in a channel changed - its notes or private
    /// notes (issue 65). Published with the channel's attributes once it is
    /// stored, before the agent announces the end of its turn; without
    /// content, since part of it is private.
    AgentChannelStateChanged(SessionId, ChannelId, AgentId),
    UserChannelJoined(SessionId, ChannelId, UserId),
    UserChannelLeft(SessionId, ChannelId, UserId),
    UserCreated(UserId),
    UserUpdated(UserId, UserUpdateEvent),
    UserDeleted(UserId),
    /// The profile of a user changed (issue 13), as it now stands.
    /// Published with the profile's attributes: only its user, and the
    /// runtime, receive it.
    UserProfileUpdated(UserId, UserProfile),
    /// About a workspace (issue 253), which belongs to a user, not to a
    /// session it is attached to. Published with the workspace's attributes:
    /// its owner and whom its access list grants reading receive them.
    WorkspaceCreated(WorkspaceId),
    WorkspaceUpdated(WorkspaceId, WorkspaceUpdateEvent),
    WorkspaceDeleted(WorkspaceId),
    /// A deleted workspace was restored, with its files (issue 164).
    WorkspaceRestored(WorkspaceId),
    /// About a skill (issue 22). Published with the skill's attributes: its
    /// owner and whom its access list grants reading receive them -
    /// everybody, for the system's.
    SkillCreated(SkillId),
    SkillUpdated(SkillId, SkillUpdateEvent),
    SkillDeleted(SkillId),
    SkillRestored(SkillId),
    /// About an artifact (issue 271), which belongs to a user. Published
    /// with the artifact's attributes: its owner receives them; the readers
    /// of a channel learn of a version from the message that published it.
    /// A version was published - the first makes the artifact.
    ArtifactPublished(ArtifactId, u32),
    /// Kept in another scope, or described anew.
    ArtifactUpdated(ArtifactId),
    ArtifactDeleted(ArtifactId),
    ArtifactRestored(ArtifactId),
    /// About a team (issue 286). Published with the group's attributes: its
    /// owner and its members receive them. The participants of a session or
    /// channel are announced as they join and leave, not as a group.
    GroupCreated(GroupId),
    GroupUpdated(GroupId, GroupUpdateEvent),
    GroupDeleted(GroupId),
    GroupRestored(GroupId),
}

pub trait EventPublisher: Send + Sync + std::fmt::Debug {
    /// Publishes `event` about an entity with these `attributes` - the
    /// session's, channel's, agent's, user's or workspace's the event is
    /// about (issue 39).
    /// They decide who may receive it; they are not part of the event.
    fn publish(&self, event: GlobalEvent, attributes: SecurityAttributes);

    /// Publishes `event` about a classified entity - a channel and what
    /// happens in it, a workspace, a skill, an artifact: received only by
    /// whoever may read it and is cleared for one of `classifications`, as
    /// the views reach it (issue 312). One classification, the entity's;
    /// two as it is reclassified, the one it had and the one it has, so
    /// that whoever no longer reaches it learns so.
    fn publish_classified(&self, event: GlobalEvent, attributes: SecurityAttributes, classifications: &[Classification]);
}

impl EventPublisher for () {
    fn publish(&self, _event: GlobalEvent, _attributes: SecurityAttributes) {}

    fn publish_classified(&self, _event: GlobalEvent, _attributes: SecurityAttributes, _classifications: &[Classification]) {}
}
