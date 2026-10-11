//! A piece of work of an agent as what is asked during it reads it (issue
//! 204): the turn of an agent run here, or an external agent's exchange with
//! its remote agent - both a `Work` in the model. [`AgentWork`] is the part
//! both kinds of agent have: who works, in which contexts and bound by what,
//! the agent's states to read from, and its session to navigate. What only a
//! turn has - staged changes, tool calls, the plan - is `Turn`'s.
//!
//! Components that only read - a `PromptProvider` - are given an
//! `AgentWork` rather than a `Turn`, so that they serve both kinds of agent.

use crate::agent::state::AgentStateHandle;
use crate::view::{SessionView, ViewsFactory};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::ModelProfile;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::ToolSet;
use caelumora_model::work::Work;
use std::sync::Arc;

/// The work under way of an agent in its channel, to read.
#[derive(Debug, Clone, Copy)]
pub struct AgentWork<'a> {
    agent_id: AgentId,
    work: &'a Work,
    agent_channel_state: &'a AgentChannelState,
    agent_state: &'a Arc<AgentStateHandle>,
    /// What it navigates: the agent's views. `None` for work outside of a
    /// channel - in tests - which reads nothing elsewhere.
    views: Option<&'a ViewsFactory>,
}

impl<'a> AgentWork<'a> {
    /// `work` of the agent `agent_id`, with its state in the channel and its
    /// own state, navigating `views`.
    pub(crate) fn new(
        agent_id: AgentId,
        work: &'a Work,
        agent_channel_state: &'a AgentChannelState,
        agent_state: &'a Arc<AgentStateHandle>,
        views: Option<&'a ViewsFactory>,
    ) -> Self {
        Self { agent_id, work, agent_channel_state, agent_state, views }
    }

    /// The agent whose work this is.
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    /// The agent's channel: where the envelope the work answers was routed.
    pub fn channel_id(&self) -> ChannelId {
        self.work.event_context.channel_id
    }

    /// The session of the agent's channel, as `channel_id`.
    pub fn session_id(&self) -> SessionId {
        self.work.event_context.session_id
    }

    /// Whose authority the work acts with.
    pub fn security_context(&self) -> &'a SecurityContext {
        &self.work.security_context
    }

    /// The classification of the agent's channel as it was when the work
    /// began: the most it may read, and the most it writes as.
    pub fn classification(&self) -> Classification {
        self.work.classification.clone()
    }

    /// The classification of what the work writes (issue 168): the join of
    /// everything it has seen so far, at most `classification`.
    pub fn label(&self) -> Classification {
        self.work.label()
    }

    /// The agent's session as the work reads it from its channel (issue
    /// 163): the root of what it may read elsewhere - the other channels'
    /// notes and histories, its own private notes there - at the
    /// classification the work began with (issue 67). `None` for work
    /// outside of a channel, which reads nothing elsewhere.
    pub async fn session(&self) -> Option<SessionView> {
        self.views?.session(&self.work.security_context, self.channel_id(), self.work.classification.clone()).await
    }

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes a turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &'a AgentChannelState {
        self.agent_channel_state
    }

    /// How the agent's model is given its context (issue 135): what a
    /// prompt part tells of how what it is given is marked.
    pub fn profile(&self) -> ModelProfile {
        self.work.context.profile
    }

    /// The tool sets the work began with, as its model is offered them
    /// (issue 194) - kept in the agent's state by the digest its context
    /// names. None for work that offers no tools, e.g. an external agent's.
    pub fn tool_sets(&self) -> &'a [ToolSet] {
        let state = self.agent_channel_state;
        self.work.context.tools.and_then(|digest| state.offered_tools.get(&digest)).map(Vec::as_slice).unwrap_or_default()
    }

    /// The agent's own state (issue 62), shared by all its actors.
    pub fn agent_state(&self) -> &'a Arc<AgentStateHandle> {
        self.agent_state
    }
}
