//! `ActorRuntime`: the interface the core gives the actor of an agent of an application (issue
//! 516) - and `AgentRuntime`, the core's implementation of it.

use super::{Face, ParticipantRuntime, RuntimeManager};
use crate::agent::state::AgentStateHandle;
use crate::attachment::AttachmentResult;
use crate::component::{ComponentContainer, ComponentContext, ComponentResult};
use crate::controller::{AgentController, PersonalityController};
use crate::view::{ChannelReader, SessionReader, ViewResult, Viewer, Views};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentState, Personality};
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId, RuleDecision};
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, ConfidentialityQuestions, Place};
use caelumora_model::content::ContentRef;
use caelumora_model::context::ModelProfile;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::memory::{MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::secret::Secrets;
use caelumora_model::security::{ActionType, Enforcement, Reclassified, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::tool::{ToolCall, ToolMedia};
use caelumora_model::type_map::{ErasedArc, TypeKey};
use caelumora_model::value::time_zone::TimeZone;
use chrono::Utc;
use std::fmt::Debug;
use std::sync::Arc;

/// What the core decides of a tool call as it is admitted (`ActorRuntime::admit_tool_call`, issue
/// 516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPermit {
    /// The policy does not let it be called (`CallTool`).
    Forbidden,
    /// A rule of a place denies it.
    Denied(ApprovalRuleId),
    /// A rule of a place asks a person to decide it, whatever its tool says (issue 13).
    Ask,
    /// Nothing of the core stands against it: whether a person is asked is its tool's.
    Open,
}

/// What an agent keeps of its own in its channel, which it may reclassify
/// (`ActorRuntime::may_reclassify`, issue 516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnRecord {
    /// One of its notes, public or private.
    Note,
    /// An entry of its memory, recalled in the channel.
    Memory,
}

/// What the actor of an agent of an application runs with (issue 516): the one interface its
/// kind is given by the core (`AgentKinds::register`). Who the agent is and its channel as they
/// are now, its state, the attachments of its messages, its session to read, what the core
/// decides of its work, and the extensions plugins bind to that work - each asked of the core in
/// the agent's name, which the core builds at every call. It hands out no controller, no store
/// and no policy.
#[async_trait]
pub trait ActorRuntime: Send + Sync + Debug {
    /// The agent as an actor.
    fn actor_id(&self) -> ActorId;

    /// The channel the agent takes part in.
    fn channel_id(&self) -> ChannelId;

    /// The agent as it is now, as its session holds it (issue 120).
    fn agent(&self) -> Arc<Agent>;

    /// The agent's personality as it is now, as the instance holds it (issue 406): how it is
    /// introduced, who sees its reasoning, what it hears during a turn.
    fn personality(&self) -> Arc<Personality>;

    /// The session of the agent's channel.
    fn session_id(&self) -> SessionId;

    /// The classification of the agent's channel as it is now, which what the agent says there is
    /// bound by (issues 141, 212).
    fn channel_classification(&self) -> Classification;

    /// The time zone of the agent's session as it is now - UTC where it names none -, which its
    /// model is given the times of messages in (issue 424).
    fn session_time_zone(&self) -> TimeZone;

    /// Announces that the agent's state in its channel changed and is stored (issue 65), without
    /// content. Nothing once the channel is gone.
    fn announce_channel_state_changed(&self);

    /// The agent's state in its channel, as stored (issue 206).
    ///
    /// # Errors
    /// If it cannot be read.
    async fn load_channel_state(&self) -> Result<AgentChannelState>;

    /// Stores the agent's `state` in its channel, and then lets go of what its history holds
    /// before its window (`AgentChannelState::keep_window`, issue 379).
    ///
    /// # Errors
    /// If it cannot be stored; nothing is let go then.
    async fn store_channel_state(&self, state: &mut AgentChannelState) -> Result<()>;

    /// The message `id` as the agent's transcript in its channel holds it, on whichever branch,
    /// with every change of it recorded since (issue 380); `None` if it holds it nowhere.
    ///
    /// # Errors
    /// If the transcript cannot be read.
    async fn transcript_message(&self, id: MessageId) -> Result<Option<Message>>;

