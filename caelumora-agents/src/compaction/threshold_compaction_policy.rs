//! The `threshold_compaction_policy` component: an agent compacts its
//! context on its own once it fills `threshold` of its model's context size
//! (issue 284). The default compaction policy of an agent.
//!
//! How full the context is, is measured by the calls made on it and
//! estimated beyond them (`Situation::token_positions`); only once a call
//! made on the active branch measured it, not right after a compaction,
//! whose branch no call measured yet. A model that reports no context size
//! gives nothing to measure against: its agent does not compact on its own,
//! which the policy says once.

use crate::situation::Situation;
use crate::compaction::CompactionPolicy;
use caelumora_runtime::component::{config_of_params, ComponentError, ComponentFacets, ComponentKind, ComponentRegistry};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

pub const THRESHOLD_COMPACTION_POLICY_KIND: &str = "threshold_compaction_policy";

/// How full the context may be before the agent compacts it, as a share of
/// its model's context size, unless configured.
const DEFAULT_THRESHOLD: f64 = 0.8;

#[derive(Debug)]
pub struct ThresholdCompactionPolicy {
    /// How full the context may be, as a share of the context size.
    threshold: f64,
    /// Whether it said that the agent's model reports no context size.
    told_no_size: AtomicBool,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdCompactionPolicyConfig {
    #[serde(default = "default_threshold")]
    threshold: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

impl ThresholdCompactionPolicy {
    fn new(threshold: f64) -> Self {
        Self { threshold, told_no_size: AtomicBool::new(false) }
    }
}

impl CompactionPolicy for ThresholdCompactionPolicy {
    fn is_due(&self, situation: &Situation<'_>) -> bool {
        let Some((positions, true)) = situation.token_positions() else { return false };
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
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(THRESHOLD_COMPACTION_POLICY_KIND).provides::<dyn CompactionPolicy>(),
        |descriptor, _context| {
            let ThresholdCompactionPolicyConfig { threshold } = config_of_params(&descriptor.params)?;
            if !(threshold > 0.0 && threshold <= 1.0) {
                return Err(ComponentError::InvalidConfig(format!("'threshold' is a share of the context size, more than 0 and at most 1, not {threshold}")));
            }
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CompactionPolicy>(Arc::new(ThresholdCompactionPolicy::new(threshold)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::workspace_turn;
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use serde_json::Value;

    #[tokio::test]
    async fn it_is_due_once_a_call_measured_the_context_that_full() {
        let world = session_with_workspace().await;
        let policy = ThresholdCompactionPolicy::new(0.8);
        let heard = Message { content: Some("hello".to_string()), ..Message::new(Classification::PUBLIC) };
        let mut state = AgentChannelState::default();
        state.perceive(vec![heard.clone()], vec![heard]);
        let mut turn = workspace_turn(&world, &mut state);
        assert!(!policy.is_due(&turn.situation()), "nothing measured");

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
        assert!(!policy.is_due(&turn.situation()), "701 of 1000");
        turn.push_message(call(800, Some(1000)));
        assert!(policy.is_due(&turn.situation()), "801 of 1000");
        turn.push_message(call(900, None));
        assert!(!policy.is_due(&turn.situation()), "nothing to measure against without a size");
    }

    #[test]
    fn its_threshold_is_a_share_of_the_context_size() {
        let config = |params| config_of_params::<ThresholdCompactionPolicyConfig>(&params).map(|c| c.threshold);
        assert_eq!(config(Value::Null).ok(), Some(DEFAULT_THRESHOLD));
        assert_eq!(config(serde_json::json!({ "threshold": 0.5 })).ok(), Some(0.5));
        assert!(config(serde_json::json!({ "keep": 0.5 })).is_err(), "what it keeps is its strategy's");
    }
}
