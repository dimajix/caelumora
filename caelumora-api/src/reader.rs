//! What the agent side reads of its session (issue 516): the readers, each the part of a view an
//! agent's work reads through - a session (`SessionReader`), a channel (`ChannelReader`), a
//! participant (`ParticipantReader`) -, and what reading fails with (`ViewError`). The kernel's
//! views implement them and keep their full surface for the frontends; the agent side is handed a
//! reader, never a view, so it reaches no policy, no manager and no context through it.

use crate::error::AttachmentError;
use crate::terminal::ByteStream;
use anyhow::Error;
use async_trait::async_trait;
use caelumora_domain::actor::{ActorDescription, ActorId};
use caelumora_domain::agent::AgentId;
use caelumora_domain::chain::Position;
use caelumora_domain::confidentiality::{Classification, Place};
use caelumora_domain::content::{ContentId, ContentRef};
use caelumora_domain::digest::Digest;
use caelumora_domain::group::GroupId;
use caelumora_domain::history::{HistoryField, HistoryPage};
use caelumora_domain::message::{Message, MessageId};
use caelumora_domain::note::Note;
use caelumora_domain::privacy::SessionPrivacy;
use caelumora_domain::project::ProjectId;
use caelumora_domain::query::Filter;
use caelumora_domain::security::Refused;
use caelumora_domain::session::{ChannelId, SessionId};
use caelumora_domain::snapshot::PublishedNotes;
use caelumora_domain::value::language::Language;
use caelumora_domain::value::name::Name;
use caelumora_domain::value::text::{Briefing, Summary};
use caelumora_domain::workspace::WorkspaceId;
use std::{error, fmt, result};

/// A session as an agent's work reads it from its channel (`SessionView`).
#[async_trait]
pub trait SessionReader: Send + Sync {
    /// The session read.
    fn id(&self) -> SessionId;

    /// The project its results go to (issue 467).
    fn project(&self) -> ProjectId;

    /// What the session is for, in a line (issue 245).
    fn summary(&self) -> Option<&Summary>;

    /// What an agent should know before it works in the session (issue 245).
    fn briefing(&self) -> Option<&Briefing>;

    /// Where the reading stands: the actor's channel, at the classification its work reads at.
    fn place(&self) -> Option<&Place>;

    /// Channel `id` of the session, if the reader may read it.
    async fn channel(&self, id: ChannelId) -> ViewResult<Box<dyn ChannelReader>>;

    /// The channel of the session called `name`, if the reader may read it.
    async fn channel_named(&self, name: &str) -> ViewResult<Box<dyn ChannelReader>>;

    /// The channels of the session the reader may read.
    async fn channels(&self) -> Vec<Box<dyn ChannelReader>>;

    /// The reader's private notes in the session's other channels (issue 255).
    async fn own_private_notes(&self) -> Vec<PrivateNotesElsewhere>;

    /// How session `id` stands for the flow rule now, `None` if it is gone (issue 134).
    async fn privacy_of(&self, id: SessionId) -> ViewResult<Option<SessionPrivacy>>;
}

/// A channel as an agent's work reads it (`ChannelView`): as a whole, or what of it it may read.
#[async_trait]
pub trait ChannelReader: Send + Sync {
    /// The channel read.
    fn id(&self) -> ChannelId;

    /// Its name, unique in its session.
    fn name(&self) -> &Name;

    /// How its contents are classified (issue 133).
    fn classification(&self) -> &Classification;

    /// What the channel is for, in a line (issue 245).
    fn summary(&self) -> Option<&Summary>;

    /// What an agent should know before it works in the channel (issue 245).
    fn briefing(&self) -> Option<&Briefing>;

    /// The language the channel's conversation is held in: its own, else its session's (issue
    /// 425).
    fn effective_language(&self) -> Option<&Language>;

    /// The session it was reached through.
    fn session(&self) -> &dyn SessionReader;

    /// Its participants, each as the reader may see them.
    async fn participants(&self) -> Vec<Box<dyn ParticipantReader>>;

    /// Its participant `actor`, if it takes part.
    async fn participant(&self, actor: ActorId) -> ViewResult<Box<dyn ParticipantReader>>;

    /// Where in the history the reader's reading begins (issue 379).
    async fn visible_from(&self) -> ViewResult<Position>;