    /// The agent's own state as it is now, shared with its actors in other channels and sessions
    /// (issue 62): a snapshot, which changes applied afterwards do not change.
    fn agent_state(&self) -> Arc<AgentState>;

    /// A number for a new entry of the agent's memory, never handed out before: reserved at once,
    /// since its actors elsewhere remember at the same time.
    fn reserve_memory_id(&self) -> MemoryId;

    /// Applies `changes` to the agent's memory as it is now, each made at its origin in a turn
    /// that ended with message `after`, stores it and announces it (issue 62).
    ///
    /// # Errors
    /// If it cannot be stored; nothing is changed then.
    async fn apply_agent_state(
        &self,
        changes: Vec<(MemoryChange, MemoryOrigin)>,
        after: MessageId,
    ) -> Result<()>;

    /// Puts their content on the attachments of `messages` - of the agent's transcript - that the
    /// model of `profile` is given along (issue 282), and answers how each is classified.
    ///
    /// # Errors
    /// `AttachmentError` if one cannot be read, or is not what its message says.
    async fn load_attachments(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>>;

    /// Keeps `media`, what a tool gave its model for a call the agent makes for `initiator`,
    /// classified `classification` as the call's result (issue 512).
    ///
    /// # Errors
    /// `AttachmentError` if it may not or cannot be kept.
    async fn create_media(
        &self,
        initiator: ActorId,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>>;

    /// `attachment`, one of the agent's channel, copied for `initiator` for a message the agent
    /// will say there (issue 282).
    ///
    /// # Errors
    /// `AttachmentError` if it may not or cannot be copied.
    async fn copy_attachment(
        &self,
        initiator: ActorId,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef>;

    /// The agent's session as its work for `initiator` reads it from its channel at
    /// `classification` (issue 163), if it may read it (`GetSession`): the root of what the work
    /// reads elsewhere. `None` while the channel is not attached to its session, or once either
    /// is gone.
    ///
    /// # Errors
    /// The agent may not read it, or reading it failed.
    async fn session(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn SessionReader>>>;

    /// The agent's own channel as its work for `initiator` reads it from there at
    /// `classification` (`session`, `SessionReader::channel`, issue 399): its participants, its
    /// notes. `None` where the work reaches no session.
    ///
    /// # Errors
    /// What following the views to it reports.
    async fn channel(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn ChannelReader>>>;

    /// How a call of `call`, made by the agent for `initiator`, is admitted (issue 516): whether
    /// the policy lets it be called, and what the strongest rule of the system, the session and
    /// the channel decides of it now (issue 13).
    async fn admit_tool_call(&self, initiator: ActorId, call: &ToolCall) -> CallPermit;

    /// The rule of a place that denies `call` now, if one does (issue 13): asked again as an
    /// admitted call is run, since one may have been set while it waited.
    fn rule_denying(&self, call: &ToolCall) -> Option<ApprovalRuleId>;

    /// `questions` put to the users of the agent's channel, their addressees resolved as the
    /// agent's context for `initiator` names them (issue 165); `None` while the channel is not
    /// running, when there is nobody to ask.
    ///
    /// # Errors
    /// An addressee the channel does not know.
    fn address_questions(
        &self,
        initiator: ActorId,
        questions: Questions,
    ) -> Option<Result<Questions, QuestionError>>;

    /// Whether what is classified `classification` may flow, in the agent's name for
    /// `initiator`, into what is classified `into` (issue 516).
    fn may_publish(
        &self,
        initiator: ActorId,
        classification: &Classification,
        into: &Classification,
    ) -> bool;

    /// Whether what lies at `from` may be copied, in the agent's name for `initiator`, to `into`
    /// (issue 516): what the agent may recall of what it keeps of its own, written elsewhere.
    fn may_copy_information(&self, initiator: ActorId, from: &Place, into: &Place) -> bool;

    /// Whether `record` - what the agent keeps of its own in its channel - may be reclassified,
    /// in its name for `initiator`, as `classification`, within what is classified `within`
    /// (issues 133, 516).
    async fn may_reclassify(
        &self,
        initiator: ActorId,
        record: OwnRecord,
        classification: &Classification,
        within: &Classification,
    ) -> bool;

    /// The secrets that may be read in the agent's name for `initiator` (issue 516), each read
    /// asked of the policy then: what a component that talks to a remote system is handed for its
    /// credentials.
    fn secrets(&self, initiator: ActorId) -> Arc<dyn Secrets>;

    /// The texts given to models (issue 57): what the agent's work words for its model itself.
    fn prompts(&self) -> &Arc<PromptCatalogue>;

    /// The components of `config` that provide `facets` (issue 337), with those of `defaults` as
    /// the defaults of the singles they provide: built by the component kinds of the process, in
    /// the agent's name as the core holds it now (issue 321).
    ///
    /// # Errors
    /// `ComponentError` if they cannot be planned or built.
    async fn build_components(
        &self,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        facets: &[TypeKey],
    ) -> ComponentResult<ComponentContainer>;

    /// The reader of the type `key` names that a plugin or the core binds to the agent's work for
    /// `initiator` at `classification` (`RuntimeBuilder::extend_readers`), its type forgotten:
    /// what `reader` asks.
    async fn reader_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc>;

    /// The writer of the type `key` names, bound as `reader_of` binds a reader
    /// (`RuntimeBuilder::extend_writers`): what `writer` asks.
    async fn writer_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc>;
}

impl dyn ActorRuntime + '_ {
    /// The reader `T` a plugin or the core binds to the agent's work for `initiator` at
    /// `classification` (issue 516): what of the plugin's service the work reads by, acting in
    /// the work's context as the session is read (`session`). `None` if none is bound, if it is
    /// not offered there, if the agent may not read its session, or while the channel is not
    /// attached to it.
    pub async fn reader<T: ?Sized + Send + Sync + 'static>(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> Option<Arc<T>> {
        let made = self.reader_of(initiator, classification, TypeKey::of::<T>()).await?;
        made.get::<T>().cloned()
    }

    /// The writer `T` a plugin or the core binds to the agent's work: what of the plugin's
    /// service the work acts by, made as `reader` makes a reader.
    pub async fn writer<T: ?Sized + Send + Sync + 'static>(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> Option<Arc<T>> {
        let made = self.writer_of(initiator, classification, TypeKey::of::<T>()).await?;
        made.get::<T>().cloned()
    }
}

/// The `ActorRuntime` of an agent in its channel (issue 516): its participant's runtime, which
/// builds the context it acts in, with the session's controller of the agent, the instance's of
/// its personality, and the agent's state - shared with its actors in other channels and sessions
/// (issue 62). What only an agent asks of the core it answers itself; what every participant
/// needs it asks of its participant's runtime.
#[derive(Debug)]
pub struct AgentRuntime {
    /// The runtime of the process, whose services it answers by - held, as its channel's runtime
    /// holds it.
    manager: Arc<RuntimeManager>,
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
        let manager = Arc::clone(participant.runtime_manager());
        let attributes = agent.load().security_attributes.clone();
        let state = manager.agent_states().get(agent.id(), attributes).await?;
        Ok(Self { manager, participant, agent, personality, state })
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

    /// The extension of the type `key` names, bound as `face`, that a plugin or the core binds to
    /// the agent's work for `initiator` at `classification` (`RuntimeBuilder::extend_readers`,
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
        self.manager.extensions().made(face, key, session).await
    }

    /// The rule of a place that decides `call` now, if one does, with what it decides of it: the
    /// strongest of the system's, the session's and the channel's.
    fn place_rule(&self, call: &ToolCall) -> Option<(ApprovalRuleId, RuleDecision)> {
        let session = self.participant.session_controller().load();
        let channel = self.participant.channel_controller().load();
        let system = self.manager.approval_rules().iter();
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
    /// If `participant` is no agent's: the core makes the runtime of an agent alone.
    pub fn in_test(participant: ParticipantRuntime, state: Arc<AgentStateHandle>) -> Self {
        use super::actor::Acting;
        use crate::channel::ResolvedKind;
        let (agent, personality) = match &participant.acting {
            Acting::Participant(resolved) => match resolved.kind() {
                ResolvedKind::Agent(agent, personality) => {
                    (Arc::clone(agent), Arc::clone(personality))
                }
                ResolvedKind::User(_) | ResolvedKind::Approver(_) => {
                    panic!("{} takes part as no agent", participant.actor_id())
                }
            },
            Acting::Itself | Acting::Fixed(_) => Self::made_up(&participant),
        };
        Self {
            manager: Arc::clone(participant.runtime_manager()),
            participant,
            agent,
            personality,
            state,
        }
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
        let id = participant.actor_id().as_agent_id().expect("the runtime of an agent");
        let personality = make_personality("plain");
        let name =
            ScopedName::new("agent", Scope::GlobalScope).expect("a test's name of an agent is one");
        let owner = ActorId::User(UserId::new());
        let agent = Agent { id, ..Agent::new(name, personality.id, owner) };
        let stores = participant.runtime_manager().entity_stores();
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
        self.manager.event_bus().announce(changed, &*channel.channel());
    }

    async fn load_channel_state(&self) -> Result<AgentChannelState> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        Ok(self.manager.state_store().get_agent_channel_state(agent, channel).await?)
    }

    async fn store_channel_state(&self, state: &mut AgentChannelState) -> Result<()> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        self.manager.state_store().put_agent_channel_state(agent, channel, state).await?;
        state.keep_window();
        Ok(())
    }

