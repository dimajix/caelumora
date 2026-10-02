//! What a member of a channel is, as its kind makes it: a user, an agent run
//! here, an external agent (issue 179), an approver - or one that no longer
//! exists. A `MemberView` decides the kind once and asks the `MemberState`
//! that fits; whether the viewer may see something, and which of it is
//! released to them, stays the member view's, in one place for every kind.
//!
//! Only agents have a state in the channel worth reading - notes, approvals
//! waited for, what they saw. That is the `AgentMember` a member state
//! hands out if it is an agent, so that no other kind has to answer for
//! what it does not have.

use super::context::{AgentContextView, ExternalContext, InternalContext};
use super::{ViewError, ViewResult, Views};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityKind};
use caelumora_model::approval::ApprovalRequest;
use caelumora_model::channel::Participant;
use caelumora_model::digest::Digest;
use caelumora_model::external_agent::ExternalAgentChannelState;
use caelumora_model::mention::handle_of;
use caelumora_model::note::Note;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::ChannelId;
use caelumora_model::store::{EntityStore, StateStore, StoreError};
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

    /// The member as an agent, if it is one that exists.
    fn agent(&self) -> Option<&dyn AgentMember> {
        None
    }
}

/// An agent member: its document, and its state in the channel as its kind
/// keeps it (issue 179).
#[async_trait]
pub(super) trait AgentMember: Send + Sync + Debug {
    /// The agent's attributes: what `ReadAgentContext` is asked about.
    fn attributes(&self) -> &SecurityAttributes;

    /// The notes it published (issue 17), with the digest of their chain.
    async fn notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)>;

    /// The notes only it reads (issue 51), with the digest of their chain.
    async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)>;

    /// The approvals its turn waits for (issue 172).
    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>>;

    /// What it saw (issue 41).
    async fn context(&self) -> ViewResult<Box<dyn AgentContextView>>;
}

/// The member `participant` of channel `channel`, as `views` read it: its
/// documents now - an agent's personality too, which decides its kind -
/// and the rest when it is asked for. Read in the runtime's name: taking
/// part in a session makes a name known to everybody in it (issue 120),
/// introductions are public, and an agent's kind decides how its state is
/// read, not what the viewer may see.
pub(super) async fn member_state(views: &Views, participant: &Participant, channel: ChannelId) -> Arc<dyn MemberState> {
    let runtime = views.runtime();
    match participant {
        Participant::User(id) => match runtime.entity_store().get_user(&SecurityContext::system(), *id).await {
            Ok(user) => Arc::new(UserMember { id: *id, name: user.username, store: runtime.entity_store(), introduction: OnceCell::new() }),
            Err(_) => Arc::new(GoneMember),
        },
        Participant::Approver(_) => Arc::new(ApproverMember { name: participant.actor_id().fixed_name() }),
        Participant::Agent(id) => {
            let Some(agent) = views.agent_of(*id).await else { return Arc::new(GoneMember) };
            let personality = views.personality_of(&agent).await.unwrap_or_else(|e| {
                warn!("Cannot read the personality of agent {}: {:#}", agent.id, e);
                None
            });
            // Without a personality, read as before there were kinds: the
            // state of an agent run here, which an external one does not
            // write.
            let kind = personality.as_ref().map_or(PersonalityKind::Internal, |p| p.kind);
            let documents = AgentDocuments { place: Place { agent: agent.id, channel, store: runtime.state_store() }, agent, personality };
            match kind {
                PersonalityKind::Internal => Arc::new(InternalAgentMember(documents)),
                PersonalityKind::External => Arc::new(ExternalAgentMember(documents)),
            }
        }
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
            match self.store.get_user_profile(&SecurityContext::system(), self.id).await {
                Ok(profile) => Ok(profile.introduction),
                Err(StoreError::NotFound { .. }) => Ok(None),
                Err(e) => Err(ViewError::Failed(anyhow::Error::new(e).context(format!("reading the introduction of {}", ActorId::User(self.id))))),
            }
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

/// Where an agent's state is kept.
#[derive(Debug, Clone)]
struct Place {
    agent: AgentId,
    channel: ChannelId,
    store: Arc<dyn StateStore>,
}

impl Place {
    /// The error for a state that cannot be read, logged.
    fn failed(&self, e: StoreError) -> ViewError {
        warn!("Cannot read the state of agent {} in channel {}: {}", self.agent, self.channel, e);
        ViewError::Failed(anyhow::Error::new(e).context(format!("reading the state of {} in channel {}", ActorId::Agent(self.agent), self.channel)))
    }
}

/// What every agent member has: its document, its personality - `None` if
/// it is gone or cannot be read - and where its state is kept.
#[derive(Debug)]
struct AgentDocuments {
    agent: Agent,
    personality: Option<Personality>,
    place: Place,
}

impl AgentDocuments {
    fn introduction(&self) -> Option<String> {
        self.personality.as_ref().and_then(|p| p.introduction.clone())
    }
}

/// An agent run here: its turns, its notes, what its model was given.
#[derive(Debug)]
struct InternalAgentMember(AgentDocuments);

impl InternalAgentMember {
    async fn stored(&self) -> ViewResult<AgentChannelState> {
        let place = &self.0.place;
        place.store.get_agent_channel_state(place.agent, place.channel).await.map_err(|e| place.failed(e))
    }
}

#[async_trait]
impl MemberState for InternalAgentMember {
    fn name(&self) -> Option<&str> {
        Some(self.0.agent.name.as_str())
    }

    async fn introduction(&self) -> ViewResult<Option<String>> {
        Ok(self.0.introduction())
    }

    fn agent(&self) -> Option<&dyn AgentMember> {
        Some(self)
    }
}

#[async_trait]
impl AgentMember for InternalAgentMember {
    fn attributes(&self) -> &SecurityAttributes {
        &self.0.agent.security_attributes
    }

    async fn notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let state = self.stored().await?;
        Ok((state.notes.current(), state.notes.digest()))
    }

    async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let state = self.stored().await?;
        Ok((state.private_notes.current(), state.private_notes.digest()))
    }

    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        Ok(self.stored().await?.turn.map(|turn| turn.pending_approvals()).unwrap_or_default())
    }

    async fn context(&self) -> ViewResult<Box<dyn AgentContextView>> {
        Ok(Box::new(InternalContext::of(ActorId::Agent(self.0.agent.id), self.stored().await?)))
    }
}

/// An external agent (issue 179): what it heard and was given. It writes no
/// notes and has no turn that could wait for an approval.
#[derive(Debug)]
struct ExternalAgentMember(AgentDocuments);

#[async_trait]
impl MemberState for ExternalAgentMember {
    fn name(&self) -> Option<&str> {
        Some(self.0.agent.name.as_str())
    }

    async fn introduction(&self) -> ViewResult<Option<String>> {
        Ok(self.0.introduction())
    }

    fn agent(&self) -> Option<&dyn AgentMember> {
        Some(self)
    }
}

#[async_trait]
impl AgentMember for ExternalAgentMember {
    fn attributes(&self) -> &SecurityAttributes {
        &self.0.agent.security_attributes
    }

    async fn notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        Ok((Vec::new(), None))
    }

    async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        Ok((Vec::new(), None))
    }

    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        Ok(Vec::new())
    }

    async fn context(&self) -> ViewResult<Box<dyn AgentContextView>> {
        let place = &self.0.place;
        let state: ExternalAgentChannelState = place.store.get_external_agent_channel_state(place.agent, place.channel).await
            .map_err(|e| place.failed(e))?;
        Ok(Box::new(ExternalContext::of(ActorId::Agent(place.agent), state)))
    }
}
