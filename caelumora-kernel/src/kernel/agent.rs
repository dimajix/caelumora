//! `AgentRuntime`: the kernel's implementation of the interface it gives the actor of an agent
//! (`caelumora_api::runtime::ActorRuntime`, issue 516).

use super::{Kernel, ParticipantRuntime};
use crate::agent::state::AgentStateHandle;
use crate::component::BuildContext;
use crate::controller::{AgentController, PersonalityController};
use crate::view::{Viewer, Views};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_api::component::ComponentContainer;
use caelumora_api::component::ComponentResult;
use caelumora_api::component::Face;
use caelumora_api::error::AttachmentResult;
use caelumora_api::reader::{ChannelReader, SessionReader, ViewResult};
use caelumora_api::runtime::{ActorRuntime, CallPermit, OwnRecord};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentState, Personality};
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId, RuleDecision};
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::content::ContentRef;
use caelumora_model::context::ModelProfile;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::memory::{MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::secret::Secrets;
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreResult;
use caelumora_model::tool::{ToolCall, ToolMedia};
use caelumora_model::type_map::{ErasedArc, TypeKey};
use caelumora_model::value::time_zone::TimeZone;
use caelumora_security::confidentiality::ConfidentialityQuestions;
use caelumora_security::{ActionType, Enforcement, Reclassified, SecurityContext};
use chrono::Utc;
use std::fmt::Debug;
use std::sync::Arc;

/// The `ActorRuntime` of an agent in its channel (issue 516): its participant's runtime, which
/// builds the context it acts in, with the session's controller of the agent, the instance's of its
/// personality, and the agent's state - shared with its actors in other channels and sessions
/// (issue 62). What only an agent asks of the kernel it answers itself; what every participant
/// needs it asks of its participant's runtime.
#[derive(Debug)]
pub struct AgentRuntime {
    /// The kernel of the process, whose services it answers by - held, as its channel's runtime
    /// holds it.
    kernel: Arc<Kernel>,
    participant: ParticipantRuntime,
    agent: Arc<AgentController>,
    personality: Arc<PersonalityController>,
    state: Arc<AgentStateHandle>,
}

impl AgentRuntime {
    /// The runtime of the agent `agent` holds, of the personality `personality` holds, taking
    /// part in its channel as `participant`, with its own state as the process holds it, shared
    /// with its actors elsewhere.
    ///
    /// # Errors
    /// If its state cannot be loaded.
    pub(crate) async fn new(
        participant: ParticipantRuntime,
        agent: Arc<AgentController>,
        personality: Arc<PersonalityController>,
    ) -> Result<Self> {
        let kernel = Arc::clone(participant.kernel());
        let attributes = agent.load().security_attributes.clone();
        let state = kernel.agent_states().get(agent.id(), attributes).await?;
        Ok(Self { kernel, participant, agent, personality, state })
    }

    /// The context the agent acts in now, for `initiator` (`ParticipantRuntime::acting_for`).
    fn acting_for(&self, initiator: ActorId) -> SecurityContext {
        self.participant.acting_for(initiator)
    }

    /// The views a piece of work of the agent reads through, for `initiator`, from its channel at
    /// `classification`: in its context, from its place. `None` while the channel is still being
    /// built or once it or its session is gone.
    fn views_of_work(&self, initiator: ActorId, classification: Classification) -> Option<Views> {
        let session_loop = self.participant.channel_loop()?.session_loop()?;
        let channel = self.participant.channel_id;
        let from = Place { channel, classification, session: session_loop.privacy() };
        Some(session_loop.views(Viewer::from_place(self.acting_for(initiator), from)))
    }

    /// The extension of the type `key` names, bound as `face`, that a plugin or the kernel binds to
    /// the agent's work for `initiator` at `classification` (`KernelBuilder::extend_readers`,
    /// `extend_writers`): the service, acting as `session` reads. Its session is reached as
    /// `session` reaches it - asking `GetSession` - and not handed out. `None` if none is bound as
    /// that face, if it is not offered there, if the agent may not read its session, or while the
    /// channel is not attached to it.
    async fn extension_of(
        &self,
        face: Face,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc> {
        let views = self.views_of_work(initiator, classification)?;
        let session = views.session(self.participant.session_id).await.ok()?;
        self.kernel.extensions().made(face, key, session).await
    }

    /// The rule of a place that decides `call` now, if one does, with what it decides of it: the
    /// strongest of the system's, the session's and the channel's.
    fn place_rule(&self, call: &ToolCall) -> Option<(ApprovalRuleId, RuleDecision)> {
        let session = self.participant.session_controller().load();
        let channel = self.participant.channel_controller().load();
        let system = self.kernel.approval_rules().iter();
        let rules = system.chain(&session.approval_rules).chain(&channel.approval_rules);
        ApprovalRule::decide(rules, call, session.id, channel.id, Utc::now())
            .map(|(rule, decision)| (rule.id, decision))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentRuntime {
    /// The runtime of the agent that `participant` is the runtime of, with `state` as its own - in
    /// a test: of the agent as its channel resolved it where it takes part
    /// (`participant_runtime_in`), else of an agent "agent" of a plain personality, of the id of
    /// its actor.
    ///
    /// # Panics
    /// If `participant` is no agent's: the kernel makes the runtime of an agent alone.
    pub fn in_test(participant: ParticipantRuntime, state: Arc<AgentStateHandle>) -> Self {
        use super::ActingKind;
        use crate::channel::ResolvedKind;
        let (agent, personality) = match &participant.acting.0 {
            ActingKind::Participant { participant: resolved, .. } => match resolved.kind() {
                ResolvedKind::Agent(agent, personality) => {
                    (Arc::clone(agent), Arc::clone(personality))
                }
                ResolvedKind::User(_) | ResolvedKind::Approver(_) => {
                    panic!("{} takes part as no agent", participant.actor)
                }
            },
            ActingKind::Itself { .. } | ActingKind::Given(_) => Self::made_up(&participant),
        };
        Self { kernel: Arc::clone(participant.kernel()), participant, agent, personality, state }
    }

    /// This runtime, of an agent of `personality` - in a test of what a personality decides.
    pub fn with_personality(self, personality: Personality) -> Self {
        Self { personality: Arc::new(PersonalityController::new(personality)), ..self }
    }

    /// The controllers of an agent "agent" of the id of `participant`'s actor, of a plain
    /// personality, owned by a user of its own.
    ///
    /// # Panics
    /// If `participant`'s actor is no agent.
    #[expect(clippy::expect_used, reason = "test code: what fails here fails the test")]
    fn made_up(
        participant: &ParticipantRuntime,
    ) -> (Arc<AgentController>, Arc<PersonalityController>) {
        use caelumora_model::agent::testing::make_personality;
        use caelumora_model::scope::Scope;
        use caelumora_model::user::UserId;
        use caelumora_model::value::name::ScopedName;
        let id = participant.actor.as_agent_id().expect("the runtime of an agent");
        let personality = make_personality("plain");
        let name =
            ScopedName::new("agent", Scope::GlobalScope).expect("a test's name of an agent is one");
        let owner = ActorId::User(UserId::new());
        let agent = Agent { id, ..Agent::new(name, personality.id, owner) };
        let stores = participant.kernel().entity_stores();
        let agent = AgentController::new(agent, None, Arc::new(()), stores);
        (Arc::new(agent), Arc::new(PersonalityController::new(personality)))
    }
}

#[async_trait]
impl ActorRuntime for AgentRuntime {
    fn actor_id(&self) -> ActorId {
        self.participant.actor
    }

    fn session_id(&self) -> SessionId {
        self.participant.session_id
    }

    fn channel_id(&self) -> ChannelId {
        self.participant.channel_id
    }

    fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent.load())
    }

    fn personality(&self) -> Arc<Personality> {
        Arc::clone(&self.personality.load())
    }

    fn channel_classification(&self) -> Classification {
        self.participant.channel().classification.clone()
    }

    fn session_time_zone(&self) -> TimeZone {
        self.participant.session_controller().load().effective_time_zone()
    }

    fn announce_channel_state_changed(&self) {
        let Some(channel) = self.participant.channel_loop() else { return };
        let address = (self.participant.session_id, self.participant.channel_id);
        let changed = GlobalEvent::custom::<Channel>(
            &address,
            channel::AGENT_STATE_CHANGED,
            &self.agent.id(),
        );
        self.kernel.event_bus().announce(changed, &*channel.channel());
    }

    async fn load_channel_state(&self) -> StoreResult<AgentChannelState> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        self.kernel.state_store().get_agent_channel_state(agent, channel).await
    }

    async fn store_channel_state(&self, state: &mut AgentChannelState) -> StoreResult<()> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        self.kernel.state_store().put_agent_channel_state(agent, channel, state).await?;
        state.keep_window();
        Ok(())
    }

    async fn transcript_message(&self, id: MessageId) -> StoreResult<Option<Message>> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        self.kernel.transcript_store().message(agent, channel, id).await
    }

    fn agent_state(&self) -> Arc<AgentState> {
        self.state.current()
    }

    fn reserve_memory_id(&self) -> MemoryId {
        self.state.reserve_memory_id()
    }

    async fn apply_agent_state(
        &self,
        changes: Vec<(MemoryChange, MemoryOrigin)>,
        after: MessageId,
    ) -> StoreResult<()> {
        self.state.apply(changes, after).await
    }

    async fn load_attachments(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>> {
        // Asking nothing: what lies in the agent's transcript, it heard.
        self.kernel.attachment_manager().load(messages, profile).await
    }

    async fn create_media(
        &self,
        initiator: ActorId,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        let context = &self.acting_for(initiator);
        let channel = self.participant.channel();
        let attachments = self.kernel.attachment_manager();
        attachments.create_media(context, &channel, classification, media).await
    }

    async fn copy_attachment(
        &self,
        initiator: ActorId,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        let context = &self.acting_for(initiator);
        let channel = self.participant.channel();
        self.kernel.attachment_manager().copy(context, &channel, attachment).await
    }

    async fn session(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn SessionReader>>> {
        let Some(views) = self.views_of_work(initiator, classification) else { return Ok(None) };
        let session = views.session(self.participant.session_id).await?;
        Ok(Some(Box::new(session)))
    }

    async fn channel(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn ChannelReader>>> {
        let Some(session) = self.session(initiator, classification).await? else { return Ok(None) };
        session.channel(self.participant.channel_id).await.map(Some)
    }

    async fn admit_tool_call(&self, initiator: ActorId, call: &ToolCall) -> CallPermit {
        let context = &self.acting_for(initiator);
        let call_tool = ActionType::CallTool(call);
        if self.kernel.access_policy().require(context, call_tool, |_| ()).await.is_err() {
            return CallPermit::Forbidden;
        }
        match self.place_rule(call) {
            Some((rule, RuleDecision::Deny)) => CallPermit::Denied(rule),
            Some((_, RuleDecision::Ask)) => CallPermit::Ask,
            Some((_, RuleDecision::Allow)) | None => CallPermit::Open,
        }
    }

    fn rule_denying(&self, call: &ToolCall) -> Option<ApprovalRuleId> {
        match self.place_rule(call)? {
            (rule, RuleDecision::Deny) => Some(rule),
            (_, RuleDecision::Ask | RuleDecision::Allow) => None,
        }
    }

    fn address_questions(
        &self,
        initiator: ActorId,
        questions: Questions,
    ) -> Option<Result<Questions, QuestionError>> {
        let context = &self.acting_for(initiator);
        Some(self.participant.channel_loop()?.address_questions(questions, context))
    }

    fn may_publish(
        &self,
        initiator: ActorId,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        let context = &self.acting_for(initiator);
        self.kernel.confidentiality().may_publish(context, classification, into)
    }

    fn may_copy_information(&self, initiator: ActorId, from: &Place, into: &Place) -> bool {
        let context = &self.acting_for(initiator);
        self.kernel.confidentiality().may_copy_information(context, from, into)
    }

    async fn may_reclassify(
        &self,
        initiator: ActorId,
        record: OwnRecord,
        classification: &Classification,
        within: &Classification,
    ) -> bool {
        let context = &self.acting_for(initiator);
        let author = self.participant.actor;
        let attributes = self.participant.channel().security_attributes.clone();
        let target = match record {
            OwnRecord::Note => Reclassified::Note { author, attributes: &attributes },
            OwnRecord::Memory => Reclassified::Memory { author, attributes: &attributes },
        };
        let reclassify = ActionType::Reclassify(target);
        self.kernel.access_policy().require(context, reclassify, |_| ()).await.is_ok()
            && self
                .kernel
                .confidentiality()
                .require_publish(context, classification, within, || ())
                .is_ok()
    }

    fn secrets(&self, initiator: ActorId) -> Arc<dyn Secrets> {
        let context = self.acting_for(initiator);
        Arc::new(self.kernel.secret_manager().for_context(context))
    }

    fn prompts(&self) -> &Arc<PromptCatalogue> {
        self.kernel.prompts()
    }

    async fn build_components(
        &self,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        facets: &[TypeKey],
    ) -> ComponentResult<ComponentContainer> {
        let registry = self.kernel.component_registry();
        // In the agent's own name, as the kernel holds it at each call (issues 321, 520).
        let acting = self.participant.acting.clone();
        let context = BuildContext::acting_as(acting, self.kernel.services().clone());
        registry.build_with_defaults(config, defaults, facets, &context).await
    }

    async fn reader_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc> {
        self.extension_of(Face::Reader, initiator, classification, key).await
    }

    async fn writer_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc> {
        self.extension_of(Face::Writer, initiator, classification, key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ChannelRuntime;
    use crate::session::testing::test_kernel;
    use caelumora_model::actor::ActorState;
    use caelumora_model::agent::AgentId;
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::session::testing::bare_session;

    /// What an agent keeps in its channel it keeps under its own id, and reads back; another
    /// agent of the channel reads its own.
    #[tokio::test]
    async fn an_agent_s_runtime_keeps_its_state_in_its_channel() {
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let channel = ChannelRuntime::outside_a_session(test_kernel(), session, channel);
        let agent_in_channel = |channel: &ChannelRuntime| {
            let participant = channel.new_participant_runtime(ActorId::Agent(AgentId::new()));
            AgentRuntime::in_test(participant, AgentStateHandle::detached())
        };
        let its = agent_in_channel(&channel);
        let mut asleep =
            AgentChannelState { processing_state: ActorState::Sleeping, ..Default::default() };

        its.store_channel_state(&mut asleep).await.expect("its own state in its channel");
        let loaded = its.load_channel_state().await.expect("and back");
        assert_eq!(loaded.processing_state, ActorState::Sleeping, "as it was stored");
        let other = agent_in_channel(&channel).load_channel_state().await.expect("its own");
        assert_eq!(other.processing_state, ActorState::Idle, "not the first agent's");
    }
}
