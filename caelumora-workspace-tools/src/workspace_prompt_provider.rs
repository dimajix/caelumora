//! The `workspace_prompt_provider` component (issue 251, part C7): tells the
//! agent which workspace it works on in its channel - its name, how it is
//! classified, what it holds as its owner describes it, and whether it can
//! run commands there - or that it has none here, so that it does not try.
//!
//! It says what is there, whatever tools a personality has to work on it,
//! so a personality with several gets it once. Read afresh at the start of
//! every turn through the agent's views: the workspace the channel may use
//! now (`Situation::workspace_view`). In the system prompt by default
//! (parameter `position`): it changes rarely. The part is classified as the
//! workspace, which flows into the channel where it may be used there.
//!
//! It names the files at the workspace's root that say how to work on it
//! (issue 279) - those of the parameter `instruction_files` that are there,
//! by default `AGENTS.md` and `CLAUDE.md` - and asks the agent to read them
//! with `read_file`; their content does not go into the prompt, so that
//! the part changes only when one appears or goes. That a directory may have
//! its own is said in one sentence, without looking for them. A storage
//! that cannot be reached leaves the files out, not the turn.

use crate::terminal_tool::READ_FILE;
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::prompt::{
    texts_of, writing_for_the_model, ContextError, Prompt, PromptPosition, PromptProvider,
};
use caelumora_agents::situation::Situation;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_runtime::component::{
    config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry,
    ComponentResult, Params,
};
use caelumora_runtime::terminal::{TerminalError, WorkspacePath};
use caelumora_runtime::view::WorkspaceView;
use futures::future;
use serde::Deserialize;
use std::sync::Arc;
use tracing::warn;

pub const WORKSPACE_PROMPT_PROVIDER_KIND: &str = "workspace_prompt_provider";

#[derive(Debug)]
pub struct WorkspacePromptProvider {
    pub position: PromptPosition,
    /// The names of the files that say how to work on a workspace, each a
    /// file at its root, in the order they are named.
    instruction_files: Vec<WorkspacePath>,
    /// The texts of its part (issue 57).
    prompts: PromptTexts,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspacePromptConfig {
    #[serde(default)]
    position: PromptPosition,
    #[serde(default = "default_instruction_files")]
    instruction_files: Vec<String>,
}

impl WorkspacePromptConfig {
    /// Its `instruction_files`, each as a file at a workspace's root.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a name that is not one of a file at the root.
    fn instruction_files(&self) -> ComponentResult<Vec<WorkspacePath>> {
        self.instruction_files
            .iter()
            .map(|name| match WorkspacePath::parse(name) {
                Ok(path) if !path.is_root() && path.parent().is_root() => Ok(path),
                _ => Err(ComponentError::InvalidConfig(format!(
                    "instruction file '{name}' is not the name of a file at the workspace's root"
                ))),
            })
            .collect()
    }
}

impl Params for WorkspacePromptConfig {
    /// # Errors
    /// `InvalidConfig` for an instruction file that is not one at a workspace's root.
    fn check(&self) -> ComponentResult<()> {
        self.instruction_files().map(drop)
    }
}

fn default_instruction_files() -> Vec<String> {
    vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]
}

impl WorkspacePromptProvider {
    /// Those of the instruction files that lie at the root of `workspace`,
    /// the one `situation` may use in its channel; none if its storage cannot be
    /// reached, which must not cost the turn.
    async fn instruction_files_in(
        &self,
        workspace: &WorkspaceView,
        situation: &Situation<'_>,
    ) -> Vec<&WorkspacePath> {
        if self.instruction_files.is_empty() {
            return Vec::new();
        }
        let terminal = match situation.workspace_terminal().await {
            Ok((_, terminal)) => terminal,
            Err(error) => {
                warn!(workspace = %workspace.id(), "Looking for the instruction files of a workspace failed: {error}");
                return Vec::new();
            }
        };
        let found = future::join_all(self.instruction_files.iter().map(|file| {
            let terminal = Arc::clone(&terminal);
            async move { (file, terminal.stat(file).await) }
        }))
        .await;
        found.into_iter()
            .filter_map(|(file, entry)| match entry {
                Ok(entry) => (!entry.is_dir).then_some(file),
                Err(TerminalError::NotFound(_)) => None,
                Err(error) => {
                    warn!(workspace = %workspace.id(), "Looking for an instruction file of a workspace failed: {error}");
                    None
                }
            })
            .collect()
    }
}

/// What the agent is told of `workspace`, at whose root lie `found` of the
/// instruction files `names`, and which it may change if `writable` (issue
/// 23), worded by `prompts`.
fn told(
    prompts: &PromptTexts,
    workspace: &WorkspaceView,
    writable: bool,
    names: &[WorkspacePath],
    found: &[&WorkspacePath],
) -> Result<String, PromptError> {
    let listed = |paths: &mut dyn Iterator<Item = &WorkspacePath>| {
        paths.map(|path| path.to_string()).collect::<Vec<_>>()
    };
    prompts.render(
        &format!("{NAMESPACE}.workspace"),
        context! {
            name => workspace.name(),
            classification => workspace.classification().to_string(),
            description => workspace.description().map(|description| description.to_string()),
            writable,
            shell => workspace.has_shell(),
            found => listed(&mut found.iter().copied()),
            names => listed(&mut names.iter()),
            read_file => READ_FILE,
        },
    )
}

