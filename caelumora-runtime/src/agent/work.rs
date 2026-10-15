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
use crate::terminal::Terminal;
use crate::runtime::ActorRuntime;
use crate::view::SessionView;
use crate::workspace::WorkspaceResult;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::{ContextResolver, ModelProfile};
use caelumora_model::security::{Grant, SecurityAttributes, SecurityContext};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::ToolSet;
use caelumora_model::work::Work;
use caelumora_model::workspace::Workspace;
use std::sync::Arc;

/// The work under way of an agent in its channel, to read.
#[derive(Debug, Clone, Copy)]
pub struct AgentWork<'a> {
    agent_id: AgentId,
    work: &'a Work,
    agent_channel_state: &'a AgentChannelState,
    agent_state: &'a Arc<AgentStateHandle>,
    /// The runtime of the agent's actor: what it asks of its channel, and
    /// what it navigates.
    runtime: &'a ActorRuntime,
}

impl<'a> AgentWork<'a> {
    /// `work` of the agent `agent_id`, with its state in the channel and its
    /// own state, read through the actor's `runtime`.
    pub(crate) fn new(
        agent_id: AgentId,
        work: &'a Work,
        agent_channel_state: &'a AgentChannelState,
        agent_state: &'a Arc<AgentStateHandle>,
        runtime: &'a ActorRuntime,
    ) -> Self {
        Self { agent_id, work, agent_channel_state, agent_state, runtime }
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

    /// The attributes of the agent's channel as it stands - the system's
    /// while it is attached to none -, by which an act on what the agent
    /// keeps there is asked (issue 312).
    pub fn channel_attributes(&self) -> SecurityAttributes {
        self.runtime.channel_attributes().unwrap_or_default()
    }

    /// The classification of what the work writes (issue 168): the join of
    /// everything it has seen so far, at most `classification`.
    pub fn label(&self) -> Classification {
        self.work.label()
    }

    /// The agent's session as the work reads it from its channel (issue
    /// 163): the root of what it may read elsewhere - the other channels'
    /// notes and histories, its own private notes there - at the
    /// classification the work began with (issue 67). `None` while it is not
    /// attached to its channel, once the session is gone, or if the agent
    /// may not read it.
    pub async fn session(&self) -> Option<SessionView> {
        self.runtime.session_view(&self.work.security_context, self.work.classification.clone()).await
    }

    /// The workspace of the agent's session and its terminal, to read its
    /// files, if the work may use it in its channel
    /// (`ActorRuntime::workspace_terminal`, issue 344) - what a component
    /// that only reads looks at. Changing the files, and running commands,
    /// is a turn's (`Turn::workspace_terminal`, `Turn::workspace_shell`).
    ///
    /// # Errors
    /// As `ActorRuntime::workspace_terminal`.
    pub async fn workspace_for_reading(&self) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        self.runtime.workspace_terminal(&self.work.security_context, Grant::Read).await
    }

    /// The agent's state in this channel as it stands: what components read
    /// their state from (issue 65). Changes a turn made count only once it
    /// completes.
    pub fn agent_channel_state(&self) -> &'a AgentChannelState {
        self.agent_channel_state
    }

    /// Where each entry of the agent's transcript stands in what its model
    /// is given next, in tokens, and whether a call measured any of it
    /// (`ContextResolver::token_positions`, issue 284): how full its context
    /// is, and where it may be cut. `None` if the work's context cannot be
    /// resolved.
    pub fn token_positions(&self) -> Option<(Vec<u64>, bool)> {
        ContextResolver::of(self.agent_channel_state).token_positions(&self.work.context, self.work.turn_id, ActorId::Agent(self.agent_id))
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
