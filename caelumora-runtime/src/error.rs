//! Turning errors into messages that may be shown to users (issue 02).
//!
//! Errors travel as `anyhow::Error` through the runtime, with the domain
//! errors of the layers below somewhere in their chain. Some of those are
//! written to be read by users - they name the component, the endpoint or the
//! entity that failed, and nothing that must not be disclosed. Others, like a
//! store backend failure, may carry file paths or other internals and belong
//! in the log only.

use crate::agent::AgentError;
use crate::attachment::AttachmentError;
use crate::command::CommandError;
use crate::content::ContentError;
use crate::component::ComponentError;
use crate::session::SessionError;
use crate::view::{AgentContextError, ViewError};
use crate::channel::ChannelError;
use crate::terminal::TerminalError;
use crate::workspace::WorkspaceError;
use caelumora_model::security::Refused;
use caelumora_model::store::StoreError;
use caelumora_model::template::TemplateError;
use caelumora_model::value::ValueError;
use std::error::Error;

/// The message to show a user for `error`: the outermost error in its chain
/// whose type is known to be safe to disclose, or `None` if there is none -
/// in which case the caller should show a generic message and point to the
/// log. Judged by the runtime's types alone; a crate that defines error
/// types of its own for others to answer in judges them itself
/// (`user_message_with`, issue 370).
pub fn user_message(error: &anyhow::Error) -> Option<String> {
    user_message_with(error, |_| None)
}

