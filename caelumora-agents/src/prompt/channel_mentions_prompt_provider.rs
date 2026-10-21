//! The `channel_mentions_prompt_provider` component (issue 442): reminds an
//! agent, before the turn's input, when to write an @handle and when a
//! name - an @handle only for an answer or an action needed from that
//! participant now, never to greet, thank, praise, agree with or refer to
//! someone. The roster says as much (`channel_participants_prompt_provider`),
//! but in the system prompt, read before everything said since: there it
//! changed little, and agents kept mentioning each other in every greeting,
//! each mention asking the next one to answer. Read last, it does what the
//! roster alone does not (`work/quiet_agents/MENTIONS.md`, findings 1-3).
//!
//! It is given only where another agent is in the channel: a mention of a
//! person asks no agent, and an agent alone has nobody to ask. The same in
//! every turn, so it is not stored with the turn's context; it goes before
//! the turn's input by default (parameter `position`).

use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::channel_participants_prompt_provider::participants;
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, texts_of, writing_for_the_model};
use crate::situation::Situation;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::prompt_catalogue::PromptTexts;
use std::sync::Arc;

pub const CHANNEL_MENTIONS_PROMPT_PROVIDER_KIND: &str = "channel_mentions_prompt_provider";

/// Its text in the prompt catalogue, as the experiments worded it
/// (`MENTIONS.md`, `test_names_turn`).
const TEXT: &str = "agents.channel_mentions";

#[derive(Debug)]
pub struct ChannelMentionsPromptProvider {
    pub position: PromptPosition,
    pub prompts: PromptTexts,
}

#[async_trait]
impl PromptProvider for ChannelMentionsPromptProvider {
    fn position(&self) -> PromptPosition {
        self.position
    }

    /// The rule, where another agent is in the channel; nothing otherwise.
    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let others = participants(situation).await?.map(|(_, others)| others).unwrap_or_default();
        let text = if others.iter().any(|participant| !participant.is_person()) { self.prompts.render(TEXT, ())? } else { String::new() };
        // A rule of the runtime's, no secret of anything.
        Ok(Prompt::classified(text, Classification::PUBLIC))
    }
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(CHANNEL_MENTIONS_PROMPT_PROVIDER_KIND).provides::<dyn PromptProvider>()),
        |descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            let position = PromptPosition::of_params_or(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(ChannelMentionsPromptProvider { position, prompts }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use caelumora_model::agent::AgentChannelState;

    #[tokio::test]
    async fn an_agent_is_reminded_only_where_another_agent_could_be_asked() {
        let world = testing::world().await;
        let personality = world.personality("plain", None, &[]).await;
        let jonathan = world.agent("Jonathan", world.alice, &personality).await;
        let sassy = world.agent("Sassy", world.alice, &personality).await;
        let provider = ChannelMentionsPromptProvider { position: PromptPosition::BeforeTurnInput, prompts: testing::prompts() };

        let with_sassy = world.shared_session(&[jonathan, sassy], &[]).await;
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&testing::turn(&with_sassy, jonathan, world.alice, &mut state).situation()).await.unwrap();
        assert_eq!(prompt.text, "[Before you write]\nOnly use an @handle if you require an answer or an action from that participant now. \
                                 Do NOT use one to greet, thank, praise, agree with or refer to someone - write their name.\n");
        assert_eq!(prompt.classification, Some(Classification::PUBLIC));

        let among_people = world.shared_session(&[jonathan], &[sassy]).await;
        let mut state = AgentChannelState::default();
        let prompt = provider.get_prompt(&testing::turn(&among_people, jonathan, world.alice, &mut state).situation()).await.unwrap();
        assert_eq!(prompt.text, "", "a mention of a person asks no agent; an agent waiting outside the channel is not in it");
    }
}
