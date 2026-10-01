//! A channel as its viewer reached it, and its members.

use super::{SessionView, ViewError, ViewResult, Views};
use crate::channel::ChannelLoop;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::mention::handle_of;
use caelumora_model::security::{AccessQuestions, SecurityAttributes};
use caelumora_model::session::ChannelId;
use caelumora_model::snapshot::{ChannelSnapshot, MemberSnapshot};
use std::sync::Arc;

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

    /// The member as the viewer sees it at this moment (issue 80).
    pub async fn snapshot(&self) -> MemberSnapshot {
        MemberSnapshot { id: self.id(), name: self.name.clone(), handle: self.handle(), can_read_context: self.may_read_context().await }
    }
}
