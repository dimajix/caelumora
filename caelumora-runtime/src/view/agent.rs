//! An agent as a viewer reached it on its own, not as a member of a channel
//! (issues 164, 227): one they may read (`GetAgent`), with what they may do
//! with it and the channels it takes part in that they may read.

use super::{ChannelView, ViewError, ViewResult, Views};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId};
use caelumora_model::security::{AccessQuestions, SecurityContext};
use caelumora_model::session::SessionField;
use caelumora_model::snapshot::AgentSnapshot;
use caelumora_model::store::query::{Filter, Query};
use caelumora_model::store::StoreResultExt;

#[derive(Clone)]
pub struct AgentView {
    views: Views,
    agent: Agent,
}

impl AgentView {
    pub(super) fn of(views: Views, agent: Agent) -> Self {
        Self { views, agent }
    }

    pub fn id(&self) -> AgentId {
        self.agent.id
    }

    /// Whether the viewer may rename it (`UpdateAgent`).
    pub async fn may_change(&self) -> bool {
        self.views.access_policy().may_change_agent(self.views.context(), &self.agent.security_attributes).await
    }

    /// Whether the viewer may delete it (`DeleteAgent`).
    pub async fn may_delete(&self) -> bool {
        self.views.access_policy().may_delete_agent(self.views.context(), &self.agent.security_attributes).await
    }

    /// The channels it takes part in - resting in them included - that the
    /// viewer may read as a whole: of the sessions that have it among their
    /// participants and the viewer may read (`GetSession`), in the order the
    /// store lists them, the channels `SessionView::channels` gives them, in
    /// the order of their session. Reading the agent grants nothing of
    /// where it takes part.
    ///
    /// # Errors
    /// `Failed` if the store cannot list the sessions.
    pub async fn channels(&self) -> ViewResult<Vec<ChannelView>> {
        let actor = ActorId::Agent(self.agent.id);
        let taking_part = Filter::any(SessionField::Participants, Filter::eq(SessionField::Participant, actor));
        let mut channels = Vec::new();
        for session in self.views.sessions_of(&Query::filtered(taking_part)).await? {
            channels.extend(session.channels().await.into_iter().filter(|channel| channel.lists(actor)));
        }
        Ok(channels)
    }

    /// The agent as the viewer sees it. Its personality is read in the
    /// runtime's name, as a member's is: what an agent is made of is no
    /// secret to whoever may read the agent.
    ///
    /// # Errors
    /// `Failed` if its personality cannot be read; one that is gone is
    /// `None`.
    pub async fn snapshot(&self) -> ViewResult<AgentSnapshot> {
        let personality = self.views.stores().personalities.get(&SecurityContext::system(), self.agent.personality_id).await.optional()
            .map_err(|e| ViewError::failed(e, format!("reading the personality of agent {}", self.agent.id)))?;
        Ok(AgentSnapshot {
            id: self.agent.id,
            name: self.agent.name.to_string(),
            scope: self.agent.name.scope.clone(),
            owner: self.agent.security_attributes.owner_id,
            personality_id: self.agent.personality_id,
            personality: personality.map(|personality| personality.name.to_string()),
            can_change: self.may_change().await,
            can_delete: self.may_delete().await,
        })
    }
}
