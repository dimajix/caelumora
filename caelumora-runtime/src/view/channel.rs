//! A channel as its viewer reached it, and its members.

use super::{AgentContextView, SessionView, ViewError, ViewResult, Views};
use crate::channel::{visible_part, ChannelLoop, VisibleHistory};
use crate::moderator::Moderator;
use crate::moderator_impl::ResponderPolicy;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentId};
use caelumora_model::approval::ApprovalRequest;
use caelumora_model::channel::{Channel, ChannelState, Participant};
use caelumora_model::confidentiality::{ChannelRef, Classification};
use caelumora_model::digest::Digest;
use caelumora_model::mention::handle_of;
use caelumora_model::name::Name;
use caelumora_model::note::Note;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes};
use caelumora_model::session::ChannelId;
use caelumora_model::snapshot::{ChannelSnapshot, MemberSnapshot};
use caelumora_model::store::StoreError;
use std::sync::Arc;
use tracing::warn;

/// The notes one agent published in a channel.
#[derive(Debug, Clone)]
pub struct PublishedNotes {
    pub agent: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
    /// The digest of the agent's note chain: the state of knowledge read.
    pub digest: Option<Digest>,
}

/// How much of a channel its viewer may read (issue 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Readable {
    /// All of it.
    Whole,
    /// Only what was released to where the viewer reads from: entry by
    /// entry, each by its own classification. A channel of which nothing
    /// is readable is answered as one that does not exist.
    Released,
}

/// A channel the viewer may read.
#[derive(Debug, Clone)]
pub struct ChannelView {
    session: SessionView,
    /// The document as it was when the channel was reached.
    channel: Arc<Channel>,
    /// Its loop, if it runs.
    running: Option<Arc<ChannelLoop>>,
    readable: Readable,
}

impl ChannelView {
    pub(super) fn of(session: SessionView, channel: Arc<Channel>, running: Option<Arc<ChannelLoop>>, readable: Readable) -> Self {
        Self { session, channel, running, readable }
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

    pub fn name(&self) -> &Name {
        &self.channel.name
    }

    /// The kind of the channel's moderator, e.g. `policy_moderator` (issue
    /// 174); `None` if it is a preconfigured component.
    pub fn moderator(&self) -> Option<String> {
        self.views().runtime.component_registry().kind_of::<dyn Moderator>(&self.channel.components)
    }

    /// The kind of the policy configured for the moderator's slot, e.g.
    /// `round_robin_policy` (issues 44, 174); `None` if it is a
    /// preconfigured component.
    pub fn responder_policy(&self) -> Option<String> {
        self.views().runtime.component_registry().kind_of::<dyn ResponderPolicy>(&self.channel.components)
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

    /// Whether the viewer may read an entry of the channel classified
    /// `classification`: every one if it reads the channel as a whole, else
    /// those released to where it reads from.
    pub(super) async fn may_read_entry(&self, classification: &Classification) -> bool {
        match self.readable {
            Readable::Whole => true,
            Readable::Released => self.views().may_read_from(&self.channel_ref(), classification, &self.session.privacy()).await,
        }
    }

    /// What a read of a channel read entry by entry answers when nothing
    /// could be read: that there is no such channel, so that nothing can be
    /// probed.
    fn nothing_released(&self) -> ViewError {
        ViewError::NoChannelNamed(self.channel.name.to_string())
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
            members.push(MemberView::of(self.clone(), participant.clone(), false).await);
        }
        members
    }

    /// The member `actor` of the channel - or, for an agent that took part
    /// and left, a former member (`MemberView::former`): its messages are in
    /// the history and its state in the store, so what it saw stays
    /// readable (issue 163). Until the store marks who left (issue 164), a
    /// former member is known by the messages it left in the history the
    /// viewer sees.
    ///
    /// # Errors
    /// `MemberNotFound` if `actor` neither takes part in the channel nor
    /// left messages in it; `Failed` if the history cannot be read.
    pub async fn member(&self, actor: ActorId) -> ViewResult<MemberView> {
        if let Some(participant) = self.channel.participants.iter().find(|p| p.actor_id() == actor) {
            return Ok(MemberView::of(self.clone(), participant.clone(), false).await);
        }
        let not_found = || ViewError::MemberNotFound { channel: self.channel.id, actor };
        let ActorId::Agent(agent) = actor else { return Err(not_found()) };
        if !self.history().await?.messages.iter().any(|m| m.sender_id == actor) {
            return Err(not_found());
        }
        Ok(MemberView::of(self.clone(), Participant::Agent(agent), true).await)
    }

    /// The history as the viewer sees it (issue 46): all of it, unless they
    /// joined later with a limit - with what a reader records as read
    /// (issue 72); of a channel read entry by entry, the messages released
    /// to where the viewer reads from. From the running loop, or from the
    /// store.
    ///
    /// # Errors
    /// `Failed` if the stored history cannot be read; `NoChannelNamed` if
    /// nothing of a channel read entry by entry can be.
    pub async fn history(&self) -> ViewResult<VisibleHistory> {
        // Whoever reads: an agent reads as itself, not as the owner it acts
        // for - its own limit applies, not the owner's.
        let actor = self.views().viewer().context().caller_id;
        let mut history = match &self.running {
            Some(channel_loop) => channel_loop.visible_history(actor).await,
            None => {
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
                VisibleHistory { messages: visible_part(start, state.history.messages()), digest: state.history.digest(), start }
            }
        };
        if self.readable == Readable::Released {
            let mut released = Vec::with_capacity(history.messages.len());
            for message in history.messages {
                if self.may_read_entry(&message.classification).await {
                    released.push(message);
                }
            }
            if released.is_empty() {
                return Err(self.nothing_released());
            }
            history.messages = released;
        }
        Ok(history)
    }

    /// The notes every agent of the channel published (issue 17), read from
    /// their states in the store: notes of finished turns only; of a
    /// channel read entry by entry, those released to where the viewer
    /// reads from.
    ///
    /// # Errors
    /// `Failed` if an agent's state cannot be read; `NoChannelNamed` if
    /// nothing of a channel read entry by entry can be.
    pub async fn notes(&self) -> ViewResult<Vec<PublishedNotes>> {
        let mut published = Vec::new();
        for member in self.members().await {
            if let Some(notes) = member.notes().await? {
                published.push(notes);
            }
        }
        if self.readable == Readable::Released && published.iter().all(|p| p.notes.is_empty()) {
            return Err(self.nothing_released());
        }
        Ok(published)
    }

    /// The approvals the channel's agents wait for (issue 172), read from
    /// their turns as stored - the record the approver is asked again from
    /// after a restart - for a viewer who takes part in the channel as a
    /// user: the users decide, and are told of every request as it is made.
    /// None for any other viewer.
    ///
    /// # Errors
    /// `Failed` if an agent's state cannot be read.
    pub async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        let viewer = self.views().viewer().context().initiator_id;
        let takes_part = self.channel.participants.iter().any(|p| matches!(p, Participant::User(_)) && p.actor_id() == viewer);
        if !takes_part || self.readable != Readable::Whole {
            return Ok(Vec::new());
        }
        let mut pending = Vec::new();
        for member in self.members().await {
            pending.extend(member.pending_approvals().await?);
        }
        Ok(pending)
    }

