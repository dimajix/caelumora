//! The `workspace_prompt_provider` component (issue 251, part C7): tells the
//! agent which workspace it works on in its channel - its name, how it is
//! classified, what it holds as its owner describes it, and whether it can
//! run commands there - or that it has none here, so that it does not try.
//!
//! It says what is there, whatever tools a personality has to work on it,
//! so a personality with several gets it once. Read afresh at the start of
//! every turn through the agent's views: the workspace the channel may use
//! now (`ChannelView::workspace`). In the system prompt by default
//! (parameter `position`): it changes rarely. The part is classified as the
//! workspace, which flows into the channel where it may be used there.

use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::context::{titled, Prompt, PromptPosition, PromptProvider};
use crate::view::WorkspaceView;
use crate::agent::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use std::sync::Arc;

pub const WORKSPACE_PROMPT_PROVIDER_KIND: &str = "workspace_prompt_provider";

const TITLE: &str = "Your workspace";

#[derive(Debug)]
pub struct WorkspacePromptProvider {
    pub position: PromptPosition,
}

/// What the agent is told of `workspace`.
fn told(workspace: &WorkspaceView) -> String {
    let mut text = format!(
        "You work on the files of the workspace \"{}\" of this session, which you share with the people and agents of \
        the channels that use it. Everything in it is classified {}.\n",
        workspace.name(),
        workspace.classification(),
    );
    if let Some(description) = workspace.description() {
        text.push_str(&format!("What it holds, as its owner describes it: {description}\n"));
    }
    text.push_str(if workspace.has_shell() {
        "You can run shell commands in it.\n"
    } else {
        "Its storage gives it no shell: you cannot run commands in it.\n"
    });
    titled(TITLE, &text)
}

#[async_trait]
impl PromptProvider for WorkspacePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        // Work outside of a channel, in tests, has no workspace to tell of.
        let Some(session) = work.session() else { return Ok(Prompt::classified(String::new(), Classification::PUBLIC)) };
        let channel = session.channel(work.channel_id()).await?;
        Ok(match channel.workspace().await? {
            Some(workspace) => Prompt::classified(told(&workspace), workspace.classification().clone()),
            None => Prompt::classified(
                titled(TITLE, "There is no workspace you can use in this channel: no files to work on, no commands to run.\n"),
                Classification::PUBLIC,
            ),
        })
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(WORKSPACE_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        |descriptor, _context| {
            let position = PromptPosition::of_params(&descriptor.params)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(WorkspacePromptProvider { position }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::security::SecurityContext;

    #[tokio::test]
    async fn the_agent_is_told_of_the_workspace_it_may_use_here_or_that_it_has_none() {
        let world = session_with_workspace().await;
        let provider = WorkspacePromptProvider { position: PromptPosition::Start };
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.starts_with("[Your workspace]") && prompt.text.contains("\"docs\"") && prompt.text.contains("Drafts") && prompt.text.contains("You can run shell commands"), "{}", prompt.text);
        assert_eq!(prompt.classification.as_ref(), Some(&world.workspace.classification));

        world.session.channels()[0].update_uses_workspace(&SecurityContext::system(), false).await.unwrap();
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&world.turn(&mut state).work()).await.unwrap();
        assert!(prompt.text.contains("There is no workspace you can use in this channel"), "{}", prompt.text);
        assert!(!prompt.text.contains("docs"), "nor which there is: {}", prompt.text);
    }
}
