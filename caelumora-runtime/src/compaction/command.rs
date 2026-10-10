//! The `compaction` component: `/compact @agent`, a command the agent
//! carries out itself (issue 269), for its owners alone. How its context is
//! compacted is its [`CompactionStrategy`]'s; the command asks for it and
//! tells the channel what it did.

use crate::agent::turn::Turn;
use crate::command::{AgentCommandProvider, CommandError, CommandResult, ProvidesCommands};
use crate::compaction::CompactionStrategy;
use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput};
use std::sync::Arc;

pub const COMPACTION_KIND: &str = "compaction";
pub const COMPACT_COMMAND_NAME: &str = "compact";

#[derive(Debug)]
pub struct CompactionComponent {
    /// The agent's strategy, which compacts.
    strategy: Arc<dyn CompactionStrategy>,
}

impl ProvidesCommands for CompactionComponent {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: COMPACT_COMMAND_NAME.to_string(),
            description: "Compacts the agent's context: the older part of its conversation is replaced by a shorter one, the newest is kept. What it saw before stays recorded.".to_string(),
            usage: None,
            // Like approving its calls (issue 269): its owners'.
            owners_only: true,
        }]
    }
}

#[async_trait]
impl AgentCommandProvider for CompactionComponent {
    async fn execute(&self, name: &str, args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        if name != COMPACT_COMMAND_NAME {
            return Err(CommandError::Unknown(name.to_string()));
        }
        let invalid = |reason: &str| CommandError::InvalidArguments { command: name.to_string(), reason: reason.to_string() };
        if !args.trim().is_empty() {
            return Err(invalid("it takes no arguments"));
        }
        let compaction = self.strategy.compact(turn).await
            .map_err(|e| CommandError::Failed { command: name.to_string(), reason: e.to_string() })?
            .ok_or_else(|| invalid(&format!("there is nothing to compact: {} keeps the newest part of its context as it is", turn.agent_name())))?;

        let mut answer = format!("{} compacted its context: {} messages replaced, {} kept as they were.", turn.agent_name(), compaction.replaced, compaction.kept);
        if !compaction.skills_given.is_empty() {
            answer.push_str(&format!(" Skills given again: {}.", compaction.skills_given.join(", ")));
        }
        if compaction.skills_lost > 0 {
            answer.push_str(&format!(" {} skill(s) it had loaded can no longer be loaded here and were not given again.", compaction.skills_lost));
        }
        Ok(CommandOutput::channel(answer))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(COMPACTION_KIND).provides::<dyn AgentCommandProvider>(),
        |_descriptor, context| async move {
            let strategy = context.one::<dyn CompactionStrategy>().await?.facet;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::new(CompactionComponent { strategy }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::{Compaction, CompactionError};
    use crate::workspace::testing::session_with_workspace;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::command::CommandVisibility;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A strategy that answers with what `answer` returns, counting how
    /// often it was asked.
    #[derive(Debug)]
    struct Answering {
        answer: fn() -> Result<Option<Compaction>, CompactionError>,
        asked: AtomicUsize,
    }

    #[async_trait]
    impl CompactionStrategy for Answering {
        async fn compact(&self, _: &mut Turn<'_>) -> Result<Option<Compaction>, CompactionError> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            (self.answer)()
        }
    }

    async fn compact(answer: fn() -> Result<Option<Compaction>, CompactionError>, args: &str) -> (CommandResult<CommandOutput>, usize) {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let strategy = Arc::new(Answering { answer, asked: AtomicUsize::new(0) });
        let component = CompactionComponent { strategy: Arc::<Answering>::clone(&strategy) };
        let mut turn = world.turn(&mut state);
        let result = component.execute(COMPACT_COMMAND_NAME, args, &mut turn).await;
        (result, strategy.asked.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn compact_tells_the_channel_what_its_strategy_did() {
        let (answer, _) = compact(|| Ok(Some(Compaction { replaced: 3, kept: 1, skills_given: vec!["release-notes".to_string()], skills_lost: 1 })), "").await;
        let answer = answer.expect("compacted");
        assert_eq!(answer.text, "Jonathan compacted its context: 3 messages replaced, 1 kept as they were. Skills given again: release-notes. \
            1 skill(s) it had loaded can no longer be loaded here and were not given again.");
        assert_eq!(answer.visibility, CommandVisibility::Channel, "everyone should know");
    }

    #[tokio::test]
    async fn compact_refuses_what_its_strategy_cannot_shorten_and_takes_no_arguments() {
        let (refused, _) = compact(|| Ok(None), "").await;
        assert!(matches!(refused, Err(CommandError::InvalidArguments { ref reason, .. }) if reason.contains("nothing to compact")), "{refused:?}");
        let (refused, asked) = compact(|| Ok(None), "hard").await;
        assert!(matches!(refused, Err(CommandError::InvalidArguments { ref reason, .. }) if reason.contains("no arguments")), "{refused:?}");
        assert_eq!(asked, 0, "its strategy was not asked");
    }

    #[tokio::test]
    async fn compact_fails_with_the_reason_its_strategy_failed() {
        let (failed, _) = compact(|| Err(CompactionError::CallLimitReached), "").await;
        assert!(matches!(failed, Err(CommandError::Failed { ref reason, .. }) if reason == "the agent made all the calls of its model it may in one turn"), "{failed:?}");
    }
}
