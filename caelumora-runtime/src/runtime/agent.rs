//! `ActorRuntime`: the interface the core gives the actor of an agent of an application (issue
//! 516) - and `AgentRuntime`, the core's implementation of it.

use super::{CallPermit, Face, OwnRecord, ParticipantRuntime};
use crate::agent::state::AgentStateHandle;
use crate::attachment::AttachmentResult;
use crate::component::{ComponentContainer, ComponentResult};
use crate::controller::{AgentController, PersonalityController};
use crate::view::{SessionReader, ViewResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentState, Personality};
use caelumora_model::approval::ApprovalRuleId;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::content::ContentRef;
use caelumora_model::context::ModelProfile;
use caelumora_model::memory::{MemoryChange, MemoryId, MemoryOrigin};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::secret::Secrets;
use caelumora_model::tool::{ToolCall, ToolMedia};
use caelumora_model::type_map::{ErasedArc, TypeKey};
use caelumora_model::value::time_zone::TimeZone;
use std::fmt::Debug;
use std::sync::Arc;

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

    /// The agent's channel as it stands - as it stood last once the channel is gone (issue 374).
    fn channel(&self) -> Arc<Channel>;

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

/// The `ActorRuntime` of an agent in its channel (issue 516): its participant's runtime, with
/// the session's controller of the agent, the instance's of its personality, and the agent's
/// state - shared with its actors in other channels and sessions (issue 62).
#[derive(Debug)]
pub struct AgentRuntime {
    runtime: ParticipantRuntime,
    agent: Arc<AgentController>,
    personality: Arc<PersonalityController>,
    state: Arc<AgentStateHandle>,
}

