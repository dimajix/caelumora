//! A channel as its viewer reached it, and its members.

use super::{AgentContextView, SessionView, ViewError, ViewResult, Views};
use crate::channel::{visible_part, ChannelLoop, VisibleHistory};
use crate::session_reader::PublishedNotes;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::channel::{Channel, ChannelState, Participant};
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::mention::handle_of;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes};
use caelumora_model::session::ChannelId;
use caelumora_model::snapshot::{ChannelSnapshot, MemberSnapshot};
use caelumora_model::store::StoreError;
use std::sync::Arc;
use tracing::warn;

/// A channel the viewer may read.
#[derive(Debug, Clone)]
pub struct ChannelView {
    session: SessionView,
    /// The document as it was when the channel was reached.
    channel: Arc<Channel>,
    /// Its loop, if it runs.
    running: Option<Arc<ChannelLoop>>,
}

impl ChannelView {
    pub(super) fn of(session: SessionView, channel: Arc<Channel>, running: Option<Arc<ChannelLoop>>) -> Self {
        Self { session, channel, running }
    }

    fn views(&self) -> &Views {
        self.session.views()
    }

    /// The session the channel belongs to.
    pub fn session(&self) -> &SessionView {
        &self.session
    }

    pub fn id(&self) -> ChannelId {
        self.channel.id
    }

    pub fn name(&self) -> &str {
        &self.channel.name
    }

    /// The kind of moderator, e.g. `round_robin` (issue 44).
    pub fn moderator(&self) -> &str {
        &self.channel.moderator
    }

    /// The classification of the channel's contents (issue 133).
    pub fn classification(&self) -> &Classification {
        &self.channel.classification
    }

    pub fn channel_ref(&self) -> ChannelRef {
        ChannelRef { session_id: self.channel.session_id, channel_id: self.channel.id }
    }

    /// Whether the channel runs: its loop is where an operation goes.
    pub fn running(&self) -> Option<&Arc<ChannelLoop>> {
        self.running.as_ref()
    }

    /// Whether the viewer may rename the channel and add or remove its
    /// participants (`UpdateChannel`, issue 46).
    pub async fn may_change(&self) -> bool {
        self.views().runtime().security_manager().may_change_channel(self.views().viewer().context(), &self.channel.security_attributes).await
    }

    /// Its participants, in the order of the channel.
    pub async fn members(&self) -> Vec<MemberView> {
        let mut members = Vec::with_capacity(self.channel.participants.len());
        for participant in &self.channel.participants {
            members.push(MemberView::of(self.clone(), participant.clone()).await);
        }
        members
    }

    /// The member `actor` of the channel.
    ///
    /// # Errors
    /// `MemberNotFound` if `actor` does not take part in the channel.
    pub async fn member(&self, actor: ActorId) -> ViewResult<MemberView> {
        let participant = self.channel.participants.iter().find(|p| p.actor_id() == actor)
            .ok_or(ViewError::MemberNotFound { channel: self.channel.id, actor })?;
        Ok(MemberView::of(self.clone(), participant.clone()).await)
    }

    /// The history as the viewer sees it (issue 46): all of it, unless they
    /// joined later with a limit - with what a reader records as read
    /// (issue 72). From the running loop, or from the store.
    ///
    /// # Errors
    /// `Failed` if the stored history cannot be read.
    pub async fn history(&self) -> ViewResult<VisibleHistory> {
        let actor = self.views().viewer().context().initiator_id;
        if let Some(channel_loop) = &self.running {
            return Ok(channel_loop.visible_history(actor).await);
        }
        let state = match self.views().runtime().state_store().get_channel_state(self.channel.id).await {
            Ok(state) => state,
            // Nothing was said yet.
            Err(StoreError::NotFound { .. }) => ChannelState::new(self.channel.id),
            Err(e) => {
                warn!("Cannot read the history of channel {}: {}", self.channel.id, e);
                return Err(ViewError::Failed(anyhow::Error::new(e).context(format!("reading the history of channel {}", self.channel.id))));
            }
        };
        let start = self.channel.history_limits.iter().find(|l| l.participant == actor).map(|l| l.start);
        Ok(VisibleHistory { messages: visible_part(start, state.history.messages()), digest: state.history.digest(), start })
    }

    /// The notes every agent of the channel published (issue 17), read from
    /// their states in the store: notes of finished turns only.
    ///
    /// # Errors
    /// `Failed` if an agent's state cannot be read.
    pub async fn notes(&self) -> ViewResult<Vec<PublishedNotes>> {
        let mut published = Vec::new();
        for member in self.members().await {
            if let Some(notes) = member.notes().await? {
                published.push(notes);
            }
        }
        Ok(published)
    }