    /// The channel as the viewer sees it at this moment (issue 80).
    pub async fn snapshot(&self) -> ChannelSnapshot {
        let mut members = Vec::with_capacity(self.channel.participants.len());
        for member in self.members().await {
            members.push(member.snapshot().await);
        }
        ChannelSnapshot {
            id: self.channel.id,
            name: self.channel.name.to_string(),
            moderator: self.moderator(),
            responder_policy: self.responder_policy(),
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
    /// The name of a member that is no agent; `None` for a user that no
    /// longer exists. An agent's is in its document.
    name: Option<String>,
    /// For an agent: its document - its name, its attributes, which its
    /// context is read by, and its personality, which introduces it (issue
    /// 176); `None` for one that no longer exists.
    agent: Option<Agent>,
    /// Took part once and left.
    former: bool,
}

impl MemberView {
    async fn of(channel: ChannelView, participant: Participant, former: bool) -> Self {
        let (name, agent) = match participant {
            Participant::Agent(id) => (None, channel.views().agent_of(id).await),
            _ => (channel.views().name_of(&participant).await, None),
        };
        Self { channel, participant, name, agent, former }
    }

    /// Whether the member took part once and left: not among the channel's
    /// participants any more, but what it said and saw is still there.
    pub fn former(&self) -> bool {
        self.former
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
        match &self.agent {
            Some(agent) => Some(agent.name.as_str()),
            None => self.name.as_deref(),
        }
    }

    /// The handle the member is `@`-mentioned by (see `crate::mention`), or
    /// `None` if it cannot be mentioned - approvers, participants that no
    /// longer exist, names that yield no handle.
    pub fn handle(&self) -> Option<String> {
        match &self.participant {
            Participant::Approver(_) => None,
            _ => self.name().map(handle_of).filter(|handle| !handle.is_empty()),
        }
    }

    /// How the member is introduced (issue 176): an agent as its
    /// personality introduces it, a user as they introduce themselves in
    /// their profile. `None` for an approver, for a member that has no
    /// introduction, and for one that no longer exists. Public, like the
    /// name.
    ///
    /// # Errors
    /// `Failed` if it cannot be read.
    pub async fn introduction(&self) -> ViewResult<Option<String>> {
        self.channel.views().introduction_of(&self.participant, self.agent.as_ref()).await
    }

    /// Whether the viewer may see what this member saw when it produced a
    /// message (`ReadAgentContext`, issue 41) - only ever for agents.
    pub async fn may_read_context(&self) -> bool {
        match &self.agent {
            Some(agent) => self.channel.views().runtime().security_manager().may_read_agent_context(self.channel.views().viewer().context(), &agent.security_attributes).await,
            None => false,
        }
    }

    /// The notes the member published in the channel (issue 17), with the
    /// digest of their chain - of a channel read entry by entry, those
    /// released to where the viewer reads from; `None` for a member that
    /// is no agent.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    pub async fn notes(&self) -> ViewResult<Option<PublishedNotes>> {
        let Participant::Agent(agent) = self.participant else { return Ok(None) };
        let state = self.stored_state(agent).await?;
        let notes = self.channel.released(state.notes.current()).await;
        Ok(Some(PublishedNotes { agent: self.id(), name: self.name().unwrap_or_default().to_string(), notes, digest: state.notes.digest() }))
    }

    /// The approvals the member's turn waits for (issue 172), as stored;
    /// none for a member that is no agent or has left.
    ///
    /// # Errors
    /// `Failed` if the agent's state cannot be read.
    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>> {
        let Participant::Agent(agent) = self.participant else { return Ok(Vec::new()) };
        if self.former {
            return Ok(Vec::new());
        }
        let state = self.stored_state(agent).await?;
        Ok(state.turn.map(|turn| turn.pending_approvals()).unwrap_or_default())
    }

    /// The private notes the member wrote in the channel (issue 51), with
    /// the digest of their chain, for a viewer who may read what it saw
    /// (`ReadAgentContext`) - of a channel read entry by entry, those
    /// released to where the viewer reads from (issue 133).
    ///
    /// # Errors
    /// `NoContext` if the member is no agent, `Forbidden` if the viewer may
    /// not read what it saw, `Failed` if its state cannot be read.
    pub async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)> {
        let (agent, _) = self.readable_agent().await?;
        let state = self.stored_state(agent).await?;
        Ok((self.channel.released(state.private_notes.current()).await, state.private_notes.digest()))
    }

