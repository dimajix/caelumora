//! The command of `compaction` (issue 269): `/compact @agent`, which compacts the agent's context
//! when its owners ask, and tells the channel what it did.

use super::{nothing_to_compact, CompactionComponent};
use async_trait::async_trait;
use caelumora_agents::command::AgentCommand;
use caelumora_agents::turn::Turn;
use caelumora_api::command::{Command, CommandError, CommandResult};
use caelumora_domain::command::{CommandDescription, CommandOutput};
use caelumora_domain::value::command_name::CommandName;
use std::sync::Arc;

/// The name of its command, `/compact`.
const COMPACT_COMMAND_NAME: &str = "compact";

/// `/compact @agent`: compacts the agent's context now, as its component would once it is too
/// full.
#[derive(Debug)]
pub(super) struct CompactCommand {
    description: CommandDescription,
    /// What compacts the context.
    compaction: Arc<CompactionComponent>,
}

impl CompactCommand {
    /// The command, compacting with `compaction`.
    pub(super) fn new(compaction: Arc<CompactionComponent>) -> Self {
        let description = CommandDescription {
            name: CommandName::fixed(COMPACT_COMMAND_NAME),
            aliases: Vec::new(),
            description: "Compacts the agent's context: the older part of its conversation is replaced by a shorter one, the newest is kept. What it saw before stays recorded.".to_string(),
            usage: None,
            // Like approving its calls (issue 269): its owners'.
            owners_only: true,
        };
        Self { description, compaction }
    }
}

impl Command for CompactCommand {
    fn description(&self) -> &CommandDescription {
        &self.description
    }
}

#[async_trait]
impl AgentCommand for CompactCommand {
    async fn execute(&self, _args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        let name = COMPACT_COMMAND_NAME;
        let compacted = self
            .compaction
            .compact(turn)
            .await
            .map_err(|e| CommandError::Failed { command: name.to_string(), reason: e.to_string() })?
            .ok_or_else(|| CommandError::InvalidArguments {
                command: name.to_string(),
                reason: nothing_to_compact(turn.agent_name()),
            })?;
        Ok(CommandOutput::channel(compacted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::{component, nothing, Before};
    use crate::testing::{heard, two_calls, Scripted};
    use crate::CompactionError;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::command::CommandVisibility;
    use caelumora_kernel::workspace::testing::session_with_workspace;

    /// What `/compact` answers, after `before`, with `strategy`, and how often the strategy was
    /// asked.
    async fn compact(strategy: Scripted, before: Before) -> (CommandResult<CommandOutput>, usize) {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard("u1")]);
        let strategy = Arc::new(strategy);
        let command = CompactCommand::new(Arc::new(component(&strategy)));
        let mut turn = workspace_turn(&world, &mut state);
        before(&mut turn, world.agent);
        let result = command.execute("", &mut turn).await;
        (result, strategy.times_asked())
    }

    #[tokio::test]
    async fn compact_tells_the_channel_what_it_did() {
        let (answer, _) = compact(Scripted::default(), two_calls).await;
        let answer = answer.expect("compacted");
        assert_eq!(
            answer.text,
            "Jonathan compacted its context: 3 messages replaced, 2 kept as they were."
        );
        assert_eq!(answer.visibility, CommandVisibility::Channel, "everyone should know");
    }

    #[tokio::test]
    async fn compact_refuses_what_it_cannot_shorten() {
        let (refused, asked) = compact(Scripted::default(), nothing).await;
        assert!(
            matches!(
                refused,
                Err(CommandError::InvalidArguments { ref reason, .. })
                    if reason.contains("nothing to compact")
            ),
            "{refused:?}"
        );
        assert_eq!(asked, 0, "its strategy was not asked");
    }

    #[tokio::test]
    async fn compact_fails_with_the_reason_its_strategy_failed() {
        let failing = Scripted::failing(|| CompactionError::ModelUnreachable);
        let (failed, _) = compact(failing, two_calls).await;
        assert!(
            matches!(
                failed,
                Err(CommandError::Failed { ref reason, .. })
                    if reason == "its model could not be asked"
            ),
            "{failed:?}"
        );
    }
}
