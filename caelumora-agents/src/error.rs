//! What a person is told of an error of the agents' logic (issue 370): the
//! error types of its facets - the model's (`LLMError`), an external
//! agent's (`ExternalAgentError`), a tool's (`ToolError`), a context's
//! (`ContextError`) and a message stage's (`ProcessingError`) - which the
//! plugins that serve those facets answer in (PLUGINS.md), judged here, by
//! whoever defines them; everything else as the runtime judges it
//! (`caelumora_runtime::error::user_message_with`).

use caelumora_runtime::error::{store_message, user_message_with, view_message};
use crate::external::ExternalAgentError;
use crate::llm::LLMError;
use crate::message::ProcessingError;
use crate::prompt::ContextError;
use crate::tool::ToolError;
use std::error::Error;

/// The message to show a user for `error`: the outermost error in its chain
/// whose type - the agents' logic's or the runtime's - is known to be safe
/// to disclose, or `None` if there is none.
pub fn user_message(error: &anyhow::Error) -> Option<String> {
    user_message_with(error, judged)
}

/// What the channel's users are told of `error`, which ended an answer: its
/// message where it is one for them (`user_message`), else that the details
/// are in the server log - which has them, and nothing else about the
/// server.
pub(crate) fn failure_message(error: &anyhow::Error) -> String {
    user_message(error).unwrap_or_else(|| "internal error - the details are in the server log".to_string())
}

/// What is shown of `e`, if it is of a type of the agents' logic that may be
/// shown.
fn judged(e: &(dyn Error + 'static)) -> Option<String> {
    if let Some(e) = e.downcast_ref::<LLMError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ExternalAgentError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ToolError>() {
        Some(e.to_string())
    } else if let Some(e) = e.downcast_ref::<ContextError>() {
        context_message(e)
    } else if let Some(e) = e.downcast_ref::<ProcessingError>() {
        match e {
            ProcessingError::Failed(_) => Some(e.to_string()),
            ProcessingError::View(e) => view_message(e),
            ProcessingError::Store(e) => store_message(e),
        }
    } else {
        None
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

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_runtime::component::ComponentError;
    use caelumora_model::store::StoreError;
    use anyhow::Error;

    #[test]
    fn finds_the_disclosable_error_inside_a_context_chain() {
        let error = Error::new(LLMError::Connection {
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
    fn a_prompt_part_that_failed_names_its_component_before_what_it_says() {
        let failed = |cause| Error::new(ContextError::Component { component: "skills".to_string(), cause: Box::new(cause) })
            .context("component 'context' could not assemble the context");

        let error = failed(ContextError::Failed("skills are not offered here".to_string()));
        assert_eq!(user_message(&error).as_deref(), Some("component 'skills' failed: skills are not offered here"));
        // What the store reported is judged as store errors are.
        let error = failed(ContextError::Store(StoreError::Backend("/home/someone/local/entity_store: permission denied".to_string())));
        assert_eq!(user_message(&error), None);
    }

    /// The runtime's types are judged as the runtime judges them.
    #[test]
    fn the_runtime_s_errors_are_judged_as_the_runtime_judges_them() {
        let component = ComponentError::InvalidConfig("missing field `url`".to_string());
        let shown = component.to_string();
        assert_eq!(user_message(&Error::new(component).context("building the agent")), Some(shown));
        let backend = Error::new(StoreError::Backend("/home/someone/local/entity_store: permission denied".to_string()));
        assert_eq!(user_message(&backend), None, "not what the runtime does not show");
        assert_eq!(failure_message(&anyhow::anyhow!("something internal")), "internal error - the details are in the server log");
    }
}
