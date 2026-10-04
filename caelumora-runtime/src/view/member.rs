//! What a member of a channel is, as its kind makes it: a user, an agent run
//! here, an external agent (issue 179), an approver - or one that no longer
//! exists. A `MemberView` asks the `MemberState` that fits; whether the
//! viewer may see something, and which of it is released to them, stays the
//! member view's, in one place for every kind.
//!
//! Only agents have a state in the channel worth reading - notes, approvals
//! waited for, what they saw. That is the `AgentMember` a member state
//! hands out if it is an agent, so that no other kind has to answer for
//! what it does not have. Both kinds of agent keep the same state (issue
//! 206): which kind it is does not matter here. Its personality is read
//! for its introduction alone, when that is first asked for, not for every
//! member looked at (issue 189).

use super::context::AgentContextView;
use super::{ViewError, ViewResult, Views};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentId};
use caelumora_model::channel::Participant;
use caelumora_model::digest::Digest;
use caelumora_model::mention::handle_of;
use caelumora_model::note::Note;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::ChannelId;
use caelumora_model::store::{EntityStore, StateStore, StoreError, StoreResultExt};
use caelumora_model::user::UserId;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::warn;

/// A member of a channel, as its kind makes it.
#[async_trait]
pub(super) trait MemberState: Send + Sync + Debug {
    /// Its name; `None` for a user or agent that no longer exists.
    fn name(&self) -> Option<&str>;

    /// The handle it is `@`-mentioned by (see `crate::mention`), or `None`
    /// if it cannot be mentioned.
    fn handle(&self) -> Option<String> {
        self.name().map(handle_of).filter(|handle| !handle.is_empty())
    }

    /// How it is introduced (issue 176). Public, like the name.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    async fn introduction(&self) -> ViewResult<Option<String>> {
        Ok(None)
    }

    /// An agent's attributes: what `ReadAgentContext` is asked about.
    /// `None` for a member that is no agent.
    fn attributes(&self) -> Option<&SecurityAttributes> {
        None
    }

    /// The member as an agent, if it is one that exists.
    fn agent(&self) -> Option<&AgentMember> {
        None
    }
}

/// The member `participant` of channel `channel`, as `views` read it: its
/// document now, and the rest - an agent's personality too - when it is
/// asked for. Read in the runtime's name: taking part in a session makes a
/// name known to everybody in it (issue 120), introductions are public,
/// and an agent's kind decides how its state is read, not what the viewer
/// may see.
pub(super) async fn member_state(views: &Views, participant: &Participant, channel: ChannelId) -> Arc<dyn MemberState> {
    let runtime = views.runtime();
    match participant {
        Participant::User(id) => match runtime.entity_store().get_user(&SecurityContext::system(), *id).await {
            Ok(user) => Arc::new(UserMember { id: *id, name: user.username, store: runtime.entity_store(), introduction: OnceCell::new() }),
            Err(StoreError::NotFound { .. }) => Arc::new(GoneMember),
            Err(e) => {
                warn!("Cannot read user {}: {:#}", id, e);
                Arc::new(GoneMember)
            }
        },
        Participant::Approver(_) => Arc::new(ApproverMember { name: participant.actor_id().fixed_name() }),
        Participant::Agent(id) => match views.agent_of(*id).await {
            Some(agent) => Arc::new(AgentMemberState {
                member: AgentMember(StateRef { agent: agent.id, channel, store: runtime.state_store() }),
                agent,
                store: runtime.entity_store(),
                introduction: OnceCell::new(),
            }),
            None => Arc::new(GoneMember),
        },
    }
}

/// A user: their name, and how they introduce themselves, read once when
/// it is first asked for - most who look at a member do not.
#[derive(Debug)]
struct UserMember {
    id: UserId,
    name: String,
    store: Arc<dyn EntityStore>,
    introduction: OnceCell<Option<String>>,
}

#[async_trait]
impl MemberState for UserMember {
    fn name(&self) -> Option<&str> {
        Some(&self.name)
    }

