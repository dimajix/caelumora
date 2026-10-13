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
use crate::context::ContextError;
use crate::message::ProcessingError;
use crate::external::ExternalAgentError;
use crate::llm::LLMError;
use crate::session::SessionError;
use crate::tool::ToolError;
use crate::view::{AgentContextError, ViewError};
use crate::channel::ChannelError;
use crate::terminal::TerminalError;
use crate::workspace::WorkspaceError;
use caelumora_model::security::Refused;
use caelumora_model::store::StoreError;
use caelumora_model::template::TemplateError;
use caelumora_model::value::ValueError;

/// The message to show a user for `error`: the outermost error in its chain
/// whose type is known to be safe to disclose, or `None` if there is none -
/// in which case the caller should show a generic message and point to the
/// log.
pub fn user_message(error: &anyhow::Error) -> Option<String> {
    error.chain().find_map(|e| {
        if let Some(e) = e.downcast_ref::<LLMError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ExternalAgentError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ComponentError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ToolError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<CommandError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<AgentContextError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ViewError>() {
            view_message(e)
        } else if let Some(e) = e.downcast_ref::<ContextError>() {
            context_message(e)
        } else if let Some(e) = e.downcast_ref::<ProcessingError>() {
            match e {
                ProcessingError::Failed(_) => Some(e.to_string()),
                ProcessingError::View(e) => view_message(e),
                ProcessingError::Store(e) => store_message(e),
            }
        } else if let Some(e) = e.downcast_ref::<ChannelError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<SessionError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<WorkspaceError>() {
            // Its store errors are judged as store errors are; the rest name
            // the workspace and what the caller wrote, never a place.
            match e {
                WorkspaceError::Store(e) => store_message(e),
                _ => Some(e.to_string()),
            }
        } else if let Some(e) = e.downcast_ref::<crate::artifact::ArtifactError>() {
            // As for workspaces: what it names is safe, never a place.
            match e {
                crate::artifact::ArtifactError::Store(e) => store_message(e),
                _ => Some(e.to_string()),
            }
        } else if let Some(e) = e.downcast_ref::<TerminalError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<TemplateError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<ValueError>() {
            Some(e.to_string())
        } else if let Some(e) = e.downcast_ref::<StoreError>() {
            store_message(e)
        } else {
            None
        }
    })
}

/// View errors: what they name is safe; why reaching it failed is for the
/// log.
fn view_message(error: &ViewError) -> Option<String> {
    match error {
        ViewError::Failed(_) => None,
        _ => Some(error.to_string()),
    }
}

/// What building a context failed with, each variant judged by what it
/// holds - a plugin's provider has decided there what may be shown (issue
/// 336); the component that failed named before its cause.
fn context_message(error: &ContextError) -> Option<String> {
    match error {
        ContextError::Failed(_) => Some(error.to_string()),
        ContextError::View(e) => view_message(e),
        ContextError::Store(e) => store_message(e),
        ContextError::Component { cause, .. } => context_message(cause).map(|cause| format!("{error}: {cause}")),
    }
}

/// Store errors name entities and ids - except `Backend`, which wraps what
/// the storage itself reported, file paths included.
fn store_message(error: &StoreError) -> Option<String> {
    match error {
        StoreError::Backend(_) => None,
        _ => Some(error.to_string()),
    }
}

/// What the `SecurityManager` refused, if `error` is a refusal - which a
/// caller answers with "not allowed", not with "something broke". Found
/// wherever in its chain it is (issue 323): every error that ends in a
/// refusal carries it as its source.
pub fn refusal(error: &anyhow::Error) -> Option<&Refused> {
    error.chain().find_map(|e| e.downcast_ref::<Refused>())
}

/// Whether `error` is a refusal by the `SecurityManager`.
pub fn is_forbidden(error: &anyhow::Error) -> bool {
    refusal(error).is_some()
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
    fn a_prompt_part_that_failed_names_its_component_before_what_it_says() {
        let failed = |cause| anyhow::Error::new(ContextError::Component { component: "skills".to_string(), cause: Box::new(cause) })
            .context("component 'context' could not assemble the context");

        let error = failed(ContextError::Failed("skills are not offered here".to_string()));
        assert_eq!(user_message(&error).as_deref(), Some("component 'skills' failed: skills are not offered here"));
        // What the store reported is judged as store errors are.
        let error = failed(ContextError::Store(StoreError::Backend("/home/someone/local/entity_store: permission denied".to_string())));
        assert_eq!(user_message(&error), None);
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