    /// What the member saw in the channel (issue 41): its transcript, its
    /// private notes, the context of each of its messages, the turns it
    /// gave up - for a viewer who may read its context (`ReadAgentContext`)
    /// and the channel as a whole.
    ///
    /// # Errors
    /// `NoContext` if the member is no agent, `Forbidden` if the viewer may
    /// not read what it saw or reads the channel entry by entry only,
    /// `Failed` if its state cannot be read.
    pub async fn context(&self) -> ViewResult<AgentContextView> {
        let (agent, _) = self.readable_agent().await?;
        if self.channel.readable == Readable::Released {
            return Err(ViewError::Forbidden { actor: self.id() });
        }
        Ok(AgentContextView::of(self.id(), self.stored_state(agent).await?))
    }

    /// The member as an agent whose context the viewer may read.
    async fn readable_agent(&self) -> ViewResult<(AgentId, &SecurityAttributes)> {
        let (Participant::Agent(agent), Some(Agent { security_attributes: attributes, .. })) = (&self.participant, &self.agent) else {
            return Err(ViewError::NoContext { actor: self.id() });
        };
        let views = self.channel.views();
        let forbidden = || ViewError::Forbidden { actor: self.id() };
        views.runtime().security_manager()
            .require(views.viewer().context(), ActionType::ReadAgentContext(attributes), forbidden).await
            .map_err(|_| forbidden())?;
        Ok((*agent, attributes))
    }

    /// The state of `agent` in the channel, as stored.
    async fn stored_state(&self, agent: AgentId) -> ViewResult<AgentChannelState> {
        let channel = self.channel.id();
        self.channel.views().runtime().state_store().get_agent_channel_state(agent, channel).await.map_err(|e| {
            warn!("Cannot read the state of agent {} in channel {}: {}", agent, channel, e);
            ViewError::Failed(anyhow::Error::new(e).context(format!("reading the state of {} in channel {}", self.id(), channel)))
        })
    }

    /// The member as the viewer sees it at this moment (issue 80).
    pub async fn snapshot(&self) -> MemberSnapshot {
        MemberSnapshot { id: self.id(), name: self.name().map(str::to_string), handle: self.handle(), can_read_context: self.may_read_context().await }
    }
}

impl ChannelView {
    /// Of `notes`, those the viewer may read (`may_read_entry`).
    async fn released(&self, notes: Vec<Note>) -> Vec<Note> {
        if self.readable == Readable::Whole {
            return notes;
        }
        let mut released = Vec::with_capacity(notes.len());
        for note in notes {
            if self.may_read_entry(&note.classification).await {
                released.push(note);
            }
        }
        released
    }
}
