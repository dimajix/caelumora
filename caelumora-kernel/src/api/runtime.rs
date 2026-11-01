//! `ActorRuntime`: the interface the kernel gives the actor of an agent, on the agent side (issue
//! 516) - who it is, its channel and its state, what it reads, and what it asks the kernel to
//! decide -, which the kernel serves (`AgentRuntime`).

use crate::api::component::ComponentContainer;
use crate::api::component::ComponentResult;
use crate::api::error::AttachmentResult;
use crate::api::reader::{ChannelReader, SessionReader, ViewResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentChannelState, AgentState, Personality};
use caelumora_model::approval::ApprovalRuleId;
use caelumora_model::channel::ChannelId;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::content::ContentRef;
use caelumora_model::context::ModelProfile;
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
use std::fmt::Debug;
use std::sync::Arc;

/// What the kernel decides of a tool call as it is admitted (`ActorRuntime::admit_tool_call`, issue
/// 516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPermit {
    /// The policy does not let it be called (`CallTool`).
    Forbidden,
    /// A rule of a place denies it.
    Denied(ApprovalRuleId),
    /// A rule of a place asks a person to decide it, whatever its tool says (issue 13).
    Ask,
    /// Nothing of the kernel stands against it: whether a person is asked is its tool's.
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

/// What the actor of an agent runs with, on the agent side (issue 516): the one interface its kind
/// is given by the kernel (`AgentKinds::register`). Who the agent is and its channel as they are
/// now, its state, the attachments of its messages, its session to read, what the kernel decides of
/// its work, and the extensions plugins bind to that work - each asked of the kernel in the agent's
/// name, which the kernel builds at every call. It hands out no controller, no store and no policy.
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
    async fn load_channel_state(&self) -> StoreResult<AgentChannelState>;

    /// Stores the agent's `state` in its channel, and then lets go of what its history holds
    /// before its window (`AgentChannelState::keep_window`, issue 379).
    ///
    /// # Errors
    /// If it cannot be stored; nothing is let go then.
    async fn store_channel_state(&self, state: &mut AgentChannelState) -> StoreResult<()>;

    /// The message `id` as the agent's transcript in its channel holds it, on whichever branch,
    /// with every change of it recorded since (issue 380); `None` if it holds it nowhere.
    ///
    /// # Errors
    /// If the transcript cannot be read.
    async fn transcript_message(&self, id: MessageId) -> StoreResult<Option<Message>>;

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
    ) -> StoreResult<()>;

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
    /// the agent's name as the kernel holds it now (issue 321).
    ///
    /// # Errors
    /// `ComponentError` if they cannot be planned or built.
    async fn build_components(
        &self,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        facets: &[TypeKey],
    ) -> ComponentResult<ComponentContainer>;

    /// The reader of the type `key` names that a plugin or the kernel binds to the agent's work for
    /// `initiator` at `classification` (`KernelBuilder::extend_readers`), its type forgotten: what
    /// `reader` asks.
    async fn reader_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc>;

    /// The writer of the type `key` names, bound as `reader_of` binds a reader
    /// (`KernelBuilder::extend_writers`): what `writer` asks.
    async fn writer_of(
        &self,
        initiator: ActorId,
        classification: Classification,
        key: TypeKey,
    ) -> Option<ErasedArc>;
}

impl dyn ActorRuntime + '_ {
    /// The reader `T` a plugin or the kernel binds to the agent's work for `initiator` at
    /// `classification` (issue 516): what of the plugin's service the work reads by, acting in the
    /// work's context as the session is read (`session`). `None` if none is bound, if it is not
    /// offered there, if the agent may not read its session, or while the channel is not attached
    /// to it.
    pub async fn reader<T: ?Sized + Send + Sync + 'static>(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> Option<Arc<T>> {
        let made = self.reader_of(initiator, classification, TypeKey::of::<T>()).await?;
        made.get::<T>().cloned()
    }

    /// The writer `T` a plugin or the kernel binds to the agent's work: what of the plugin's
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
