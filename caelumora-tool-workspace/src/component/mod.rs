//! The component kind `workspace` (issues 251, part C, 532 and 534): the session's workspace, for
//! the agents of the channels that may use it - what it is configured with: how long a command is
//! followed, how much a result holds, which calls ask, where its prompt part goes and which files
//! say how to work on a workspace. It serves three facets: the tools on the workspace's files and
//! its shell (`tool`), the prompt part that tells the agent which workspace that is (`prompt`), and
//! the files the agent knew given again after a compaction (`knowledge`, issue 538).
//! Generic over the terminal: what it can do is what the workspace's storage gives, and the
//! component knows no provider.

mod knowledge;
mod prompt;
#[cfg(test)]
mod testing;
mod tool;

use crate::NAMESPACE;
use caelumora_agents::prompt::{texts_of, writing_for_the_model, PromptPosition, PromptProvider};
use caelumora_agents::tool::{tool_text, ApprovalMode, ToolProvider};
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_api::terminal::WorkspacePath;
use caelumora_api::workspace::{WorkspaceReader, WorkspaceWriter};
use caelumora_compaction::KnowledgeRestorer;
use caelumora_domain::prompt_catalogue::{PromptError, PromptTexts};
use caelumora_domain::tool::Tool;
use serde::Deserialize;
use std::sync::Arc;

/// The kind of the component, and the path of its texts in the catalogue.
pub const WORKSPACE_KIND: &str = "workspace";

fn default_wait_secs() -> u64 {
    60
}

fn default_output_bytes() -> usize {
    64 * 1024
}

fn default_instruction_files() -> Vec<String> {
    vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceParams {
    /// How long a call follows a command at most, in seconds: 60 unless
    /// configured, never more than 300.
    #[serde(default = "default_wait_secs")]
    max_wait_secs: u64,
    /// How much of a command's output, or of a file, one result holds at
    /// most. 64 KiB unless configured.
    #[serde(default = "default_output_bytes")]
    max_output_bytes: usize,
    /// Which calls need approval - unless configured, all that may: a
    /// command may act anywhere its place reaches (issue 150).
    /// `"unless_read_only"` spares the tools that only read files.
    /// `command_output` and `kill_command` concern a command that ran
    /// already, and never do; nor does `publish_file`, which gives nothing
    /// away (issue 275).
    #[serde(default)]
    requires_approval: ApprovalMode,
    /// What the tool set is for, written for a model (issue 194); a text of
    /// its own unless configured.
    #[serde(default)]
    description: Option<String>,
    /// Where its prompt part goes; the system prompt unless configured.
    #[serde(default)]
    position: PromptPosition,
    /// The names of the files that say how to work on a workspace, each a file at its root:
    /// `AGENTS.md` and `CLAUDE.md` unless configured.
    #[serde(default = "default_instruction_files")]
    instruction_files: Vec<String>,
}

impl WorkspaceParams {
    /// Its `instruction_files`, each as a file at a workspace's root - every one, since `check`
    /// refuses a name that is none.
    fn instruction_files(&self) -> Vec<WorkspacePath> {
        self.instruction_files.iter().filter_map(|name| at_root(name)).collect()
    }
}

impl Params for WorkspaceParams {
    /// # Errors
    /// `InvalidConfig` for an instruction file that is not one at a workspace's root.
    fn check(&self) -> ComponentResult<()> {
        match self.instruction_files.iter().find(|name| at_root(name).is_none()) {
            Some(name) => Err(ComponentError::InvalidConfig(format!(
                "instruction file '{name}' is not the name of a file at the workspace's root"
            ))),
            None => Ok(()),
        }
    }
}

/// The file at a workspace's root named `name`; `None` if `name` names none there.
fn at_root(name: &str) -> Option<WorkspacePath> {
    WorkspacePath::parse(name).ok().filter(|path| !path.is_root() && path.parent().is_root())
}

/// A component of the kind `workspace`: the tools on the session's workspace and the prompt part
/// that tells the agent which workspace that is.
#[derive(Debug)]
struct WorkspaceComponent {
    params: WorkspaceParams,
    /// What its tool set is for and its tools, as the configuration or the
    /// prompt catalogue words them (issue 57).
    description: String,
    tools: Vec<Tool>,
    /// The files that say how to work on a workspace, each a file at its root, in the order they
    /// are named.
    instruction_files: Vec<WorkspacePath>,
    /// What its prompt part says, and what its results say of how to go on.
    prompts: PromptTexts,
}

impl WorkspaceComponent {
    /// The component, configured by `params`, its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    fn new(params: WorkspaceParams, prompts: PromptTexts) -> Result<Self, PromptError> {
        let tools = tool::tools(&params, &prompts)?;
        let description = match &params.description {
            Some(description) => description.clone(),
            None => tool_text(&prompts, NAMESPACE, WORKSPACE_KIND, "description", ())?,
        };
        let instruction_files = params.instruction_files();
        Ok(Self { params, description, tools, instruction_files, prompts })
    }
}

/// Registers the component kind `workspace`, which reads and writes the agent's workspace and
/// writes for the agent's model.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(WORKSPACE_KIND)
                .params::<WorkspaceParams>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>()
                .provides::<dyn KnowledgeRestorer>()
                .reads::<dyn WorkspaceReader>()
                .writes::<dyn WorkspaceWriter>(),
        ),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let params = WorkspaceParams::read(&descriptor.params)?;
            let component = Arc::new(WorkspaceComponent::new(params, prompts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<WorkspaceComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(Arc::<WorkspaceComponent>::clone(&component));
            facets.insert::<dyn KnowledgeRestorer>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue;
    use caelumora_agents::testing::with_agent_kinds;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::type_map::TypeKey;
    use caelumora_kernel::component::BuildContext;
    use caelumora_kernel::component::ComponentRegistry;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::{EntityStores, Persistence};
    use serde_json::json;

    /// Written by hand without `params`, as the seed does: every parameter
    /// takes its default, and it provides its tools and its prompt part.
    #[tokio::test]
    async fn is_built_without_parameters() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        // The agents' kinds, with a model whose profile names the variant.
        with_agent_kinds(&mut registry);
        register(&mut registry.library_components());
        let config = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("workspace", WORKSPACE_KIND)],
        };
        let world = session_with_workspace().await;
        // The world runs the kernel alone: the texts of these tools are added.
        let context = BuildContext::new(
            SecurityContext::default(),
            world.kernel.services().clone().with(catalogue()),
        );
        let facets = [TypeKey::of::<dyn ToolProvider>(), TypeKey::of::<dyn PromptProvider>()];
        let container = (Arc::new(registry)).build(&config, &facets, &context).await.unwrap();
        assert_eq!(container.get_all::<dyn ToolProvider>(None).unwrap().len(), 1, "its tools");
        assert_eq!(container.get_all::<dyn PromptProvider>(None).unwrap().len(), 1, "its part");
    }

    #[test]
    fn an_instruction_file_is_one_at_the_root() {
        for invalid in ["", "docs/AGENTS.md", "../AGENTS.md", "/AGENTS.md"] {
            let params = json!({ "instruction_files": [invalid] });
            assert!(
                matches!(WorkspaceParams::read(&params), Err(ComponentError::InvalidConfig(_))),
                "{invalid:?}"
            );
        }
    }
}