impl AgentRuntime {
    /// The runtime of the agent `agent` holds, of the personality `personality` holds, running
    /// with `runtime` in its channel, with its own state as the process holds it.
    ///
    /// # Errors
    /// If its state cannot be loaded.
    pub(crate) async fn new(
        runtime: ParticipantRuntime,
        agent: Arc<AgentController>,
        personality: Arc<PersonalityController>,
    ) -> Result<Self> {
        let attributes = agent.load().security_attributes.clone();
        let state = runtime.agent_state(attributes).await?;
        Ok(Self { runtime, agent, personality, state })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentRuntime {
    /// The runtime of the agent `runtime` is made for, with `state` as its own - in a test: of
    /// the agent as its channel resolved it where it takes part (`participant_runtime_in`), else
    /// of an agent "agent" of a plain personality, of the id of `runtime`'s actor.
    ///
    /// # Panics
    /// If `runtime` is no agent's: the core makes the runtime of an agent alone.
    pub fn in_test(runtime: ParticipantRuntime, state: Arc<AgentStateHandle>) -> Self {
        use super::actor::Acting;
        use crate::channel::ResolvedKind;
        let (agent, personality) = match &runtime.acting {
            Acting::Participant(resolved) => match resolved.kind() {
                ResolvedKind::Agent(agent, personality) => {
                    (Arc::clone(agent), Arc::clone(personality))
                }
                ResolvedKind::User(_) | ResolvedKind::Approver(_) => {
                    panic!("{} takes part as no agent", runtime.actor_id())
                }
            },
            Acting::Itself | Acting::Fixed(_) => Self::made_up(&runtime),
        };
        Self { runtime, agent, personality, state }
    }

    /// This runtime, of an agent of `personality` - in a test of what a personality decides.
    pub fn with_personality(self, personality: Personality) -> Self {
        Self { personality: Arc::new(PersonalityController::new(personality)), ..self }
    }

    /// The controllers of an agent "agent" of the id of `runtime`'s actor, of a plain personality,
    /// owned by a user of its own.
    ///
    /// # Panics
    /// If `runtime`'s actor is no agent.
    #[expect(clippy::expect_used, reason = "test code: what fails here fails the test")]
    fn made_up(runtime: &ParticipantRuntime) -> (Arc<AgentController>, Arc<PersonalityController>) {
        use caelumora_model::agent::testing::make_personality;
        use caelumora_model::scope::Scope;
        use caelumora_model::user::UserId;
        use caelumora_model::value::name::ScopedName;
        let id = runtime.actor_id().as_agent_id().expect("the runtime of an agent");
        let personality = make_personality("plain");
        let name =
            ScopedName::new("agent", Scope::GlobalScope).expect("a test's name of an agent is one");
        let owner = ActorId::User(UserId::new());
        let agent = Agent { id, ..Agent::new(name, personality.id, owner) };
        let stores = runtime.runtime.runtime_manager.entity_stores();
        let agent = AgentController::new(agent, None, Arc::new(()), stores);
        (Arc::new(agent), Arc::new(PersonalityController::new(personality)))
    }
}

#[async_trait]
impl ActorRuntime for AgentRuntime {
    fn actor_id(&self) -> ActorId {
        self.runtime.actor_id()
    }

    fn channel_id(&self) -> ChannelId {
        self.runtime.channel_id
    }

    fn agent(&self) -> Arc<Agent> {
        Arc::clone(&self.agent.load())
    }

    fn personality(&self) -> Arc<Personality> {
        Arc::clone(&self.personality.load())
    }

    fn channel(&self) -> Arc<Channel> {
        self.runtime.channel()
    }

    fn channel_classification(&self) -> Classification {
        self.runtime.channel_classification()
    }

    fn session_time_zone(&self) -> TimeZone {
        self.runtime.session_time_zone()
    }

    fn announce_channel_state_changed(&self) {
        self.runtime.announce_channel_state_changed();
    }

    async fn load_channel_state(&self) -> Result<AgentChannelState> {
        self.runtime.load_channel_state().await
    }

    async fn store_channel_state(&self, state: &mut AgentChannelState) -> Result<()> {
        self.runtime.store_channel_state(state).await
    }

    async fn transcript_message(&self, id: MessageId) -> Result<Option<Message>> {
        self.runtime.transcript_message(id).await
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
        self.runtime.load_attachments(messages, profile).await
    }

    async fn create_media(
        &self,
        initiator: ActorId,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        self.runtime.create_media(initiator, classification, media).await
    }

    async fn copy_attachment(
        &self,
        initiator: ActorId,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        self.runtime.copy_attachment(initiator, attachment).await
    }

    async fn session(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn SessionReader>>> {
        self.runtime.session(initiator, classification).await
    }

    async fn admit_tool_call(&self, initiator: ActorId, call: &ToolCall) -> CallPermit {
        self.runtime.admit_tool_call(initiator, call).await
    }

    fn rule_denying(&self, call: &ToolCall) -> Option<ApprovalRuleId> {
        self.runtime.rule_denying(call)
    }

    fn address_questions(
        &self,
        initiator: ActorId,
        questions: Questions,
    ) -> Option<Result<Questions, QuestionError>> {
        self.runtime.address_questions(initiator, questions)
    }

    fn may_publish(
        &self,
        initiator: ActorId,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        self.runtime.may_publish(initiator, classification, into)
    }

    fn may_copy_information(&self, initiator: ActorId, from: &Place, into: &Place) -> bool {
        self.runtime.may_copy_information(initiator, from, into)
    }

    async fn may_reclassify(
        &self,
        initiator: ActorId,
        record: OwnRecord,
        classification: &Classification,
        within: &Classification,
    ) -> bool {
        self.runtime.may_reclassify(initiator, record, classification, within).await
    }

    fn secrets(&self, initiator: ActorId) -> Arc<dyn Secrets> {
        self.runtime.secrets(initiator)
    }

    fn prompts(&self) -> &Arc<PromptCatalogue> {
        self.runtime.prompts()
    }

    async fn build_components(
        &self,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        facets: &[TypeKey],
    ) -> ComponentResult<ComponentContainer> {
        let registry = self.runtime.component_registry();
        let context = self.runtime.component_context();
        ComponentContainer::with_defaults(config, defaults, facets, registry, &context).await
    }

    async fn reader_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc> {
        self.runtime.extension_of(Face::Reader, initiator, classification, key).await
    }

    async fn writer_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc> {
        self.runtime.extension_of(Face::Writer, initiator, classification, key).await
    }
}
