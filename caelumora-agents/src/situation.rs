//! The situation of a piece of work of an agent, to read (issues 204,
//! 399): where it stands and what it sees from there - who works, for whom
//! and bound by what, what it has seen so far, the agent's states,
//! and its session to navigate. A piece of work is a turn of an agent run
//! here or an external agent's exchange with its remote agent - both a
//! `Work` in the model; a [`Situation`] is what both kinds have. What only a
//! turn has - staged changes, tool calls, the plan, what it says and does in
//! its channel - is `Turn`'s, which reads through its situation
//! (`Turn::situation`) and passes none of it on.
//!
//! Components that only read - a `PromptProvider`, a `ToolProvider` asked
//! what it offers - are given a `Situation` rather than a `Turn`, so that
//! they serve both kinds of agent.

use caelumora_kernel::api::reader::{ChannelReader, SessionReader, ViewResult};
use caelumora_kernel::api::runtime::ActorRuntime;
use caelumora_model::actor::{ActorId, RequestReason};
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::context::{ContextResolver, ModelProfile};
use caelumora_model::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_model::secret::Secrets;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::tool::ToolSet;
use caelumora_model::value::time_zone::TimeZone;
use caelumora_model::work::Work;
use std::sync::Arc;

/// The situation of the work under way of an agent in its channel, to
/// read.
#[derive(Debug, Clone, Copy)]
pub struct Situation<'a> {
    agent_id: AgentId,
    work: &'a Work,
    agent_channel_state: &'a AgentChannelState,
    /// The runtime of the agent's actor: what it asks of its channel, what
    /// it navigates, and the agent's own state.
    runtime: &'a dyn ActorRuntime,
}

impl<'a> Situation<'a> {
    /// `work` of the agent of `runtime`, with its state in the channel, read
    /// through the actor's `runtime`.
    pub(crate) fn new(
        work: &'a Work,
        agent_channel_state: &'a AgentChannelState,
        runtime: &'a dyn ActorRuntime,
    ) -> Self {
        Self { agent_id: runtime.agent().id, work, agent_channel_state, runtime }
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

    /// Why the work was asked for (issue 439): the reason of the request
    /// to respond it answers, `None` if no request began it.
    pub fn asked(&self) -> Option<RequestReason> {
        self.work.asked
    }

    /// Who began the chain the work answers (`EventContext::initiator_id`, issue 432): what the
    /// kernel is told with every call it is asked in the work's name, for the record - never for
    /// rights, which the kernel builds itself (issue 516).
    pub fn initiator(&self) -> ActorId {
        self.work.initiator()
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
    /// classification the work began with (issue 67). `None` while it is not
    /// attached to its channel, or once the session is gone.
    ///
    /// # Errors
    /// As `ActorRuntime::session`: the agent may not read it, or reading
    /// it failed.
    pub async fn session(&self) -> ViewResult<Option<Box<dyn SessionReader>>> {
        self.runtime.session(self.initiator(), self.work.classification.clone()).await
    }

    /// The reader `T` a plugin or the kernel binds to the work (`ActorRuntime::reader`, issue 516):
    /// what of the plugin's service the work reads by - the skills' `SkillReader`, say -, in its
    /// context. Asked for where it is used and not kept: it acts in the context of the call. `None`
    /// if none is bound, if it is not offered here, or where the work reaches no session or may not
    /// read it. What acts is the turn's (`Turn::writer`).
    pub async fn reader<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.runtime.reader(self.initiator(), self.work.classification.clone()).await
    }

    /// Whether a turn of this work would be given the writer `T` bound to it (`Turn::writer`):
    /// what a tool provider asks before it offers what acts by it (issue 516), handed nothing.
    pub async fn has_writer<T: ?Sized + Send + Sync + 'static>(&self) -> bool {
        let classification = self.work.classification.clone();
        self.runtime.writer::<T>(self.initiator(), classification).await.is_some()
    }

    /// The agent's channel as the work reads it from there (`ActorRuntime::channel`, issue 399):
    /// its participants, its notes, at the classification the work began with. `None` where the
    /// work reaches no session.
    ///
    /// # Errors
    /// What following the views to it reports.
    pub async fn channel(&self) -> ViewResult<Option<Box<dyn ChannelReader>>> {
        self.runtime.channel(self.initiator(), self.work.classification.clone()).await
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
    ///
    /// # Errors
    /// `PromptError` if a text it is given cannot be had.
    pub fn token_positions(&self) -> Result<Option<(Vec<u64>, bool)>, PromptError> {
        ContextResolver::of(self.agent_channel_state, self.runtime.prompts()).token_positions(
            &self.work.context,
            self.work.turn_id,
            ActorId::Agent(self.agent_id),
        )
    }

    /// How the agent's model is given its context (issue 135): what a
    /// prompt part tells of how what it is given is marked.
    pub fn profile(&self) -> ModelProfile {
        self.work.context.frame.profile
    }

    /// The texts the work words for its model itself (issue 57) - the
    /// results of tool calls, a plan's instructions -, in the variant its
    /// profile names: its model's, or the one a turn resumed was begun with.
    pub fn texts(&self) -> PromptTexts {
        self.profile().texts(self.runtime.prompts())
    }

    /// The time zone the work gives times in (issue 424): its session's as
    /// it began, recorded with its context.
    pub fn time_zone(&self) -> TimeZone {
        self.work.context.frame.time_zone
    }

    /// The tool sets the work began with, as its model is offered them
    /// (issue 194) - kept in the agent's state by the digest its context
    /// names. None for work that offers no tools, e.g. an external agent's.
    pub fn tool_sets(&self) -> &'a [ToolSet] {
        let state = self.agent_channel_state;
        self.work
            .context
            .frame
            .tools
            .and_then(|digest| state.offered_tools.get(&digest))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The agent's own state as it is now (issue 62), shared by all its actors
    /// (`ActorRuntime::agent_state`): a snapshot, which changes applied after do not change.
    pub fn agent_state(&self) -> Arc<AgentState> {
        self.runtime.agent_state()
    }

    /// The secrets that may be read in the work's name (`ActorRuntime::secrets`, issue 516): what
    /// a component that talks to a remote system resolves its credentials by.
    pub fn secrets(&self) -> Arc<dyn Secrets> {
        self.runtime.secrets(self.initiator())
    }

    /// Whether what lies at `from` may be copied, in the work's name, to `into`
    /// (`ActorRuntime::may_copy_information`, issue 516): what the agent may recall of what it
    /// keeps of its own.
    pub fn may_copy_information(&self, from: &Place, into: &Place) -> bool {
        self.runtime.may_copy_information(self.initiator(), from, into)
    }
}
