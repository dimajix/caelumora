//! The component kind `attachments` (issues 282, 533, 534): what it is built of and how - its
//! tools, with which an agent reaches the attachments it heard by the numbers its transcript gave
//! them, are `tool`'s; the component is their `ToolProvider`, which offers each where it can be
//! used.

mod tool;

use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::tool::{tool_text, Tool, ToolProvider};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::workspace::{WorkspaceReader, WorkspaceWriter};
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
const ATTACHMENTS_KIND: &str = "attachments";

/// A component of the kind `attachments`: the tools `view_attachment`, `read_attachment`,
/// `copy_attachment` and `attach` over the attachments its agent heard - each held apart where
/// what it is offered by differs (`ToolProvider::offered`).
#[derive(Debug)]
struct AttachmentsComponent {
    /// What its tool set is for, as the prompt catalogue words it.
    description: String,
    /// `view_attachment`, offered to a model that sees images.
    view: Arc<dyn Tool>,
    /// `read_attachment`, offered always.
    read: Arc<dyn Tool>,
    /// `copy_attachment`, offered where a workspace may be written to.
    copy: Arc<dyn Tool>,
    /// `attach`, offered always.
    attach: Arc<dyn Tool>,
}

impl AttachmentsComponent {
    /// The component, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(prompts: &PromptTexts) -> Result<Self, PromptError> {
        let description = tool_text(prompts, NAMESPACE, ATTACHMENTS_KIND, "description", ())?;
        Ok(Self {
            description,
            view: Arc::new(tool::ViewAttachmentTool::new(prompts)?),
            read: Arc::new(tool::ReadAttachmentTool::new(prompts)?),
            copy: Arc::new(tool::CopyAttachmentTool::new(prompts)?),
            attach: Arc::new(tool::AttachTool::new(prompts)?),
        })
    }
}

/// Registers the component kind `attachments`, which reads and writes the agent's workspace and
/// writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(ATTACHMENTS_KIND)
                .provides::<dyn ToolProvider>()
                .reads::<dyn WorkspaceReader>()
                .writes::<dyn WorkspaceWriter>(),
        ),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(AttachmentsComponent::new(&texts)?));
            Ok(facets)
        },
    );
}
