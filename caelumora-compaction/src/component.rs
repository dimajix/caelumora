//! The `compaction` component: an agent's context compacted, on its own and when asked (issues
//! 269, 284, 464).
//!
//! On its own, as an activity, once its context fills `threshold` of its model's context size;
//! when asked, by `/compact @agent`, a command the agent carries out itself, for its owners alone.
//! Either way it keeps `keep` of the context size as it stands, below `threshold`, and has the
//! rest replaced by what its [`CompactionStrategy`] puts there (`Compactor`); it tells the channel
//! what it did. An agent whose personality does not list it does not compact at all.
//!
//! How full the context is, is measured by the calls made on it and estimated beyond them
//! (`Situation::token_positions`); only once a call made on the active branch measured it, not
//! right after a compaction, whose branch no call measured yet. A model that reports no context
//! size gives nothing to measure against: its agent does not compact on its own, which the
//! component says once.

use crate::compactor::Compactor;
use crate::{CompactionError, CompactionStrategy, KnowledgeRestorer};
use async_trait::async_trait;
use caelumora_agents::activity::{ActivityError, ActivityProvider};
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::situation::Situation;
use caelumora_agents::turn::Turn;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_runtime::command::{CommandError, CommandResult, ProvidesCommands};
use caelumora_runtime::component::{
    config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry,
    ComponentResult, Params,
};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

/// The kind of the component.
pub const COMPACTION_KIND: &str = "compaction";
/// The name of its command, `/compact`.
pub const COMPACT_COMMAND_NAME: &str = "compact";

/// What the agent is told it does as it compacts on its own.
const ACTIVITY: &str = "compacting its context";

/// How full the context may be before the agent compacts it, as a share of its model's context
/// size, unless configured.
const DEFAULT_THRESHOLD: f64 = 0.8;

/// How much of the context a compaction keeps as it stands, as a share of its model's context
/// size, unless configured.
const DEFAULT_KEEP: f64 = 0.25;

/// Compaction of an agent's context: on its own, as an activity, once the context fills
/// `threshold`, and by `/compact`.
#[derive(Debug)]
pub struct CompactionComponent {
    /// What carries out a compaction, with the agent's strategy.
    compactor: Compactor,
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
    #[serde(default = "default_keep")]
    keep: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

fn default_keep() -> f64 {
    DEFAULT_KEEP
}

impl Params for CompactionConfig {
    /// # Errors
    /// `InvalidConfig` if `keep` and `threshold` are no shares of the context size with `keep`
    /// below `threshold`: kept as much as it compacts at, a compaction would free too little.
    fn check(&self) -> ComponentResult<()> {
        let (threshold, keep) = (self.threshold, self.keep);
        if !(0.0 <= keep && keep < threshold && threshold <= 1.0) {
            return Err(ComponentError::InvalidConfig(format!("'keep' and 'threshold' are shares of the context size, 'keep' at least 0 and less than 'threshold', 'threshold' at most 1, not {keep} and {threshold}")));
        }
        Ok(())
    }
}

impl CompactionComponent {
    fn new(compactor: Compactor, threshold: f64) -> Self {
        Self { compactor, threshold, told_no_size: AtomicBool::new(false) }
    }

