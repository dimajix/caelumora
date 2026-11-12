//! `ActorRuntime`: the interface the kernel gives the actor of an agent, on the agent side (issue
//! 516) - who it is, its channel and its state, what it reads, and what it asks the kernel to
//! decide -, which the kernel serves (`AgentRuntime`).

use crate::component::ComponentContainer;
use crate::component::ComponentResult;
use crate::error::AttachmentResult;
use crate::reader::{ChannelReader, SessionReader, ViewResult};
use crate::staging::CommitResult;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{Agent, AgentChannelState, Personality};
use caelumora_domain::approval::ApprovalRuleId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::component::ComponentConfig;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::content::{ContentId, ContentRef};
use caelumora_domain::context::ModelProfile;
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::prompt_catalogue::PromptCatalogue;
use caelumora_domain::question::{QuestionError, Questions};
use caelumora_domain::secret::Secrets;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResult;
use caelumora_domain::tool::{ToolCall, ToolContent};
use caelumora_domain::turn::StagedChanges;
use caelumora_domain::type_map::{ErasedArc, TypeKey};
use caelumora_domain::value::time_zone::TimeZone;
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

    /// Hands what a turn of the agent's work for `initiator` at `classification`, ended with
    /// message `after`, staged as `kind` in `staged` to the writer bound to commit that kind
    /// (issue 518), in the work's name: all of it, or none.
    ///
    /// # Errors
    /// `NotOffered` if no writer commits `kind` there, `Unreachable` if the work's session cannot
    /// be reached, `Unreadable` if the changes are not what the writer commits, or what it reports.
    async fn commit_staged(
        &self,
        initiator: ActorId,
        classification: Classification,
        staged: &StagedChanges,
        kind: &str,
        after: MessageId,
    ) -> CommitResult<()>;

    /// Puts their content on the attachments of `messages` - of the agent's transcript - that the
    /// model of `profile` is given along (issue 282), and answers how each is classified, as its
    /// message says. Only an attachment of the agent's channel is read (issue 549).
    ///
    /// # Errors
    /// `AttachmentError`: `NotFound` for one that is not of the agent's channel; if one cannot be
    /// read, or is not what its message says.
    async fn load_attachments(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>>;

    /// Keeps `files`, what a tool gave its model for a call the agent makes for `initiator`,
    /// classified `classification` as the call's result (issue 512).
    ///
    /// # Errors
    /// `AttachmentError` if it may not or cannot be kept.
    async fn create_content(
        &self,
        initiator: ActorId,
        classification: Classification,
        files: Vec<ToolContent>,
    ) -> AttachmentResult<Vec<ContentRef>>;

    /// Whether `create_content` would keep `files`, what a tool gives for a call the agent makes
    /// for `initiator`, classified `classification` (issue 553): for what a tool keeps only where
    /// it can. A yes holds but for a storage that fails.
    fn can_keep_content(
        &self,
        initiator: ActorId,
        classification: &Classification,
        files: &[ToolContent],
    ) -> bool;

    /// The attachment `id` of message `message` in the agent's channel, as its work for
    /// `initiator` reads it there at `classification`, copied for a message the agent will say
    /// there (issues 282, 549): named, not handed over, so that the copy has the content and the
    /// label its channel holds for it, not those the agent side would state.
    ///
    /// # Errors
    /// What following the views to the attachment reports (`channel`,
    /// `ChannelReader::attachment`); `Failed` while the channel is not attached to its session, or
    /// once either is gone; `Attachment` if it may not or cannot be copied.
    async fn copy_attachment(
        &self,
        initiator: ActorId,
        classification: Classification,
        message: MessageId,
        id: ContentId,
    ) -> ViewResult<ContentRef>;

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

    /// Whether a note of the agent in its channel, public or private, may be reclassified, in its
    /// name for `initiator`, as `classification`, within what is classified `within` (issues 133,
    /// 516).
    async fn may_reclassify_note(
        &self,
        initiator: ActorId,
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
