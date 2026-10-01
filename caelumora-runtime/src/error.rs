//! Turning errors into messages that may be shown to users (issue 02).
//!
//! Errors travel as `anyhow::Error` through the runtime, with the domain
//! errors of the layers below somewhere in their chain. Some of those are
//! written to be read by users - they name the component, the endpoint or the
//! entity that failed, and nothing that must not be disclosed. Others, like a
//! store backend failure, may carry file paths or other internals and belong
//! in the log only.

use crate::command::CommandError;
use crate::component::ComponentError;
use crate::llm::LLMError;
use crate::session::SessionError;
use crate::tool::ToolError;
use crate::view::{ContextError, ViewError};
use crate::channel::ChannelError;
use caelumora_model::store::StoreError;
use caelumora_model::template::TemplateError;

/// The message to show a user for `error`: the outermost error in its chain
/// whose type is known to be safe to disclose, or `None` if there is none -
/// in which case the caller should show a generic message and point to the
/// log.
pub fn user_message(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|e| {
        if let Some(e) = e.downcast_ref::<LLMError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ComponentError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ToolError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<CommandError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ContextError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ViewError>() {
            // What it names is safe; why reaching it failed is for the log.
            match e {
                ViewError::Failed(_) => None,
                _ => Some(e.to_string()),
            }
        } else if let Some(e) = e.downcast_ref::<ChannelError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<SessionError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<TemplateError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<StoreError>() {
            store_message(e)
        } else {
            None
        }
    })
}

/// Store errors name entities and ids - except `Backend`, which wraps what
/// the storage itself reported, file paths included.
fn store_message(error: &StoreError) -> Option<String> {
    match error {
        StoreError::Backend(_) => None,
        _ => Some(error.to_string()),
    }
}

/// Whether `error` is a refusal by the `SecurityManager` - which a caller
/// answers with "not allowed", not with "something broke".
pub fn is_forbidden(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        matches!(e.downcast_ref::<ChannelError>(), Some(ChannelError::Forbidden { .. }))
            || matches!(e.downcast_ref::<CommandError>(), Some(CommandError::Forbidden(_)))
            || matches!(e.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied))
    })
}

/// Whether `error` means that an entity of kind `entity` (e.g. "session")
/// does not exist. The kind matters: loading a session can fail because its
/// agent's personality is missing, and that is not "session not found".
pub fn is_not_found(error: &anyhow::Error, entity: &str) -> bool {
    let matches = |e: &StoreError| matches!(e, StoreError::NotFound { entity: kind, .. } if *kind == entity);
    error.chain().any(|e| e.downcast_ref::<StoreError>().is_some_and(matches))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_disclosable_error_inside_a_context_chain() {
        let error = anyhow::Error::new(LLMError::Connection {
            endpoint: "http://localhost:11434".to_string(),
            reason: "connection refused".to_string(),
        })
        .context("Error during LLM stream");

        assert_eq!(
            user_message(&error).as_deref(),
            Some("cannot reach LLM backend at http://localhost:11434: connection refused")
        );
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

    #[test]
    fn store_backend_errors_are_not_disclosed() {
        let error = anyhow::Error::new(StoreError::Backend("/home/someone/local/entity_store: permission denied".to_string()));
        assert_eq!(user_message(&error), None);
        assert_eq!(user_message(&anyhow::anyhow!("something internal")), None);
    }

    #[test]
    fn not_found_is_recognised_through_wrappers() {
        let not_found = |entity| StoreError::NotFound { entity, id: "42".to_string() };
        assert!(is_not_found(&anyhow::Error::new(not_found("session")).context("loading"), "session"));
        assert!(!is_not_found(&anyhow::Error::new(not_found("personality")), "session"), "another entity is missing");
        assert!(!is_not_found(&anyhow::Error::new(StoreError::Backend("x".to_string())), "session"));
    }
}