/// As `user_message`, each error of the chain asked of `judge` first: what
/// it shows of an error of a type it knows, `None` for one it does not know
/// or that may not be shown - then judged as the runtime judges it. One walk
/// through the chain, so the outermost error that may be shown is shown,
/// whoever knows its type: the agents' logic judges its own errors so
/// (`caelumora_agents::error::user_message`, issue 370).
pub fn user_message_with(error: &anyhow::Error, judge: impl Fn(&(dyn Error + 'static)) -> Option<String>) -> Option<String> {
    error.chain().find_map(|e| judge(e).or_else(|| judged(e)))
}

/// What the runtime shows of `e`, if it is of a type of the runtime's that
/// may be shown.
fn judged(e: &(dyn Error + 'static)) -> Option<String> {
    if let Some(e) = e.downcast_ref::<ComponentError>() {
        component_message(e)
    } else if let Some(e) = e.downcast_ref::<CommandError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<AgentContextError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ViewError>() {
        view_message(e)
    } else if let Some(e) = e.downcast_ref::<ChannelError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<SessionError>() {
        // As a component error, whose message it repeats.
        match e {
            SessionError::InvalidComponents(cause) => component_message(cause).map(|_| e.to_string()),
            _ => Some(e.to_string()),
        }
    } else if let Some(e) = e.downcast_ref::<WorkspaceError>() {
        // Its store errors are judged as store errors are, a storage no longer configured is the
        // configuration's; the rest name the workspace and what the caller wrote, never a place.
        match e {
            WorkspaceError::Store(e) => store_message(e),
            WorkspaceError::StorageGone(_) => None,
            _ => Some(e.to_string()),
        }
    } else if let Some(e) = e.downcast_ref::<AgentError>() {
        // As a workspace's: its store errors are judged as store errors
        // are, the rest name the agent, its personality or its name.
        match e {
            AgentError::Store(e) => store_message(e),
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
        ContentError::TooLarge { .. } | ContentError::Changed | ContentError::Unavailable(_) => Some(error.to_string()),
        ContentError::InvalidKey(_) | ContentError::NotFound(_) => None,
    }
}

/// Component errors name the component, the bundle and what was written - except where a bundle
/// was looked for in a store that failed (issue 457), whose failure is judged as store errors are:
/// then none of it is shown, the components that wrap it included.
fn component_message(error: &ComponentError) -> Option<String> {
    match error {
        ComponentError::Bundle { cause, .. } => store_message(cause).map(|_| error.to_string()),
        ComponentError::Build { cause, .. } | ComponentError::Backend { cause, .. } => component_message(cause).map(|_| error.to_string()),
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

/// What the `AccessPolicy` refused, if `error` is a refusal - which a
/// caller answers with "not allowed", not with "something broke". Found
/// wherever in its chain it is (issue 323): every error that ends in a
/// refusal carries it as its source.
pub fn refusal(error: &anyhow::Error) -> Option<&Refused> {
    error.chain().find_map(|e| e.downcast_ref::<Refused>())
}

/// Whether `error` is a refusal by the `AccessPolicy`.
pub fn is_forbidden(error: &anyhow::Error) -> bool {
    refusal(error).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::StorageGone;
    use caelumora_model::actor::ActorId;
    use caelumora_model::channel::ChannelId;
    use caelumora_model::content::ContentId;

    /// An error of a type of another crate's, holding one of the runtime's.
    #[derive(Debug, thiserror::Error)]
    #[error("the other crate's")]
    struct Others(#[source] ChannelError);

    #[test]
    fn finds_the_disclosable_error_inside_a_context_chain() {
        let channel = ChannelError::NotAParticipant { participant: ActorId::System, channel: ChannelId::new() };
        let shown = channel.to_string();
        let error = anyhow::Error::new(channel).context("removing a participant");
        assert_eq!(user_message(&error), Some(shown));
    }

    /// A crate that judges its own types is asked first, and the outermost
    /// error that may be shown is shown, whoever knows its type (issue 370).
    #[test]
    fn another_crate_judges_its_own_types_and_the_outermost_is_shown() {
        let inner = ChannelError::NotAParticipant { participant: ActorId::System, channel: ChannelId::new() };
        let shown = inner.to_string();
        let error = anyhow::Error::new(Others(inner));

        assert_eq!(user_message(&error), Some(shown.clone()), "unknown to the runtime: what it holds");
        let knows = |e: &(dyn Error + 'static)| e.downcast_ref::<Others>().map(ToString::to_string);
        assert_eq!(user_message_with(&error, knows).as_deref(), Some("the other crate's"), "the outermost");
        assert_eq!(user_message_with(&error, |_| None), Some(shown), "the runtime's still");
    }

    #[test]
    fn component_errors_name_the_component() {
        let error = anyhow::Error::new(ComponentError::Build {
            component: "github-mcp".to_string(),
            kind: "mcp_tool".to_string(),
            cause: Box::new(ComponentError::InvalidConfig("invalid parameters: missing field `url`".to_string())),
        });

        let message = user_message(&error).unwrap();
        assert!(message.contains("'github-mcp'") && message.contains("mcp_tool") && message.contains("missing field `url`"), "{message}");
    }

    /// A bundle looked for in a store that failed is not shown, nor what
    /// wraps it; one that is gone is (issue 457).
    #[test]
    fn a_bundle_whose_store_failed_is_not_disclosed() {
        let built = |cause| ComponentError::Build { component: "tools".to_string(), kind: "ref".to_string(), cause: Box::new(ComponentError::Bundle { name: "tools".to_string(), cause }) };
        let failed = || built(StoreError::Backend("/srv/data/component_bundle: permission denied".to_string()));
        assert_eq!(user_message(&anyhow::Error::new(failed())), None);
        assert_eq!(user_message(&anyhow::Error::new(SessionError::InvalidComponents(failed()))), None, "nor by the session that repeats it");
        let gone = built(StoreError::NotFound { entity: "component_bundle", id: "tools".to_string() });
        assert!(user_message(&anyhow::Error::new(gone)).is_some_and(|message| message.contains("'tools'")));
    }

    #[test]
    fn a_storage_no_longer_configured_is_not_disclosed() {
        let gone = || AttachmentError::StorageGone(StorageGone("old".to_string()));
        assert_eq!(user_message(&anyhow::Error::new(gone())), None);
        assert_eq!(user_message(&anyhow::Error::new(ViewError::Attachment(gone()))), None, "nor read through a view, which repeats its text");
        assert_eq!(user_message(&anyhow::Error::new(WorkspaceError::StorageGone(StorageGone("old".to_string())))), None, "nor a workspace's");
        assert!(user_message(&anyhow::Error::new(WorkspaceError::UnknownStorage("nfs".to_string()))).is_some(), "where the caller named it");
        let failed = AttachmentError::Content(ContentError::Unavailable("permission denied".to_string()));
        assert_eq!(user_message(&anyhow::Error::new(failed)).as_deref(), Some("the storage of content failed: permission denied"), "what happened, as a workspace's storage tells it");
        let key = AttachmentError::Content(ContentError::NotFound("attachments/a/b".to_string()));
        assert_eq!(user_message(&anyhow::Error::new(ViewError::Attachment(key))), None, "a key is its owner's");
        assert!(user_message(&anyhow::Error::new(ViewError::Attachment(AttachmentError::NotFound(ContentId::new())))).is_some(), "what it names is safe");
    }

    #[test]
    fn store_backend_errors_are_not_disclosed() {
        let error = anyhow::Error::new(StoreError::Backend("/home/someone/local/entity_store: permission denied".to_string()));
        assert_eq!(user_message(&error), None);
        assert_eq!(user_message(&anyhow::anyhow!("something internal")), None);
    }

    #[test]
    fn a_refusal_is_found_through_wrappers_whatever_their_type() {
        let refused = Refused { subject: "workspace", action: "UpdateWorkspace".to_string() };
        // Transparent around the store's error, which holds the refusal as
        // its source - as a plugin's error does, in the core's `anyhow`.
        let error = anyhow::Error::new(WorkspaceError::Store(StoreError::PermissionDenied(refused.clone()))).context("deleting what lies in the scope");
        assert_eq!(refusal(&error), Some(&refused));
        assert!(!is_forbidden(&anyhow::Error::new(StoreError::Backend("x".to_string()))));
    }
}