#[async_trait]
impl PromptProvider for WorkspacePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        Ok(match situation.workspace_view().await? {
            Some(workspace) => {
                let found = self.instruction_files_in(&workspace, situation).await;
                let writable = workspace.may_write().await;
                Prompt::classified(
                    told(&self.prompts, &workspace, writable, &self.instruction_files, &found)?,
                    workspace.classification().clone(),
                )
            }
            None => Prompt::classified(
                self.prompts.render(&format!("{NAMESPACE}.no_workspace"), ())?,
                Classification::PUBLIC,
            ),
        })
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(WORKSPACE_PROMPT_PROVIDER_KIND)
                .params::<WorkspacePromptConfig>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let config: WorkspacePromptConfig = config_of_params(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(WorkspacePromptProvider {
                position: config.position,
                instruction_files: config.instruction_files()?,
                prompts: texts,
            }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::security::SecurityContext;
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use serde_json::{json, Value};
    use std::fs;

    fn provider() -> WorkspacePromptProvider {
        provider_of(json!({}))
    }

    /// It with `params`.
    fn provider_of(params: Value) -> WorkspacePromptProvider {
        let config: WorkspacePromptConfig = config_of_params(&params).unwrap();
        WorkspacePromptProvider {
            position: PromptPosition::Start,
            instruction_files: config.instruction_files().unwrap(),
            prompts: prompts(),
        }
    }

    #[tokio::test]
    async fn the_agent_is_told_of_the_workspace_it_may_use_here_or_that_it_has_none() {
        let world = session_with_workspace().await;
        let provider = provider();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.starts_with("[Your workspace]")
                && prompt.text.contains("\"docs\"")
                && prompt.text.contains("Drafts")
                && prompt.text.contains("You can run shell commands"),
            "{}",
            prompt.text
        );
        assert_eq!(prompt.classification.as_ref(), Some(&world.workspace.classification));

        world.session.channels()[0]
            .update_uses_workspace(&SecurityContext::system(), false)
            .await
            .unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("There is no workspace you can use in this channel"),
            "{}",
            prompt.text
        );
        assert!(!prompt.text.contains("docs"), "nor which there is: {}", prompt.text);
    }

    #[tokio::test]
    async fn the_agent_is_asked_to_read_the_instruction_files_at_the_root_and_only_those_there() {
        let world = session_with_workspace().await;
        let provider = provider();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            !prompt.text.contains("At its top"),
            "none is there, none is named: {}",
            prompt.text
        );
        assert!(
            prompt.text.contains("may hold a file of its own named `AGENTS.md` or `CLAUDE.md`")
                && prompt.text.contains("takes precedence"),
            "{}",
            prompt.text
        );

        fs::write(world.dir().join("AGENTS.md"), "Run the tests.").unwrap();
        fs::create_dir(world.dir().join("CLAUDE.md")).unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lies `AGENTS.md`,")
                && prompt.text.contains("Read it with `read_file`"),
            "{}",
            prompt.text
        );
        assert!(!prompt.text.contains("Run the tests"), "not its content: {}", prompt.text);

        fs::remove_dir(world.dir().join("CLAUDE.md")).unwrap();
        fs::write(world.dir().join("CLAUDE.md"), "See AGENTS.md").unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lie `AGENTS.md` and `CLAUDE.md`,")
                && prompt.text.contains("Read them"),
            "every one found: {}",
            prompt.text
        );
    }

    #[tokio::test]
    async fn the_instruction_files_are_as_configured() {
        let world = session_with_workspace().await;
        fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        fs::write(world.dir().join("GUIDE.md"), "").unwrap();
        let provider = provider_of(json!({ "instruction_files": ["GUIDE.md"] }));
        let mut state = AgentChannelState::default();
        let prompt =
            provider.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("At its top lies `GUIDE.md`,")
                && !prompt.text.contains("AGENTS.md"),
            "{}",
            prompt.text
        );

        let none = provider_of(json!({ "instruction_files": [] }));
        let mut state = AgentChannelState::default();
        let prompt =
            none.get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            !prompt.text.contains("At its top") && !prompt.text.contains("of its own"),
            "{}",
            prompt.text
        );

        for invalid in ["", "docs/AGENTS.md", "../AGENTS.md", "/AGENTS.md"] {
            let params = json!({ "instruction_files": [invalid] });
            assert!(
                matches!(
                    config_of_params::<WorkspacePromptConfig>(&params),
                    Err(ComponentError::InvalidConfig(_))
                ),
                "{invalid:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_storage_that_cannot_be_reached_leaves_the_files_out_not_the_turn() {
        let world = session_with_workspace().await;
        fs::write(world.dir().join("AGENTS.md"), "").unwrap();
        fs::remove_dir_all(world.dir()).unwrap();
        let mut state = AgentChannelState::default();
        let prompt =
            provider().get_prompt(&workspace_turn(&world, &mut state).situation()).await.unwrap();
        assert!(
            prompt.text.contains("\"docs\"") && !prompt.text.contains("At its top"),
            "{}",
            prompt.text
        );
    }
}
