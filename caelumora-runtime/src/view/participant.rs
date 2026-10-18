//! What a participant of a channel is, as its kind makes it: a user, an agent run
//! here, an external agent (issue 179) - or one that no longer exists. A `ParticipantView` asks the `ParticipantState` that fits; whether the
//! viewer may see something, and which of it is released to them, stays the
//! participant view's, in one place for every kind.
//!
//! Only agents have a state in the channel worth reading - notes, approvals
//! waited for, what they saw. That is the `AgentParticipant` a participant state
//! hands out if it is an agent, so that no other kind has to answer for
//! what it does not have. Both kinds of agent keep the same state (issue
//! 206): which kind it is does not matter here. Its personality is read
//! for its introduction, whether it answers again (issue 242) and what
//! its profile shows (issue 244), when first asked for, not for every
//! participant looked at (issue 189).

use super::context::AgentContextView;
use super::{ViewError, ViewResult, Views};
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorId};
use caelumora_model::value::name::Name;
use caelumora_model::value::text::Introduction;
use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality};
use caelumora_model::channel::Participant;
use caelumora_model::digest::Digest;
use caelumora_model::mention::handle_of;
use caelumora_model::note::Note;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::ChannelId;
use caelumora_model::store::{EntityStore, StateStore, StoreError, StoreResultExt};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::transcript::TranscriptStore;
use caelumora_model::user::{UserId, UserProfile};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::warn;

/// A participant of a channel, as its kind makes it.
#[async_trait]
pub(super) trait ParticipantState: Send + Sync + Debug {
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
    async fn introduction(&self) -> ViewResult<Option<Introduction>> {
        Ok(None)
    }

    /// What of it is public to everyone in the channel, as participant `id`
    /// (issue 403); `None` for one that no longer exists.
    ///
    /// # Errors
    /// `Failed` if its introduction cannot be read.
    async fn describe(&self, _id: ActorId) -> ViewResult<Option<ActorDescription>> {
        Ok(None)
    }

    /// An agent's attributes: what `ReadAgentContext` is asked about.
    /// `None` for a participant that is no agent.
    fn attributes(&self) -> Option<&SecurityAttributes> {
        None
    }

    /// The participant as an agent, if it is one that exists.
    fn agent(&self) -> Option<&AgentParticipant> {
        None
    }

    /// An agent's personality, read once when first asked for: what it is
    /// introduced as, what kind of agent it is (issues 176, 242, 244).
    /// `None` for a participant that is no agent, and for an agent whose
    /// personality is gone.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    async fn personality(&self) -> ViewResult<Option<&Personality>> {
        Ok(None)
    }
}

/// The participant `participant` of channel `channel`, as `views` read it: its
/// document now, and the rest - an agent's personality too - when it is
/// asked for. Read in the runtime's name: taking part in a session makes a
/// name known to everybody in it (issue 120), introductions are public,
/// and an agent's kind decides how its state is read, not what the viewer
/// may see.
pub(super) async fn participant_state(views: &Views, participant: &Participant, channel: ChannelId) -> Arc<dyn ParticipantState> {
    match participant {
        Participant::User(id) => match views.stores().users.get(&SecurityContext::system(), *id).await {
            Ok(user) => Arc::new(UserParticipant { id: *id, name: user.username, profiles: Arc::clone(&views.stores().user_profiles), introduction: OnceCell::new() }),
            Err(StoreError::NotFound { .. }) => Arc::new(GoneParticipant { name: None }),
            Err(e) => {
                warn!("Cannot read user {}: {:#}", id, e);
                Arc::new(GoneParticipant { name: None })
            }
        },
        Participant::Agent(id) => match views.agent_of(*id).await {
            Some(agent) => Arc::new(AgentParticipantState {
                participant: AgentParticipant(StateRef { agent: agent.id, channel, store: Arc::clone(views.states()), histories: Arc::clone(views.histories()), transcripts: Arc::clone(views.transcripts()) }),
                agent,
                personalities: Arc::clone(&views.stores().personalities),
                personality: OnceCell::new(),
            }),
            // A deleted agent keeps its name (issue 164), nothing else.
            None => Arc::new(GoneParticipant { name: views.deleted_agent_name(*id).await }),
        },
    }
}

/// A user: their name, and how they introduce themselves, read once when
/// it is first asked for - most who look at a participant do not.
#[derive(Debug)]
struct UserParticipant {
    id: UserId,
    name: Name,
    profiles: Arc<dyn EntityStore<UserProfile>>,
    introduction: OnceCell<Option<Introduction>>,
}

