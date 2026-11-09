//! The errors the kernel's calls answer a library in, beside those of their own module - what is
//! kept as content (`ContentError`), an attachment (`AttachmentError`), a storage since removed
//! (`StorageGone`) -, and how the API's errors are told to a person (issue 02): some are written to
//! be read by users - they name the component, the endpoint or the entity that failed, and nothing
//! that must not be disclosed -, others, like a store backend failure, may carry file paths or
//! other internals and belong in the log only. An error's chain of causes is described as one line
//! (`describe_chain`), a failed request's without the URL its client writes into it
//! (`describe_request_error`, issue 188).

use crate::command::CommandError;
use crate::component::ComponentError;
use crate::model::ModelError;
use crate::reader::ViewError;
use crate::terminal::TerminalError;
use crate::workspace::WorkspaceError;
use caelumora_domain::actor::ActorId;
use caelumora_domain::content::ContentId;
use caelumora_domain::store::StoreError;
use caelumora_domain::template::TemplateError;
use caelumora_domain::value::ValueError;
use std::error::Error;
use std::io;

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    /// A key that is none: a path of segments, without `.`, `..` or an
    /// empty one - as its owner made it, never as a client wrote it.
    #[error("'{0}' is no key of content")]
    InvalidKey(String),
    #[error("nothing is kept under '{0}'")]
    NotFound(String),
    #[error("the content is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    /// What was kept is not as large as it was said to be: it changed while it was read, or broke
    /// off.
    #[error("the content was not as large as it was said to be")]
    Changed,
    /// The storage failed: what happened, never where.
    #[error("the storage of content failed: {0}")]
    Unavailable(String),
}

impl ContentError {
    /// What a failure of reading or writing is, as an error of a storage says it: its kind, never
    /// the path.
    pub fn unavailable(error: io::Error) -> Self {
        Self::Unavailable(error.kind().to_string())
    }
}

/// A backend that something kept names as where it lies - a workspace, an artifact's version, an
/// attachment -, since removed from the configuration (issue 466): the server's failure, not the
/// request's, however the kept thing was reached. Its message names the backend, which is the
/// configuration's: for the log, not for users or models.
#[derive(Debug, thiserror::Error)]
#[error("the storage '{0}' is no longer configured")]
pub struct StorageGone(pub String);

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

/// The message to show a user for `error`: the outermost error in its chain that may be shown, or
/// `None` if there is none - each error asked of `judge` first: what it shows of an error of a type
/// it knows, `None` for one it does not know or that may not be shown - then judged by the API's
/// types (and the domain's). One walk through the chain, so the outermost error that may be shown
/// is shown, whoever knows its type: the agents' logic judges its own errors so
/// (`caelumora_agents::error::user_message`, issue 370).
pub fn user_message_with(
    error: &anyhow::Error,
    judge: impl Fn(&(dyn Error + 'static)) -> Option<String>,
) -> Option<String> {
    error.chain().find_map(|e| judge(e).or_else(|| judged(e)))
}

/// View errors: what they name is safe; why reaching it failed is for the
/// log - for an error of another crate that holds one, too.
pub fn view_message(error: &ViewError) -> Option<String> {
    match error {
        ViewError::Failed(_) => None,
        ViewError::Attachment(e) => attachment_message(e),
        _ => Some(error.to_string()),
    }
}

/// Attachment errors: a storage no longer configured (`StorageGone`) is named by the
/// configuration, for the log; what a storage reported is judged as content errors are; the rest
/// names the attachment, its classification or a limit.
fn attachment_message(error: &AttachmentError) -> Option<String> {
    match error {
        AttachmentError::StorageGone(_) => None,
        AttachmentError::Content(e) => content_message(e),
        _ => Some(error.to_string()),
    }
}

/// Content errors: a limit, content other than it was said to be, or what failed in a storage -
/// what happened, never where, as a workspace's storage tells it; a key is its owner's, for the
/// log.
fn content_message(error: &ContentError) -> Option<String> {
    match error {
        ContentError::TooLarge { .. } | ContentError::Changed | ContentError::Unavailable(_) => {
            Some(error.to_string())
        }
        ContentError::InvalidKey(_) | ContentError::NotFound(_) => None,
    }
}

/// Component errors name the component, the bundle and what was written - except where a bundle
/// was looked for in a store that failed (issue 457), whose failure is judged as store errors are:
/// then none of it is shown, the components that wrap it included.
pub fn component_message(error: &ComponentError) -> Option<String> {
    match error {
        ComponentError::Bundle { cause, .. } => store_message(cause).map(|_| error.to_string()),
        ComponentError::Build { cause, .. } | ComponentError::Backend { cause, .. } => {
            component_message(cause).map(|_| error.to_string())
        }
        _ => Some(error.to_string()),
    }
}

/// Store errors name entities and ids - except `Backend`, which wraps what
/// the storage itself reported, file paths included - for an error of
/// another crate that holds one, too.
pub fn store_message(error: &StoreError) -> Option<String> {
    match error {
        StoreError::Backend(_) => None,
        _ => Some(error.to_string()),
    }
}

/// What the API shows of `e`, if it is of a type of the API's - or of the domain's - that may be
/// shown.
fn judged(e: &(dyn Error + 'static)) -> Option<String> {
    if let Some(e) = e.downcast_ref::<ComponentError>() {
        component_message(e)
    } else if let Some(e) = e.downcast_ref::<CommandError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ModelError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ViewError>() {
        view_message(e)
    } else if let Some(e) = e.downcast_ref::<WorkspaceError>() {
        // Its store errors are judged as store errors are, a storage no longer configured is the
        // configuration's; the rest name the workspace and what the caller wrote, never a place.
        match e {
            WorkspaceError::Store(e) => store_message(e),
            WorkspaceError::StorageGone(_) => None,
            _ => Some(e.to_string()),
        }
    } else if let Some(e) = e.downcast_ref::<AttachmentError>() {
        attachment_message(e)
    } else if let Some(e) = e.downcast_ref::<TerminalError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ContentError>() {
        content_message(e)
    } else if let Some(e) = e.downcast_ref::<TemplateError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ValueError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<StoreError>() {
        store_message(e)
    } else {
        None
    }
}

/// The full chain of the causes of `error`, a failed request, without the
/// URL that `reqwest` writes into its messages.
pub fn describe_request_error(error: reqwest::Error) -> String {
    describe_chain(&error.without_url())
}

/// The full chain of an error's causes as one line, e.g. "error sending
/// request: connection refused", for errors whose own `Display` stops at
/// the outermost layer.
pub fn describe_chain(error: &(dyn Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(e) = current {
        let text = e.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        current = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;

    #[test]
    fn finds_the_disclosable_error_inside_a_context_chain() {
        let error = Error::new(ModelError::Connection {
            endpoint: "http://localhost:11434".to_string(),
            reason: "connection refused".to_string(),
        })
        .context("Error during the model's stream");

        assert_eq!(
            user_message_with(&error, |_| None).as_deref(),
            Some("cannot reach the model server at http://localhost:11434: connection refused")
        );
    }
}