    /// The channel as the viewer sees it at this moment (issue 80).
    pub async fn snapshot(&self) -> ChannelSnapshot {
        let mut members = Vec::with_capacity(self.channel.participants.len());
        for member in self.members().await {
            members.push(member.snapshot().await);
        }
        ChannelSnapshot {
            id: self.channel.id,
            name: self.channel.name.clone(),
            moderator: self.channel.moderator.clone(),
            classification: self.channel.classification.clone(),
            can_change: self.may_change().await,
            members,
        }
    }
}

/// A participant of a channel - a user, an agent, an approver - as the
/// viewer reached it.
#[derive(Debug, Clone)]
pub struct MemberView {
    channel: ChannelView,
    participant: Participant,
    /// `None` for a user or agent that no longer exists.
    name: Option<String>,
    /// For an agent: its attributes, which its context is read by.
    agent: Option<SecurityAttributes>,
}

impl MemberView {
    async fn of(channel: ChannelView, participant: Participant) -> Self {
        let (name, agent) = channel.views().look_up(&participant).await;
        Self { channel, participant, name, agent }
    }

    /// The channel the member takes part in.
    pub fn channel(&self) -> &ChannelView {
        &self.channel
    }

    pub fn id(&self) -> ActorId {
        self.participant.actor_id()
    }

    /// `None` for a user or agent that no longer exists.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The handle the member is `@`-mentioned by (see `crate::mention`), or
    /// `None` if it cannot be mentioned - approvers, participants that no
    /// longer exist, names that yield no handle.
    pub fn handle(&self) -> Option<String> {
        match &self.participant {
            Participant::Approver(_) => None,
            _ => self.name.as_deref().map(handle_of).filter(|handle| !handle.is_empty()),
        }
    }

    /// Whether the viewer may see what this member saw when it produced a
    /// message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub async fn may_read_context(&self) -> bool {
        match &self.agent {
            Some(attributes) => self.channel.views().runtime().security_manager().may_read_agent_context(self.channel.views().viewer().context(), attributes).await,
            None => false,
        }
    }

    /// The notes the member published in the channel (issue 17), with the
    /// digest of their chain; `None` for a member that is no agent.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    pub async fn notes(&self) -> ViewResult<Option<PublishedNotes>> {
        let Participant::Agent(agent) = self.participant else { return Ok(None) };
        let state = self.stored_state(agent).await?;
        Ok(Some(PublishedNotes {
            agent: self.id(),
            name: self.name.clone().unwrap_or_default(),
            notes: state.notes.current(),
            digest: state.notes.digest(),
        }))
    }

    /// What the member saw in the channel (issue 41): its transcript, its
    /// private notes, the context of each of its messages, the turns it
    /// gave up - for a viewer who may read its context (`ReadAgentContext`).
    ///
    /// # Errors
    /// `NoContext` if the member is no agent, `Forbidden` if the viewer may
    /// not read what it saw, `Failed` if its state cannot be read.
    pub async fn context(&self) -> ViewResult<AgentContextView> {
        let (Participant::Agent(agent), Some(attributes)) = (&self.participant, &self.agent) else {
            return Err(ViewError::NoContext { actor: self.id() });
        };
        let views = self.channel.views();
        let forbidden = || ViewError::Forbidden { actor: self.id() };
        views.runtime().security_manager()
            .require(views.viewer().context(), ActionType::ReadAgentContext(attributes), forbidden).await
            .map_err(|_| forbidden())?;
        Ok(AgentContextView::of(self.id(), self.stored_state(*agent).await?))
    }

    /// The state of `agent` in the channel, as stored.
    async fn stored_state(&self, agent: caelumora_model::agent::AgentId) -> ViewResult<AgentChannelState> {
        let channel = self.channel.id();
        self.channel.views().runtime().state_store().get_agent_channel_state(agent, channel).await.map_err(|e| {
            warn!("Cannot read the state of agent {} in channel {}: {}", agent, channel, e);
            ViewError::Failed(anyhow::Error::new(e).context(format!("reading the state of {} in channel {}", self.id(), channel)))
        })
    }

    /// The member as the viewer sees it at this moment (issue 80).
    pub async fn snapshot(&self) -> MemberSnapshot {
        MemberSnapshot { id: self.id(), name: self.name.clone(), handle: self.handle(), can_read_context: self.may_read_context().await }
    }
}