    async fn transcript_message(&self, id: MessageId) -> Result<Option<Message>> {
        let (agent, channel) = (self.agent.id(), self.participant.channel_id);
        Ok(self.manager.transcript_store().message(agent, channel, id).await?)
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
    ) -> Result<()> {
        self.state.apply(changes, after).await
    }

    async fn load_attachments(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>> {
        // Asking nothing: what lies in the agent's transcript, it heard.
        self.manager.attachment_manager().load(messages, profile).await
    }

    async fn create_media(
        &self,
        initiator: ActorId,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        let context = &self.acting_for(initiator);
        let channel = self.participant.channel();
        let attachments = self.manager.attachment_manager();
        attachments.create_media(context, &channel, classification, media).await
    }

    async fn copy_attachment(
        &self,
        initiator: ActorId,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        let context = &self.acting_for(initiator);
        let channel = self.participant.channel();
        self.manager.attachment_manager().copy(context, &channel, attachment).await
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
        if self.manager.access_policy().require(context, call_tool, |_| ()).await.is_err() {
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
        self.manager.confidentiality().may_publish(context, classification, into)
    }

    fn may_copy_information(&self, initiator: ActorId, from: &Place, into: &Place) -> bool {
        let context = &self.acting_for(initiator);
        self.manager.confidentiality().may_copy_information(context, from, into)
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
        self.manager.access_policy().require(context, reclassify, |_| ()).await.is_ok()
            && self
                .manager
                .confidentiality()
                .require_publish(context, classification, within, || ())
                .is_ok()
    }

    fn secrets(&self, initiator: ActorId) -> Arc<dyn Secrets> {
        let context = self.acting_for(initiator);
        Arc::new(self.manager.secret_manager().for_context(context))
    }

    fn prompts(&self) -> &Arc<PromptCatalogue> {
        self.manager.prompts()
    }

    async fn build_components(
        &self,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        facets: &[TypeKey],
    ) -> ComponentResult<ComponentContainer> {
        let registry = self.manager.component_registry();
        // In the agent's own name, as the core holds it now (issue 321).
        let context = self.acting_for(self.participant.actor);
        let context = ComponentContext::new(context, self.manager.services().clone());
        ComponentContainer::with_defaults(config, defaults, facets, registry, &context).await
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
    use crate::runtime::ChannelRuntime;
    use crate::session::testing::test_runtime;
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
        let channel = ChannelRuntime::outside_a_session(test_runtime(), session, channel);
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
