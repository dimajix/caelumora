//! The facet `ActivityProvider` of `compaction` (issue 284): the agent compacts its context on its
//! own once its context fills `threshold` of its model's context size.
//!
//! How full the context is, is measured by the calls made on it and estimated beyond them
//! (`Situation::token_positions`); only once a call made on the active branch measured it, not
//! right after a compaction, whose branch no call measured yet. A model that reports no context
//! size gives nothing to measure against: its agent does not compact on its own, which the
//! component says once.

use super::{nothing_to_compact, CompactionComponent};
use async_trait::async_trait;
use caelumora_agents::activity::{ActivityError, ActivityProvider};
use caelumora_agents::situation::Situation;
use caelumora_agents::turn::Turn;
use std::sync::atomic::Ordering;
use tracing::info;

/// What the agent is told it does as it compacts on its own.
const ACTIVITY: &str = "compacting its context";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::{component, nothing, Before};
    use crate::testing::{heard, two_calls, Scripted};
    use crate::CompactionError;
    use caelumora_agents::testing::workspace_turn;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::AgentChannelState;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::message::{Message, MessageId, MessageRole};
    use caelumora_kernel::workspace::testing::session_with_workspace;
    use std::sync::Arc;

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
}
