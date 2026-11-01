//! What messages carry besides their text (issue 282): the one owner of the content of attachments,
//! the `AttachmentManager` - what is uploaded for a message not yet sent, what a message claims as
//! its channel admits it, what a tool gave its model (issue 512), what is read for a download or
//! given a model along - held in memory for its next calls -, and what goes with a channel purged.
//! An attachment is a `ContentRef` a message holds; its bytes lie under
//! `attachments/<channel>/<id>` in a storage of content.

mod cache;
mod manager;

pub use manager::AttachmentManager;

use crate::backend::StorageGone;
use crate::content::ContentError;
use caelumora_model::actor::ActorId;
use caelumora_model::content::ContentId;
use caelumora_model::value::ValueError;

/// What uploading, claiming or reading an attachment failed with - written to be read by users
/// but for `StorageGone`, which is the log's (`error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("no storage of content is configured for attachments")]
    NotConfigured,
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
    /// What a storage holds of an attachment is no longer what its message says - other bytes, or
    /// as many.
    #[error("the content of attachment {0} is no longer what its message says")]
    Altered(ContentId),
    /// No such attachment, or none the caller may reach - one case, so that nothing can be probed.
    #[error("there is no attachment {0}")]
    NotFound(ContentId),
    /// More files for one tool result than a message may carry attachments (issue 512).
    #[error("more than {0} files for one tool result")]
    TooMany(usize),
    /// What is to be given a model to see that is no image a model can be given (issue 512).
    #[error("'{name}' is {media_type}, not an image a model can be given")]
    NotAnImage { name: String, media_type: String },
    #[error(transparent)]
    Invalid(#[from] ValueError),
    /// What the storage of content reported: content too large, other than it was said to be, or
    /// a storage that failed.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// The storage an attachment lies in, since removed from the configuration.
    #[error(transparent)]
    StorageGone(#[from] StorageGone),
}

/// What the `AttachmentManager` answers.
pub type AttachmentResult<T> = Result<T, AttachmentError>;