    async fn introduction(&self) -> ViewResult<Option<String>> {
        let introduction = self.introduction.get_or_try_init(|| async {
            let profile = self.store.get_user_profile(&SecurityContext::system(), self.id).await.optional()
                .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("reading the introduction of {}", ActorId::User(self.id)))))?;
            Ok::<_, ViewError>(profile.and_then(|profile| profile.introduction))
        }).await?;
        Ok(introduction.clone())
    }
}

/// An approver: a name of its own, and nothing to mention or introduce.
#[derive(Debug)]
struct ApproverMember {
    name: Option<&'static str>,
}

impl MemberState for ApproverMember {
    fn name(&self) -> Option<&str> {
        self.name
    }

    fn handle(&self) -> Option<String> {
        None
    }
}

/// A user or agent that no longer exists: what it said stays, nothing else
/// of it is known.
#[derive(Debug)]
struct GoneMember;

impl MemberState for GoneMember {
    fn name(&self) -> Option<&str> {
        None
    }
}

/// Where an agent's state in a channel is kept: which agent, which
/// channel, which store.
#[derive(Debug, Clone)]
struct StateRef {
    agent: AgentId,
    channel: ChannelId,
    store: Arc<dyn StateStore>,
}

impl StateRef {
    /// The error for a state that cannot be read, logged.
    fn failed(&self, e: StoreError) -> ViewError {
        warn!("Cannot read the state of agent {} in channel {}: {}", self.agent, self.channel, e);
        ViewError::Failed(anyhow::Error::new(e).context(format!("reading the state of {} in channel {}", ActorId::Agent(self.agent), self.channel)))
    }
}

/// An agent, of either kind: its name and attributes from its document at
/// once, its state in the channel when it is asked for; its personality,
/// which says how it is introduced, read once, when that is first needed
/// (issue 189).
#[derive(Debug)]
struct AgentMemberState {
    agent: Agent,
    member: AgentMember,
    store: Arc<dyn EntityStore>,
    introduction: OnceCell<Option<String>>,
}

#[async_trait]
impl MemberState for AgentMemberState {
    fn name(&self) -> Option<&str> {
        Some(self.agent.name.as_str())
    }

    /// How its personality introduces it; nothing if the personality is
    /// gone.
    async fn introduction(&self) -> ViewResult<Option<String>> {
        let introduction = self.introduction.get_or_try_init(|| async {
            let personality = self.store.get_personality(&SecurityContext::system(), self.agent.personality_id).await.optional()
                .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("reading the personality of agent {}", self.agent.id))))?;
            Ok::<_, ViewError>(personality.and_then(|personality| personality.introduction))
        }).await?;
        Ok(introduction.clone())
    }

    fn attributes(&self) -> Option<&SecurityAttributes> {
        Some(&self.agent.security_attributes)
    }

    fn agent(&self) -> Option<&AgentMember> {
        Some(&self.member)
    }
}

/// An agent member's state in the channel, of either kind (issues 179,
/// 206): its notes, its turn, what it saw. Of an external agent, which
/// writes no notes and has no turn, those parts are empty.
#[derive(Debug)]
pub(super) struct AgentMember(StateRef);

impl AgentMember {
    /// Its state in the channel as stored, read anew on every call - what
    /// a member's details are taken from, read once for all of them.
    pub(super) async fn stored(&self) -> ViewResult<AgentChannelState> {
        let place = &self.0;
        place.store.get_agent_channel_state(place.agent, place.channel).await.map_err(|e| place.failed(e))
    }

    /// The notes only it reads (issue 51), with the digest of their chain.
    pub(super) async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let state = self.stored().await?;
        Ok((state.private_notes.current(), state.private_notes.digest()))
    }

    /// What it saw (issue 41).
    pub(super) async fn context(&self) -> ViewResult<AgentContextView> {
        Ok(AgentContextView::of(ActorId::Agent(self.0.agent), self.stored().await?))
    }
}