#[async_trait]
impl ParticipantState for UserParticipant {
    fn name(&self) -> Option<&str> {
        Some(self.name.as_str())
    }

    async fn introduction(&self) -> ViewResult<Option<Introduction>> {
        let introduction = self.introduction.get_or_try_init(|| async {
            let profile = self.profiles.get(&SecurityContext::system(), self.id).await.optional()
                .map_err(|e| ViewError::failed(e, format!("reading the introduction of {}", ActorId::User(self.id))))?;
            Ok::<_, ViewError>(profile.and_then(|profile| profile.introduction))
        }).await?;
        Ok(introduction.clone())
    }

    async fn describe(&self, id: ActorId) -> ViewResult<Option<ActorDescription>> {
        Ok(Some(ActorDescription { id, name: self.name.clone(), introduction: self.introduction().await? }))
    }
}

/// A user or agent that no longer exists: what it said stays, nothing else
/// of it is known - but the name of an agent that was deleted, not purged
/// (issue 164).
#[derive(Debug)]
struct GoneParticipant {
    name: Option<String>,
}

impl ParticipantState for GoneParticipant {
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn handle(&self) -> Option<String> {
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
    /// Where its history is read (issue 379).
    histories: Arc<dyn HistoryStore>,
    /// Where its transcript is read beyond what the state holds (issue 380).
    transcripts: Arc<dyn TranscriptStore>,
}

impl StateRef {
    /// The error for a state that cannot be read, logged.
    fn failed(&self, e: StoreError) -> ViewError {
        warn!("Cannot read the state of agent {} in channel {}: {}", self.agent, self.channel, e);
        ViewError::failed(e, format!("reading the state of {} in channel {}", ActorId::Agent(self.agent), self.channel))
    }
}

/// An agent, of either kind: its name and attributes from its document at
/// once, its state in the channel when it is asked for; its personality,
/// which says how it is introduced and whether it answers again, read once,
/// when that is first needed (issue 189) - `None` if it is gone.
#[derive(Debug)]
struct AgentParticipantState {
    agent: Agent,
    participant: AgentParticipant,
    personalities: Arc<dyn EntityStore<Personality>>,
    personality: OnceCell<Option<Personality>>,
}

#[async_trait]
impl ParticipantState for AgentParticipantState {
    fn name(&self) -> Option<&str> {
        Some(self.agent.name.as_str())
    }

    /// How its personality introduces it; nothing if the personality is
    /// gone.
    async fn introduction(&self) -> ViewResult<Option<Introduction>> {
        Ok(self.personality().await?.and_then(|personality| personality.introduction.clone()))
    }

    async fn describe(&self, id: ActorId) -> ViewResult<Option<ActorDescription>> {
        Ok(Some(ActorDescription { id, name: self.agent.name.name.clone(), introduction: self.introduction().await? }))
    }

    async fn personality(&self) -> ViewResult<Option<&Personality>> {
        let personality = self.personality.get_or_try_init(|| async {
            self.personalities.get(&SecurityContext::system(), self.agent.personality_id).await.optional()
                .map_err(|e| ViewError::failed(e, format!("reading the personality of agent {}", self.agent.id)))
        }).await?;
        Ok(personality.as_ref())
    }

    fn attributes(&self) -> Option<&SecurityAttributes> {
        Some(&self.agent.security_attributes)
    }

    fn agent(&self) -> Option<&AgentParticipant> {
        Some(&self.participant)
    }
}

/// An agent participant's state in the channel, of either kind (issues 179,
/// 206): its notes, its turn, what it saw. Of an external agent, which
/// writes no notes and has no turn, those parts are empty.
#[derive(Debug)]
pub(super) struct AgentParticipant(StateRef);

impl AgentParticipant {
    /// Its state in the channel as stored, read anew on every call - what
    /// a participant's details are taken from, read once for all of them.
    pub(super) async fn stored(&self) -> ViewResult<AgentChannelState> {
        let place = &self.0;
        place.store.get_agent_channel_state(place.agent, place.channel).await.map_err(|e| place.failed(e))
    }

    /// The notes only it reads (issue 51), with the digest of their chain.
    pub(super) async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let state = self.stored().await?;
        Ok((state.private_notes.current(), state.private_notes.digest()))
    }

    /// What it saw (issue 41), its view of its channel's history beginning
    /// at the position `from`.
    pub(super) async fn context(&self, from: u64) -> ViewResult<AgentContextView> {
        let place = &self.0;
        Ok(AgentContextView::of(place.agent, place.channel, self.stored().await?, from, Arc::clone(&place.histories), Arc::clone(&place.transcripts)))
    }
}