    /// Compacts the context of `turn`'s agent: what it did, in words for its channel, or `None`
    /// if there was nothing to compact.
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<String>, CompactionError> {
        Ok(self
            .compactor
            .compact(turn)
            .await?
            .map(|compaction| compaction.describe(turn.agent_name())))
    }
}

/// What the agent `agent_name`'s channel is told when there was nothing to compact.
fn nothing_to_compact(agent_name: &str) -> String {
    format!(
        "{agent_name} found nothing to compact: it keeps the newest part of its context as it is"
    )
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
    async fn execute(
        &self,
        name: &str,
        args: &str,
        turn: &mut Turn<'_>,
    ) -> CommandResult<CommandOutput> {
        if name != COMPACT_COMMAND_NAME {
            return Err(CommandError::Unknown(name.to_string()));
        }
        let invalid = |reason: &str| CommandError::InvalidArguments {
            command: name.to_string(),
            reason: reason.to_string(),
        };
        if !args.trim().is_empty() {
            return Err(invalid("it takes no arguments"));
        }
        let compacted = self
            .compact(turn)
            .await
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
        let compacted = self.compact(turn).await.map_err(|e| {
            ActivityError::Failed(format!(
                "{} could not compact its context: {e}",
                turn.agent_name()
            ))
        })?;
        Ok(compacted.unwrap_or_else(|| nothing_to_compact(turn.agent_name())))
    }
}

/// Registers the kind `compaction` with `registry`.
pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(COMPACTION_KIND)
            .params::<CompactionConfig>()
            .provides::<dyn AgentCommandProvider>()
            .provides::<dyn ActivityProvider>()
            .needs::<dyn CompactionStrategy>()
            .needs::<dyn KnowledgeRestorer>(),
        |descriptor, context| async move {
            let CompactionConfig { threshold, keep } =
                config_of_params::<CompactionConfig>(&descriptor.params)?;
            let strategy = context.one::<dyn CompactionStrategy>().await?.facet;
            // What the agent's components give again, each of its kind.
            let restorers = context
                .all::<dyn KnowledgeRestorer>()
                .await?
                .into_iter()
                .map(|restorer| restorer.facet)
                .collect();
            let compactor = Compactor::new(strategy, restorers, keep);
            let component = Arc::new(CompactionComponent::new(compactor, threshold));
            let mut facets = ComponentFacets::empty();
            facets
                .insert::<dyn AgentCommandProvider>(Arc::<CompactionComponent>::clone(&component));
            facets.insert::<dyn ActivityProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{two_calls, Scripted};
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::agent::AgentId;
    use caelumora_model::command::CommandVisibility;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use serde_json::Value;

    /// The component with `strategy`, keeping a tenth of the context.
    fn component(strategy: &Arc<Scripted>) -> CompactionComponent {
        let compactor = Compactor::new(Arc::<Scripted>::clone(strategy), Vec::new(), 0.1);
        CompactionComponent::new(compactor, DEFAULT_THRESHOLD)
    }

    /// What the turn of an agent that heard `u1` did before it compacts.
    type Before = fn(&mut Turn<'_>, AgentId);

    /// Nothing done in the turn: nothing to compact, beside `two_calls`.
    fn nothing(_: &mut Turn<'_>, _: AgentId) {}

    /// What `/compact` with `args` answers, after `before`, with `strategy`, and how often the
    /// strategy was asked.
    async fn compact(
        strategy: Scripted,
        before: Before,
        args: &str,
    ) -> (CommandResult<CommandOutput>, usize) {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard("u1")]);
        let strategy = Arc::new(strategy);
        let component = component(&strategy);
        let mut turn = workspace_turn(&world, &mut state);
        before(&mut turn, world.agent);
        let result = component.execute(COMPACT_COMMAND_NAME, args, &mut turn).await;
        (result, strategy.times_asked())
    }

    /// What compacting on its own tells, after `before`, with `strategy`.
    async fn carry_out(strategy: Scripted, before: Before) -> Result<String, ActivityError> {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard("u1")]);
        let component = component(&Arc::new(strategy));
        let mut turn = workspace_turn(&world, &mut state);
        before(&mut turn, world.agent);
        component.carry_out(&mut turn).await
    }

    fn heard(text: &str) -> Message {
        Message { content: Some(text.to_string()), ..Message::new(Classification::PUBLIC) }
    }

    #[tokio::test]
    async fn compact_tells_the_channel_what_it_did() {
        let (answer, _) = compact(Scripted::default(), two_calls, "").await;
        let answer = answer.expect("compacted");
        assert_eq!(
            answer.text,
            "Jonathan compacted its context: 3 messages replaced, 2 kept as they were."
        );
        assert_eq!(answer.visibility, CommandVisibility::Channel, "everyone should know");
    }

    #[tokio::test]
    async fn compact_refuses_what_it_cannot_shorten_and_takes_no_arguments() {
        let (refused, asked) = compact(Scripted::default(), nothing, "").await;
        assert!(
            matches!(refused, Err(CommandError::InvalidArguments { ref reason, .. }) if reason.contains("nothing to compact")),
            "{refused:?}"
        );
        assert_eq!(asked, 0, "its strategy was not asked");
        let (refused, asked) = compact(Scripted::default(), two_calls, "hard").await;
        assert!(
            matches!(refused, Err(CommandError::InvalidArguments { ref reason, .. }) if reason.contains("no arguments")),
            "{refused:?}"
        );
        assert_eq!(asked, 0, "its strategy was not asked");
    }

    #[tokio::test]
    async fn compact_fails_with_the_reason_its_strategy_failed() {
        let failing = Scripted::failing(|| CompactionError::ModelUnreachable);
        let (failed, _) = compact(failing, two_calls, "").await;
        assert!(
            matches!(failed, Err(CommandError::Failed { ref reason, .. }) if reason == "its model could not be asked"),
            "{failed:?}"
        );
    }

    #[tokio::test]
    async fn compacting_on_its_own_tells_what_it_did_and_why_it_failed() {
        let compacted = carry_out(Scripted::default(), two_calls).await;
        assert_eq!(
            compacted.ok().as_deref(),
            Some("Jonathan compacted its context: 3 messages replaced, 2 kept as they were.")
        );
        let nothing = carry_out(Scripted::default(), nothing).await;
        assert!(
            matches!(nothing, Ok(ref said) if said.contains("found nothing to compact")),
            "{nothing:?}"
        );
        let failed =
            carry_out(Scripted::failing(|| CompactionError::ModelUnreachable), two_calls).await;
        assert!(
            matches!(failed, Err(ActivityError::Failed(ref reason)) if reason == "Jonathan could not compact its context: its model could not be asked"),
            "{failed:?}"
        );
    }

    #[tokio::test]
    async fn it_is_due_once_a_call_measured_the_context_that_full() {
        let world = session_with_workspace().await;
        let component = component(&Arc::new(Scripted::default()));
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard("hello")]);
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
        turn.record_said(call(700, Some(1000)));
        assert!(!component.is_due(&turn.situation()), "701 of 1000");
        turn.record_said(call(800, Some(1000)));
        assert!(component.is_due(&turn.situation()), "801 of 1000");
        turn.record_said(call(900, None));
        assert!(!component.is_due(&turn.situation()), "nothing to measure against without a size");
    }

    #[test]
    fn what_it_keeps_and_when_it_compacts_are_shares_of_the_context_size() {
        let config =
            |params| config_of_params::<CompactionConfig>(&params).map(|c| (c.threshold, c.keep));
        assert_eq!(config(Value::Null).ok(), Some((DEFAULT_THRESHOLD, DEFAULT_KEEP)));
        assert_eq!(
            config(serde_json::json!({ "threshold": 0.5, "keep": 0.2 })).ok(),
            Some((0.5, 0.2))
        );
        assert!(
            config(serde_json::json!({ "threshold": 0.5, "keep": 0.5 })).is_err(),
            "kept as much as it compacts at, it would free too little"
        );
        assert!(config(serde_json::json!({ "threshold": 1.5 })).is_err(), "at most all of it");
        assert!(config(serde_json::json!({ "keep": -0.1 })).is_err(), "nothing less than nothing");
        assert!(
            config(serde_json::json!({ "keep_last": 10 })).is_err(),
            "what it does not take is refused"
        );
    }
}
