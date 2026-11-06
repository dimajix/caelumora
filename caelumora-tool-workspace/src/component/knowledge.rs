//! The facet `KnowledgeRestorer` of `workspace` (issue 538): the files the agent knew in what a
//! compaction summarises - read or written whole by its file tools, or given again before -, given
//! again as they are now, where its channel may still use the workspace.

use super::{tool, WorkspaceComponent};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::tool::ToolError;
use caelumora_agents::turn::Turn;
use caelumora_api::terminal::WorkspacePath;
use caelumora_api::workspace::WorkspaceReader;
use caelumora_compaction::{CompactionError, GivenAgain, KnowledgeRestorer};
use caelumora_domain::context::KnowledgeSource;
use caelumora_domain::prompt_catalogue::context;
use caelumora_domain::store::{EntityKind, Stored};
use caelumora_domain::workspace::Workspace;
use tracing::warn;

#[async_trait]
impl KnowledgeRestorer for WorkspaceComponent {
    fn kind(&self) -> EntityKind {
        Workspace::KIND
    }

    /// The files of `given` in the workspace the agent may read where it is, as they are now, each
    /// under its path. One that is gone, no text, or larger than a result holds
    /// (`max_output_bytes`) is left out; so is every one where the workspace can no longer be used
    /// or its storage cannot be reached, which must not cost the compaction, as it does not cost
    /// the turn its instruction files (`prompt`).
    ///
    /// # Errors
    /// `Prompt` if the text a file is given under cannot be had.
    async fn give_again(
        &self,
        turn: &mut Turn<'_>,
        given: &[KnowledgeSource],
    ) -> Result<Vec<GivenAgain>, CompactionError> {
        let situation = turn.situation();
        let Some(reader) = situation.reader::<dyn WorkspaceReader>().await else {
            return Ok(Vec::new());
        };
        let (workspace, terminal) = match reader.terminal().await {
            Ok(reached) => reached,
            Err(error) => {
                warn!(workspace = %reader.id(), "Giving again the files of a workspace failed: {error}");
                return Ok(Vec::new());
            }
        };
        let seen = (workspace.id, &workspace.classification);
        let mut again = Vec::new();
        for known in given {
            let Some((id, _)) = known.about::<Workspace>() else { continue };
            let Some(Ok(at)) = known.place().map(WorkspacePath::parse) else { continue };
            if id != workspace.id {
                continue;
            }
            let (text, digest) = match tool::text(terminal.as_ref(), &at).await {
                Ok(read) => read,
                // Gone, no text, too large: the model reads it again if it needs it.
                Err(ToolError::InvalidParameter(_)) => continue,
                Err(error) => {
                    warn!(workspace = %workspace.id, "Giving again a file of a workspace failed: {error}");
                    continue;
                }
            };
            if text.len() > self.params.max_output_bytes {
                continue;
            }
            let path = at.to_string();
            let text = self
                .prompts
                .render(&format!("{NAMESPACE}.file"), context! { path => path.as_str(), text })?;
            let knowledge = tool::known(turn, seen, &at, Some(digest));
            again.push(GivenAgain { name: path, text, knowledge });
        }
        Ok(again)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::component;
    use caelumora_agents::testing::{self, workspace_turn};
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::digest;
    use caelumora_domain::workspace::WorkspaceId;
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use serde_json::json;
    use std::fs;

    /// Issue 538: the files the agent knew are given again as they are now, under their path, as
    /// knowledge of the workspace there; what is gone, too large, no text, no file of this
    /// workspace or no place within it is not.
    #[tokio::test]
    async fn the_files_known_are_given_again_as_they_are_now() {
        let world = session_with_workspace().await;
        fs::create_dir_all(world.dir().join("docs")).unwrap();
        fs::write(world.dir().join("docs/plan.md"), "# Plan\nnow").unwrap();
        fs::write(world.dir().join("big.txt"), "x".repeat(100)).unwrap();
        fs::write(world.dir().join("image.png"), b"\x89PNG\xff").unwrap();
        let mut state = AgentChannelState::default();
        let mut turn = workspace_turn(&world, &mut state);
        let restorer = component(json!({ "max_output_bytes": 64 }));
        let id = world.workspace.id;
        let once = Some(digest::content(b"# Plan\nthen"));
        let file = |place: &str| KnowledgeSource::within::<Workspace>(id, place.to_string(), once);
        let given = [
            file("docs/plan.md"),
            file("gone.txt"),
            file("big.txt"),
            file("image.png"),
            KnowledgeSource::within::<Workspace>(
                WorkspaceId::new(),
                "docs/plan.md".to_string(),
                once,
            ),
            KnowledgeSource::entity::<Workspace>(id, once),
        ];

        let again = restorer.give_again(&mut turn, &given).await.unwrap();
        let [plan] = again.as_slice() else { panic!("the plan alone: {again:?}") };
        assert_eq!(plan.name, "docs/plan.md");
        assert_eq!(
            plan.text, "[File: docs/plan.md]\n# Plan\nnow\n",
            "as it is now, under its path"
        );
        assert_eq!(
            plan.knowledge.source,
            KnowledgeSource::within::<Workspace>(
                id,
                "docs/plan.md".to_string(),
                Some(digest::content(b"# Plan\nnow"))
            ),
            "known as it is now"
        );
        assert_eq!(plan.knowledge.classification.as_ref(), Some(&world.workspace.classification));
    }

    /// Where the agent's channel may use no workspace, nothing is given again: all of it is lost.
    #[tokio::test]
    async fn nothing_is_given_again_where_no_workspace_may_be_used() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let mut state = AgentChannelState::default();
        let mut turn = testing::turn(&session, helper, world.alice, &mut state);
        let known = KnowledgeSource::within::<Workspace>(
            WorkspaceId::new(),
            "docs/plan.md".to_string(),
            Some(digest::content(b"x")),
        );

        let again = component(json!({})).give_again(&mut turn, &[known]).await.unwrap();
        assert!(again.is_empty(), "{again:?}");
    }
}
