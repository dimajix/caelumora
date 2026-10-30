//! The `summary_compaction` component: the older part of an agent's context summarised by its own
//! model, which stands in its place. The default compaction strategy of an agent.
//!
//! Its model is given the part as the agent's model was, and asked last to summarise it for going
//! on from the summary alone, leaving out the text of what is given again after it.

use crate::NAMESPACE;
use crate::{CompactionError, CompactionStrategy, Replacement};
use async_trait::async_trait;
use caelumora_agents::llm::LLMClient;
use caelumora_agents::prompt::{texts_of, writing_for_the_model};
use caelumora_agents::turn::Turn;
use caelumora_model::actor::ActorId;
use caelumora_model::message::{Message, MessageEvent, MessageRole};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use futures::StreamExt;
use std::sync::Arc;

/// The kind of the strategy, the default one.
pub const SUMMARY_COMPACTION_KIND: &str = "summary_compaction";

#[derive(Debug)]
pub struct SummaryCompaction {
    /// The agent's own model, which writes the summary.
    llm_client: Arc<dyn LLMClient>,
    /// What its model is asked, and how the summary is titled (issue 57).
    prompts: PromptTexts,
}

impl SummaryCompaction {
    /// What the model is asked to summarise for - going on from the summary alone -, and to leave
    /// out `given`, which is given again.
    fn instruction(&self, given: &[String]) -> Result<String, PromptError> {
        self.prompts.render(&format!("{NAMESPACE}.instruction"), context! { given })
    }

    /// The summary `text` as the transcript holds it, under its title; nothing if there is none.
    fn titled(&self, text: &str) -> Result<String, PromptError> {
        if text.is_empty() {
            return Ok(String::new());
        }
        self.prompts.render(&format!("{NAMESPACE}.summary"), context! { text })
    }
}

#[async_trait]
impl CompactionStrategy for SummaryCompaction {
    /// The summary of `part`, written by the agent's model, under its title; the message its model
    /// answered with, for the tokens of the call.
    async fn replace(
        &self,
        turn: &Turn<'_>,
        mut part: Vec<Message>,
        given: &[String],
    ) -> Result<Replacement, CompactionError> {
        part.push(Message {
            role: MessageRole::User,
            content: Some(self.instruction(given)?),
            sender_id: ActorId::Agent(turn.situation().agent_id()),
            sender_name: turn.agent_name().to_string(),
            ..Message::new(turn.situation().classification())
        });
        let mut stream = self.llm_client.chat_completion(&part, &[]).await;
        let mut answered: Vec<Message> = Vec::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(MessageEvent::Complete(message)) => answered.push(message),
                Ok(MessageEvent::Partial(_) | MessageEvent::EndOfTurn(_)) => {}
                Err(e) => {
                    tracing::warn!(
                        "Agent {}: a compaction could not ask its model: {}",
                        turn.situation().agent_id(),
                        e
                    );
                    return Err(CompactionError::ModelUnreachable);
                }
            }
        }
        let text = answered
            .iter()
            .filter_map(|m| m.content.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        match answered.pop() {
            Some(call) if !text.is_empty() => Ok(Replacement { text: self.titled(&text)?, call }),
            _ => Err(CompactionError::NoSummary),
        }
    }
}

/// Registers the kind `summary_compaction` with `registry`.
pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(SUMMARY_COMPACTION_KIND)
                .provides::<dyn CompactionStrategy>()
                .needs::<dyn LLMClient>(),
        ),
        |_descriptor, context| async move {
            let prompts = texts_of(&context).await?;
            // The agent's own model writes the summary.
            let llm_client = context.one::<dyn LLMClient>().await?.facet;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn CompactionStrategy>(Arc::new(SummaryCompaction {
                llm_client,
                prompts,
            }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::prompts;
    use caelumora_agents::llm::{unlabelled_message, LLMResult};
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::tool::Tool;
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use futures::stream;
    use futures::stream::BoxStream;
    use std::sync::Mutex;

    /// A model that answers `answer` to everything, keeping what it was given.
    #[derive(Debug)]
    struct Summariser {
        answer: &'static str,
        asked: Mutex<Vec<Vec<Message>>>,
    }

    impl Summariser {
        fn answering(answer: &'static str) -> Arc<Self> {
            Arc::new(Self { answer, asked: Mutex::new(Vec::new()) })
        }
    }

    #[async_trait]
    impl LLMClient for Summariser {
        async fn chat_completion(
            &self,
            context: &[Message],
            _: &[Tool],
        ) -> BoxStream<'static, LLMResult<MessageEvent>> {
            self.asked.lock().unwrap().push(context.to_vec());
            let answer = Message {
                role: MessageRole::Assistant,
                content: Some(self.answer.to_string()),
                prompt_tokens: Some(40),
                completion_tokens: Some(8),
                ..unlabelled_message()
            };
            stream::iter(vec![Ok(MessageEvent::Complete(answer))]).boxed()
        }
    }

    fn strategy(model: &Arc<Summariser>) -> SummaryCompaction {
        SummaryCompaction { llm_client: Arc::<Summariser>::clone(model), prompts: prompts() }
    }

    fn heard(text: &str) -> Message {
        Message { content: Some(text.to_string()), ..Message::new(Classification::PUBLIC) }
    }

    #[tokio::test]
    async fn its_model_summarises_the_part_leaving_out_what_is_given_again() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let turn = workspace_turn(&world, &mut state);
        let model = Summariser::answering("They want release notes.");
        let strategy = strategy(&model);
        let given = ["skill release-notes".to_string()];

        let replacement =
            strategy.replace(&turn, vec![heard("u1")], &given).await.expect("summarised");

        assert_eq!(
            replacement.text, "[Summary of the conversation so far]\nThey want release notes.\n",
            "under its title"
        );
        assert_eq!(
            (replacement.call.prompt_tokens, replacement.call.completion_tokens),
            (Some(40), Some(8)),
            "the measures of its call"
        );
        let asked = model.asked.lock().unwrap().pop().expect("its model was asked");
        let texts: Vec<_> = asked.iter().filter_map(|m| m.content.clone()).collect();
        let instruction = strategy.instruction(&given).unwrap();
        assert_eq!(
            texts,
            ["u1".to_string(), instruction.clone()],
            "the part, then the instruction"
        );
        assert!(
            instruction
                .ends_with("Leave out the text of skill release-notes: it is given to you again."),
            "and to leave out what is given again: {instruction}"
        );
        assert!(
            !strategy.instruction(&[]).unwrap().contains("Leave out"),
            "nothing to leave out where nothing is given again"
        );
    }

    #[tokio::test]
    async fn an_empty_answer_is_no_summary() {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let turn = workspace_turn(&world, &mut state);
        let strategy = strategy(&Summariser::answering("  "));

        let failed = strategy.replace(&turn, vec![heard("u1")], &[]).await;

        assert!(matches!(failed, Err(CompactionError::NoSummary)), "{failed:?}");
    }
}