    /// A page of the history the reader may read, before `before`, of at most `limit` entries
    /// that match `filter`.
    async fn history_page_of(
        &self,
        before: Option<Position>,
        limit: usize,
        filter: Option<Filter<HistoryField>>,
    ) -> ViewResult<HistoryPage>;

    /// Message `id`, if the reader may read it.
    async fn message(&self, id: MessageId) -> ViewResult<Message>;

    /// What message `message` carries as `id`, if the reader may read it.
    async fn attachment(&self, message: MessageId, id: ContentId) -> ViewResult<ContentRef>;

    /// What message `message` carries as `id`, with its size and its content.
    async fn attachment_content(
        &self,
        message: MessageId,
        id: ContentId,
    ) -> ViewResult<(ContentRef, u64, ByteStream)>;

    /// The notes its participants published there, as far as the reader may read them.
    async fn notes(&self) -> ViewResult<Vec<PublishedNotes>>;
}

/// A participant of a channel as an agent's work sees it (`ParticipantView`).
#[async_trait]
pub trait ParticipantReader: Send + Sync {
    /// The actor that takes part.
    fn id(&self) -> ActorId;

    /// Its name; `None` for a user or agent that no longer exists.
    fn name(&self) -> Option<&str>;

    /// What of the participant is public to everyone in the channel (issue 176); `None` for one
    /// that no longer exists.
    async fn describe(&self) -> ViewResult<Option<ActorDescription>>;
}

/// Why an edge could not be followed. The messages are meant for users
/// (see `crate::error::user_message_with`) - or, through `ToolError`, for the
/// model that asked.
///
/// What does not exist and what the viewer may not read are one case each,
/// so that nothing can be probed - as `SessionError::ChannelNotFound` holds
/// for operations.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("There is no session {0}")]
    SessionNotFound(SessionId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    #[error("There is no workspace {0}")]
    WorkspaceNotFound(WorkspaceId),
    #[error("There is no project {0}")]
    ProjectNotFound(ProjectId),
    #[error("There is no group {0}")]
    GroupNotFound(GroupId),
    #[error("There is no agent {0}")]
    AgentNotFound(AgentId),
    /// Asked for by name, as an agent's tools do: no such channel, or
    /// nothing in it that can be read from where the viewer reads.
    #[error("there is no channel named \"{0}\" that can be read from here")]
    NoChannelNamed(String),
    #[error("Channel {channel} has no participant {actor}")]
    ParticipantNotFound { channel: ChannelId, actor: ActorId },
    /// Not in the history, or not in the part of it the viewer sees.
    #[error("Channel {channel} has no message {message}")]
    MessageNotFound { channel: ChannelId, message: MessageId },
    /// Reached, but not the viewer's to read: what an agent saw, which
    /// tools it has.
    #[error("You are not permitted to read this of {actor}")]
    Forbidden {
        actor: ActorId,
        #[source]
        refused: Refused,
    },
    /// Not the viewer's to do: searching whom to take into a session they
    /// may not change (issue 221).
    #[error("You are not permitted to do this")]
    NotPermitted(#[source] Refused),
    /// Only agents have a context.
    #[error("There is nothing {actor} saw: it is no agent, or no longer exists")]
    NoContext { actor: ActorId },
    /// An attachment of a message the viewer sees could not be read: it carries none such, or its
    /// storage failed (issue 282).
    #[error(transparent)]
    Attachment(#[from] AttachmentError),
    /// Reaching it failed; the cause is in the log.
    #[error("Reading failed")]
    Failed(#[source] Error),
}

impl ViewError {
    /// Reaching something failed: `error`, with what was being done, for
    /// the log - the viewer learns only that it failed.
    pub fn failed<E>(error: E, doing: impl fmt::Display + Send + Sync + 'static) -> Self
    where
        E: error::Error + Send + Sync + 'static,
    {
        ViewError::Failed(Error::new(error).context(doing))
    }
}

pub type ViewResult<T> = result::Result<T, ViewError>;

/// An agent's private notes in another channel of its session, as far as
/// they may be read where it reads from (`SessionView::own_private_notes`):
/// what of that channel it knows itself - no view of the channel, which it
/// may no longer reach.
#[derive(Debug, Clone)]
pub struct PrivateNotesElsewhere {
    pub channel: ChannelId,
    pub name: String,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}
