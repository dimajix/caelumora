//! The `current_time_prompt_provider` component (issue 424): tells the agent
//! the current date and time, in its session's time zone. Without it, a
//! model takes the time of the last message for now - which is wrong for a
//! turn that comes later, after `/wakeup` or a long queue - and, without the
//! times of messages, knows no date but its training's
//! (`work/message_times/RESULTS.md`, findings 3-5).
//!
//! The time is read as the turn's context is built, in the time zone the
//! turn records (`Situation::time_zone`). It changes with every turn, so it
//! goes before the turn's input by default (parameter `position`): at the
//! start, in the system prompt, it would invalidate the prompt cache with
//! every turn. Where it stands made no difference to the model measured, nor
//! did it distract from what was asked (finding 6). A personality without
//! the component leaves its agents to the times of the messages.

use crate::prompt::{
    texts_of, writing_for_the_model, ContextError, Prompt, PromptPosition, PromptProvider,
};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::{context, PromptTexts};
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub const CURRENT_TIME_PROMPT_PROVIDER_KIND: &str = "current_time_prompt_provider";

/// Its text in the prompt catalogue.
const TEXT: &str = "agents.current_time";

#[derive(Debug)]
pub struct CurrentTimePromptProvider {
    pub position: PromptPosition,
    /// What it reads the time from: the system's clock, or a fixed one in
    /// tests.
    clock: fn() -> DateTime<Utc>,
    prompts: PromptTexts,
}

impl CurrentTimePromptProvider {
    /// The current time at `position`, read from the system's clock.
    pub fn new(position: PromptPosition, prompts: PromptTexts) -> Self {
        Self { position, clock: Utc::now, prompts }
    }
}

#[async_trait]
impl PromptProvider for CurrentTimePromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let zone = situation.time_zone();
        let text = self.prompts.render(
            TEXT,
            context! { time => zone.format_long((self.clock)()), zone => zone.to_string() },
        )?;
        // What time it is is no secret of anything.
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(CURRENT_TIME_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position =
                PromptPosition::of_params_or(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(CurrentTimePromptProvider::new(
                position, texts,
            )));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::testing::detached_actor_runtime;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::value::time_zone::TimeZone;
    use caelumora_model::work::Work;
    use caelumora_runtime::agent::state::AgentStateHandle;
    use chrono::TimeZone as _;

    fn sunday() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 12, 33, 0).unwrap()
    }

    /// What `provider` tells in work whose context records `zone`.
    async fn told(provider: &CurrentTimePromptProvider, zone: TimeZone) -> Prompt {
        let mut work = Work::default();
        work.context.frame.time_zone = zone;
        let (state, agent_state) = (AgentChannelState::default(), AgentStateHandle::detached());
        provider
            .get_prompt(&Situation::new(
                AgentId::new(),
                &work,
                &state,
                &agent_state,
                &detached_actor_runtime(),
            ))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_agent_is_told_the_time_in_the_time_zone_its_turn_records() {
        let provider = CurrentTimePromptProvider {
            position: PromptPosition::BeforeTurnInput,
            clock: sunday,
            prompts: testing::prompts(),
        };
        let prompt = told(&provider, TimeZone::new("Europe/Berlin").unwrap()).await;
        assert_eq!(
            prompt.text,
            "[Current date and time]\nIt is Sunday, 18 October 2026, 14:33 (Europe/Berlin).\n"
        );
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));
        assert!(
            told(&provider, TimeZone::UTC)
                .await
                .text
                .contains("It is Sunday, 18 October 2026, 12:33 (UTC)."),
            "UTC where the session names none"
        );
    }

    #[test]
    fn it_goes_before_the_turns_input_unless_configured_otherwise() {
        let position = |params| {
            PromptPosition::of_params_or(&params, PromptPosition::BeforeTurnInput).unwrap()
        };
        assert_eq!(
            position(serde_json::json!({})),
            PromptPosition::BeforeTurnInput,
            "it changes with every turn"
        );
        assert_eq!(
            position(serde_json::json!({ "position": "start" })),
            PromptPosition::Start,
            "as configured"
        );
        assert!(PromptPosition::of_params_or(
            &serde_json::json!({ "position": "end" }),
            PromptPosition::BeforeTurnInput
        )
        .is_err());
    }
}
