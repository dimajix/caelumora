//! What messages carry besides their text (issue 282): the one owner of the content of attachments,
//! the `AttachmentManager` - what is uploaded for a message not yet sent, what a message claims as
//! its channel admits it, what is read for a download or given a model along - held in memory for
//! its next calls -, and what goes with a channel purged. An
//! attachment is a `ContentRef` a message holds; its bytes lie under `attachments/<channel>/<id>`
//! in a storage of content.

mod cache;
mod manager;

pub use manager::AttachmentManager;

use crate::content::ContentError;
use caelumora_model::actor::ActorId;
use caelumora_model::content::ContentId;
use caelumora_model::value::ValueError;

/// What uploading, claiming or reading an attachment failed with - written to be read by users
/// but for `Unavailable` and `UnknownStorage`, which are the log's (`error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("no storage of content is configured for attachments")]
    NotConfigured,
    #[error("the storage of content '{0}' is not configured")]
    UnknownStorage(String),
    /// What may not enter the channel so classified - above it, for an audience the channel's would
    /// not be covered by, or with a compartment not configured.
    #[error("content so classified cannot be attached in this channel")]
    ClassificationRefused,
    /// What its storage may not hold so classified.
    #[error("the storage of content '{0}' may not hold what is so classified")]
    StorageNotCleared(String),
    /// Who puts content into a channel takes part in it.
    #[error("{0} takes no part in the channel")]
    NotAParticipant(ActorId),
    #[error("an attachment may not be larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("the content was not as large as it was said to be")]
    Changed,
    /// What a storage holds of an attachment is no longer what its message says - other bytes, or
    /// as many.
    #[error("the content of attachment {0} is no longer what its message says")]
    Altered(ContentId),
    /// No such attachment, or none the caller may reach - one case, so that nothing can be probed.
    #[error("there is no attachment {0}")]
    NotFound(ContentId),
    #[error(transparent)]
    Invalid(#[from] ValueError),
    /// The storage failed: what happened, never where.
    #[error("the storage of content failed: {0}")]
    Unavailable(String),
}

impl AttachmentError {
    /// Whether a storage failed rather than the request: one that could not be reached, that is no
    /// longer configured, or that holds other bytes than were kept.
    pub fn is_storage_failure(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::UnknownStorage(_) | Self::Altered(_))
    }
}

/// What the `AttachmentManager` answers.
pub type AttachmentResult<T> = Result<T, AttachmentError>;

/// What a storage of content says, as an attachment's error.
impl From<ContentError> for AttachmentError {
    fn from(error: ContentError) -> Self {
        match error {
            ContentError::TooLarge { limit } => Self::TooLarge { limit },
            ContentError::Changed => Self::Changed,
            ContentError::Unavailable(what) => Self::Unavailable(what),
            error @ (ContentError::InvalidKey(_) | ContentError::NotFound(_)) => Self::Unavailable(error.to_string()),
        }
    }
}
