//! The `compaction` component: an agent's context compacted, on its own and when asked (issues
//! 269, 284, 464). It lives in `component.rs`, since a module cannot hold a file of its own name.
//!
//! On its own, as an activity, once its context fills `threshold` of its model's context size;
//! when asked, by `/compact @agent`, a command the agent carries out itself, for its owners alone.
//! How the context is compacted is its [`CompactionStrategy`]'s in either case; the component
//! asks for it and tells the channel what it did. An agent whose personality does not list it
//! does not compact at all.
//!
//! How full the context is, is measured by the calls made on it and estimated beyond them
//! (`Situation::token_positions`); only once a call made on the active branch measured it, not
//! right after a compaction, whose branch no call measured yet. A model that reports no context
//! size gives nothing to measure against: its agent does not compact on its own, which the
//! component says once.

use crate::activity::{ActivityError, ActivityProvider};
use crate::command::AgentCommandProvider;
use crate::compaction::{CompactionError, CompactionStrategy};
use crate::situation::Situation;
use crate::turn::Turn;
use caelumora_runtime::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_runtime::component::{config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use async_trait::async_trait;
use caelumora_model::command::{Command, CommandOutput};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

pub const COMPACTION_KIND: &str = "compaction";
pub const COMPACT_COMMAND_NAME: &str = "compact";

/// What the agent is told it does as it compacts on its own.
const ACTIVITY: &str = "compacting its context";

/// How full the context may be before the agent compacts it, as a share of its model's context
/// size, unless configured.
const DEFAULT_THRESHOLD: f64 = 0.8;

/// Compaction of an agent's context by its strategy: on its own, as an activity, once the context
/// fills `threshold`, and by `/compact`.
#[derive(Debug)]
pub struct CompactionComponent {
    /// The agent's strategy, which compacts.
    strategy: Arc<dyn CompactionStrategy>,
    /// How full the context may be before the agent compacts it on its own, as a share of the
    /// context size.
    threshold: f64,
    /// Whether it said that the agent's model reports no context size.
    told_no_size: AtomicBool,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactionConfig {
    #[serde(default = "default_threshold")]
    threshold: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

impl CompactionComponent {
    fn new(strategy: Arc<dyn CompactionStrategy>, threshold: f64) -> Self {
        Self { strategy, threshold, told_no_size: AtomicBool::new(false) }
    }

    /// Compacts the context of `turn`'s agent as its strategy does: what it did, in words for its
    /// channel, or `None` if there was nothing to compact.
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<String>, CompactionError> {
        Ok(self.strategy.compact(turn).await?.map(|compaction| compaction.describe(turn.agent_name())))
    }
}

/// What the agent `agent_name`'s channel is told when there was nothing to compact.
fn nothing_to_compact(agent_name: &str) -> String {
    format!("{agent_name} found nothing to compact: it keeps the newest part of its context as it is")
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
        let compacted = self.compact(turn).await
            .map_err(|e| CommandError::Failed { command: name.to_string(), reason: e.to_string() })?
            .ok_or_else(|| invalid(&nothing_to_compact(turn.agent_name())))?;

        Ok(CommandOutput::channel(compacted))
    }
}

#[async_trait]
impl ActivityProvider for CompactionComponent {
    fn activity(&self) -> &str {
        ACTIVITY
    }

    fn is_due(&self, situation: &Situation<'_>) -> bool {
        // A text that cannot be had fails the next call of the model, not this.
        let Ok(Some((positions, true))) = situation.token_positions() else { return false };
        let tokens = positions.last().copied().unwrap_or(0);
        let size = situation.agent_channel_state().used.and_then(|usage| usage.context_size);
        let Some(size) = size.filter(|size| *size > 0) else {
            if !self.told_no_size.swap(true, Ordering::Relaxed) {
                info!("Agent {}: its model reports no context size; it does not compact its context on its own", situation.agent_id());
            }
            return false;
        };
        // Shares of token counts: what rounding loses is a token.
        tokens as f64 >= self.threshold * size as f64
    }

    async fn carry_out(&self, turn: &mut Turn<'_>) -> Result<String, ActivityError> {
        let compacted = self.compact(turn).await
            .map_err(|e| ActivityError::Failed(format!("{} could not compact its context: {e}", turn.agent_name())))?;
        Ok(compacted.unwrap_or_else(|| nothing_to_compact(turn.agent_name())))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(COMPACTION_KIND).provides::<dyn AgentCommandProvider>().provides::<dyn ActivityProvider>().needs::<dyn CompactionStrategy>(),
        |descriptor, context| async move {
            let CompactionConfig { threshold } = config_of_params(&descriptor.params)?;
            if !(threshold > 0.0 && threshold <= 1.0) {
                return Err(ComponentError::InvalidConfig(format!("'threshold' is a share of the context size, more than 0 and at most 1, not {threshold}")));
            }
            let strategy = context.one::<dyn CompactionStrategy>().await?.facet;
            let component = Arc::new(CompactionComponent::new(strategy, threshold));
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::<CompactionComponent>::clone(&component));
            facets.insert::<dyn ActivityProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::workspace_turn;
    use crate::compaction::Compaction;
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::command::CommandVisibility;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use serde_json::Value;
    use std::sync::atomic::AtomicUsize;

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

    /// The component with a strategy that answers with what `answer` returns, and that strategy.
    fn answering(answer: fn() -> Result<Option<Compaction>, CompactionError>) -> (CompactionComponent, Arc<Answering>) {
        let strategy = Arc::new(Answering { answer, asked: AtomicUsize::new(0) });
        (CompactionComponent::new(Arc::<Answering>::clone(&strategy), DEFAULT_THRESHOLD), strategy)
    }

    async fn compact(answer: fn() -> Result<Option<Compaction>, CompactionError>, args: &str) -> (CommandResult<CommandOutput>, usize) {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let (component, strategy) = answering(answer);
        let mut turn = workspace_turn(&world, &mut state);
        let result = component.execute(COMPACT_COMMAND_NAME, args, &mut turn).await;
        (result, strategy.asked.load(Ordering::SeqCst))
    }

    async fn carry_out(answer: fn() -> Result<Option<Compaction>, CompactionError>) -> Result<String, ActivityError> {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let (component, _) = answering(answer);
        component.carry_out(&mut workspace_turn(&world, &mut state)).await
    }

    #[tokio::test]
    async fn compact_tells_the_channel_what_its_strategy_did() {
        let (answer, _) = compact(|| Ok(Some(Compaction { replaced: 3, kept: 1, given_again: vec!["skill release-notes".to_string()], lost: 1 })), "").await;
        let answer = answer.expect("compacted");
        assert_eq!(answer.text, "Jonathan compacted its context: 3 messages replaced, 1 kept as they were. Given again: skill release-notes. \
            1 of what it had been given can no longer be given here and was not given again.");
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
        let (failed, _) = compact(|| Err(CompactionError::ModelUnreachable), "").await;
        assert!(matches!(failed, Err(CommandError::Failed { ref reason, .. }) if reason == "its model could not be asked"), "{failed:?}");
    }

    #[tokio::test]
    async fn compacting_on_its_own_tells_what_its_strategy_did_and_why_it_failed() {
        let compacted = carry_out(|| Ok(Some(Compaction { replaced: 2, kept: 3, given_again: Vec::new(), lost: 0 }))).await;
        assert_eq!(compacted.ok().as_deref(), Some("Jonathan compacted its context: 2 messages replaced, 3 kept as they were."));
        let nothing = carry_out(|| Ok(None)).await;
        assert!(matches!(nothing, Ok(ref said) if said.contains("found nothing to compact")), "{nothing:?}");
        let failed = carry_out(|| Err(CompactionError::ModelUnreachable)).await;
        assert!(matches!(failed, Err(ActivityError::Failed(ref reason)) if reason == "Jonathan could not compact its context: its model could not be asked"), "{failed:?}");
    }

    #[tokio::test]
    async fn it_is_due_once_a_call_measured_the_context_that_full() {
        let world = session_with_workspace().await;
        let (component, _) = answering(|| Ok(None));
        let heard = Message { content: Some("hello".to_string()), ..Message::new(Classification::PUBLIC) };
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard]);
        let mut turn = workspace_turn(&world, &mut state);
        assert!(!component.is_due(&turn.situation()), "nothing measured");

        let call = |prompt: u64, size: Option<u64>| Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(world.agent),
            role: MessageRole::Assistant,
            content: Some("x".to_string()),
            prompt_tokens: Some(prompt),
            completion_tokens: Some(1),
            context_size: size,
            ..Message::new(Classification::PUBLIC)
        };
        turn.push_message(call(700, Some(1000)));
        assert!(!component.is_due(&turn.situation()), "701 of 1000");
        turn.push_message(call(800, Some(1000)));
        assert!(component.is_due(&turn.situation()), "801 of 1000");
        turn.push_message(call(900, None));
        assert!(!component.is_due(&turn.situation()), "nothing to measure against without a size");
    }

    #[test]
    fn its_threshold_is_a_share_of_the_context_size() {
        let config = |params| config_of_params::<CompactionConfig>(&params).map(|c| c.threshold);
        assert_eq!(config(Value::Null).ok(), Some(DEFAULT_THRESHOLD));
        assert_eq!(config(serde_json::json!({ "threshold": 0.5 })).ok(), Some(0.5));
        assert!(config(serde_json::json!({ "keep": 0.5 })).is_err(), "what it keeps is its strategy's");
    }
}
