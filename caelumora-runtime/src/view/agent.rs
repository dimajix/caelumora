//! The state of an agent member of a channel, as its kind keeps it (issue
//! 179): an agent run here in its `AgentChannelState`, an external agent in
//! its `ExternalAgentChannelState`. A `MemberView` decides the kind once and
//! asks the one that fits; whether the viewer may see it, and which of it
//! is released to them, stays the member view's.

use super::context::{AgentContextView, ExternalContext, InternalContext};
use super::{ViewError, ViewResult};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId, PersonalityKind};
use caelumora_model::approval::ApprovalRequest;
use caelumora_model::digest::Digest;
use caelumora_model::external_agent::ExternalAgentChannelState;
use caelumora_model::note::Note;
use caelumora_model::session::ChannelId;
use caelumora_model::store::{StateStore, StoreError};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::warn;

/// An agent's state in one channel, as stored.
#[async_trait]
pub(super) trait AgentState: Send + Sync + Debug {
    /// The notes it published (issue 17), with the digest of their chain.
    async fn notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)>;

    /// The notes only it reads (issue 51), with the digest of their chain.
    async fn private_notes(&self) -> ViewResult<(Vec<Note>, Option<Digest>)>;

    /// The approvals its turn waits for (issue 172).
    async fn pending_approvals(&self) -> ViewResult<Vec<ApprovalRequest>>;

    /// What it saw (issue 41).
    async fn context(&self) -> ViewResult<Box<dyn AgentContextView>>;
}

/// The state of `agent` in `channel`, read from `store` as an agent of
/// `kind` keeps it.
pub(super) fn agent_state(kind: PersonalityKind, agent: AgentId, channel: ChannelId, store: Arc<dyn StateStore>) -> Arc<dyn AgentState> {
    let place = Place { agent, channel, store };
    match kind {
        PersonalityKind::Internal => Arc::new(InternalAgentState(place)),
        PersonalityKind::External => Arc::new(ExternalAgentState(place)),
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

/// An agent run here: its turns, its notes, what its model was given.
#[derive(Debug)]
struct InternalAgentState(Place);

impl InternalAgentState {
    async fn stored(&self) -> ViewResult<AgentChannelState> {
        let place = &self.0;
        place.store.get_agent_channel_state(place.agent, place.channel).await.map_err(|e| place.failed(e))
    }
}

#[async_trait]
impl AgentState for InternalAgentState {
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
        Ok(Box::new(InternalContext::of(ActorId::Agent(self.0.agent), self.stored().await?)))
    }
}

/// An external agent (issue 179): what it heard and was given. It writes no
/// notes and has no turn that could wait for an approval.
#[derive(Debug)]
struct ExternalAgentState(Place);

#[async_trait]
impl AgentState for ExternalAgentState {
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
        let place = &self.0;
        let state: ExternalAgentChannelState = place.store.get_external_agent_channel_state(place.agent, place.channel).await
            .map_err(|e| place.failed(e))?;
        Ok(Box::new(ExternalContext::of(ActorId::Agent(place.agent), state)))
    }
}
