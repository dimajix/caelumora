//! The `summary_compaction` component: the older part of an agent's context summarised by its own
//! model, which stands in its place, with what of what was given there the agent still needs. The
//! default compaction strategy of an agent.
//!
//! Its model is given the part as the agent's model was, and asked last to summarise it for going
//! on from the summary alone, leaving out the text of what can be given again after it. Where
//! something can, it is asked next, after its summary, which of it the work still open needs
//! (issue 413): a skill of a task that is over is not given again. An answer that names nothing
//! of it and does not say "none" cannot be read, and everything is given again, as is where the
//! model cannot be asked.

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

/// What an answer says where the agent needs none of what it is asked about.
const NONE: &str = "none";

#[derive(Debug)]
pub struct SummaryCompaction {
    /// The agent's own model, which writes the summary.
    llm_client: Arc<dyn LLMClient>,
    /// What its model is asked, and how the summary is titled (issue 57).
    prompts: PromptTexts,
}

impl SummaryCompaction {
    /// What the model is asked to summarise for - going on from the summary alone -, and to leave
    /// out `given`, which can be given again.
    fn instruction(&self, given: &[String]) -> Result<String, PromptError> {
        self.prompts.render(&format!("{NAMESPACE}.instruction"), context! { given })
    }

    /// What the model is asked after its summary: which of `given` it still needs.
    fn question(&self, given: &[String]) -> Result<String, PromptError> {
        self.prompts.render(&format!("{NAMESPACE}.still_needed"), context! { given, none => NONE })
    }

    /// The summary `text` as the transcript holds it, under its title; nothing if there is none.
    fn titled(&self, text: &str) -> Result<String, PromptError> {
        if text.is_empty() {
            return Ok(String::new());
        }
        self.prompts.render(&format!("{NAMESPACE}.summary"), context! { text })
    }

    /// What the agent's model answers to `context`: the texts of its messages, trimmed, between
    /// blank lines - empty if there are none -, and the message it answered with last; `None` if it
    /// answered no message.
    ///
    /// # Errors
    /// `ModelUnreachable` if it cannot be asked.
    async fn ask(
        &self,
        turn: &Turn<'_>,
        context: &[Message],
    ) -> Result<Option<(String, Message)>, CompactionError> {
        let mut stream = self.llm_client.chat_completion(context, &[]).await;
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
        Ok(answered.pop().map(|last| (text, last)))
    }
}

#[async_trait]
impl CompactionStrategy for SummaryCompaction {
    /// The summary of `part`, written by the agent's model, under its title, and what of `given`
    /// it still needs, as it answers after its summary; the message it answered with, for the
    /// tokens of its calls.
    async fn replace(
        &self,
        turn: &Turn<'_>,
        mut part: Vec<Message>,
        given: &[String],
    ) -> Result<Replacement, CompactionError> {
        let classification = turn.situation().classification();
        let in_its_name = |role, text: String| Message {
            role,
            content: Some(text),
            sender_id: ActorId::Agent(turn.situation().agent_id()),
            sender_name: turn.agent_name().to_string(),
            ..Message::new(classification.clone())
        };
        part.push(in_its_name(MessageRole::User, self.instruction(given)?));
        let Some((summary, call)) =
            self.ask(turn, &part).await?.filter(|(text, _)| !text.is_empty())
        else {
            return Err(CompactionError::NoSummary);
        };
        let text = self.titled(&summary)?;
        if given.is_empty() {
            return Ok(Replacement { text, call, still_needed: Vec::new() });
        }

        part.push(in_its_name(MessageRole::Assistant, summary));
        part.push(in_its_name(MessageRole::User, self.question(given)?));
        // Its summary is written: an answer that cannot be had fails nothing.
        let (named, call) = match self.ask(turn, &part).await {
            Ok(Some((answer, then))) => (named_in(&answer, given), summed(call, &then)),
            Ok(None) | Err(_) => (None, call),
        };
        let still_needed = named.unwrap_or_else(|| {
            tracing::warn!(
                "Agent {}: what it still needs after a compaction cannot be read from its \
                model's answer; all of it is given again",
                turn.situation().agent_id()
            );
            given.to_vec()
        });
        Ok(Replacement { text, call, still_needed })
    }
}

/// Of `given`, what `answer` names, a line a name, as written but for case - a list's mark, quotes
/// and a full stop around it aside. `None` if it names none of them and does not say `NONE`: it
/// cannot be read.
fn named_in(answer: &str, given: &[String]) -> Option<Vec<String>> {
    let lines: Vec<&str> = answer
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '•'])
                .trim()
                .trim_matches(['`', '"', '\'', '.'])
                .trim()
        })
        .collect();
    let named: Vec<String> = given
        .iter()
        .filter(|name| lines.iter().any(|line| line.eq_ignore_ascii_case(name)))
        .cloned()
        .collect();
    let none = lines.iter().any(|line| line.eq_ignore_ascii_case(NONE));
    (!named.is_empty() || none).then_some(named)
}

/// `call` with the tokens of the call `then` answered added, and the context size of `then`: the
/// question after the summary has no message of its own to carry them (issue 514).
fn summed(call: Message, then: &Message) -> Message {
    let add = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    Message {
        prompt_tokens: add(call.prompt_tokens, then.prompt_tokens),
        completion_tokens: add(call.completion_tokens, then.completion_tokens),
        context_size: then.context_size.or(call.context_size),
        ..call
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
    use caelumora_agents::llm::{unlabelled_message, LLMError, LLMResult};
    use caelumora_agents::testing::workspace_turn;
    use caelumora_model::agent::AgentChannelState;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::tool::Tool;
    use caelumora_runtime::workspace::testing::session_with_workspace;
    use futures::stream;
    use futures::stream::BoxStream;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A model that gives its answers one a call - `None` as a model that cannot be reached -,
    /// each call measured as 40 tokens of prompt and 8 of answer, keeping what it was given.
    #[derive(Debug)]
    struct Summariser {
        answers: Mutex<VecDeque<Option<&'static str>>>,
        asked: Mutex<Vec<Vec<Message>>>,
    }

    impl Summariser {
        fn answering(answers: impl IntoIterator<Item = Option<&'static str>>) -> Arc<Self> {
            let answers = Mutex::new(answers.into_iter().collect());
            Arc::new(Self { answers, asked: Mutex::new(Vec::new()) })
        }

        /// The texts of what it was given in its call `call`.
        fn asked_in(&self, call: usize) -> Vec<String> {
            self.asked.lock().unwrap()[call].iter().filter_map(|m| m.content.clone()).collect()
        }

        fn times_asked(&self) -> usize {
            self.asked.lock().unwrap().len()
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
            let answer = self.answers.lock().unwrap().pop_front().expect("an answer left");
            let event = match answer {
                Some(text) => Ok(MessageEvent::Complete(Message {
                    role: MessageRole::Assistant,
                    content: Some(text.to_string()),
                    prompt_tokens: Some(40),
                    completion_tokens: Some(8),
                    ..unlabelled_message()
                })),
                None => Err(LLMError::Connection {
                    endpoint: "http://model".to_string(),
                    reason: "gone".to_string(),
                }),
            };
            stream::iter(vec![event]).boxed()
        }
    }

    fn strategy(model: &Arc<Summariser>) -> SummaryCompaction {
        SummaryCompaction { llm_client: Arc::<Summariser>::clone(model), prompts: prompts() }
    }

    fn heard(text: &str) -> Message {
        Message { content: Some(text.to_string()), ..Message::new(Classification::PUBLIC) }
    }

    /// What can be given again in the tests: two skills.
    fn given() -> Vec<String> {
        vec!["skill release-notes".to_string(), "skill quiz-round".to_string()]
    }

    /// What `model`, as the strategy's, answers to `part` with `given` that can be given again.
    async fn replaced(
        model: &Arc<Summariser>,
        given: &[String],
    ) -> Result<Replacement, CompactionError> {
        let world = session_with_workspace().await;
        let mut state = AgentChannelState::default();
        let turn = workspace_turn(&world, &mut state);
        strategy(model).replace(&turn, vec![heard("u1")], given).await
    }

    #[tokio::test]
    async fn its_model_summarises_the_part_leaving_out_what_can_be_given_again() {
        let model = Summariser::answering([Some("They want release notes."), Some("none")]);
        let given = given();

        let replacement = replaced(&model, &given).await.expect("summarised");

        assert_eq!(
            replacement.text, "[Summary of the conversation so far]\nThey want release notes.\n",
            "under its title"
        );
        let instruction = strategy(&model).instruction(&given).unwrap();
        assert_eq!(
            model.asked_in(0),
            ["u1".to_string(), instruction.clone()],
            "the part, then the instruction"
        );
        assert!(
            instruction.ends_with(
                "Leave out the text of skill release-notes, skill quiz-round: what you still need \
                of it is given to you again."
            ),
            "and to leave out what can be given again: {instruction}"
        );
        assert!(
            !strategy(&model).instruction(&[]).unwrap().contains("Leave out"),
            "nothing to leave out where nothing can be given again"
        );
    }

    /// Issue 413: asked after its summary, the model names what the work still open needs; what it
    /// does not name is not still needed.
    #[tokio::test]
    async fn its_model_is_asked_after_its_summary_what_it_still_needs() {
        let model = Summariser::answering([
            Some("They want release notes."),
            Some("I still need:\n- `skill Release-Notes`."),
        ]);
        let given = given();

        let replacement = replaced(&model, &given).await.expect("summarised");

        assert_eq!(replacement.still_needed, ["skill release-notes"], "what it named, alone");
        let question = strategy(&model).question(&given).unwrap();
        assert_eq!(
            model.asked_in(1)[1..],
            [
                strategy(&model).instruction(&given).unwrap(),
                "They want release notes.".to_string(),
                question.clone()
            ],
            "after its summary, in the same conversation"
        );
        assert!(
            question.contains("\n- skill release-notes\n- skill quiz-round\n"),
            "a line a name: {question}"
        );
        assert_eq!(
            (replacement.call.prompt_tokens, replacement.call.completion_tokens),
            (Some(80), Some(16)),
            "the tokens of both calls"
        );
    }

    #[tokio::test]
    async fn none_is_still_needed_where_its_model_says_none() {
        let model = Summariser::answering([Some("They met."), Some("None.")]);
        let replacement = replaced(&model, &given()).await.expect("summarised");
        assert!(replacement.still_needed.is_empty(), "{:?}", replacement.still_needed);
    }

    /// Where its answer cannot be read, or the model cannot be asked after its summary, all of
    /// it is still needed: given again, as before issue 413.
    #[tokio::test]
    async fn all_is_still_needed_where_its_answer_cannot_be_had() {
        for answer in [Some("I am not sure."), Some(" "), None] {
            let model = Summariser::answering([Some("They met."), answer]);
            let replacement = replaced(&model, &given()).await.expect("summarised all the same");
            assert_eq!(replacement.still_needed, given(), "{answer:?}");
            let tokens = if answer.is_none() { 40 } else { 80 };
            assert_eq!(replacement.call.prompt_tokens, Some(tokens), "the calls that answered");
        }
    }

    #[tokio::test]
    async fn it_asks_nothing_more_where_nothing_can_be_given_again() {
        let model = Summariser::answering([Some("They met.")]);
        let replacement = replaced(&model, &[]).await.expect("summarised");
        assert!(replacement.still_needed.is_empty());
        assert_eq!(model.times_asked(), 1, "the summary alone");
    }

    #[tokio::test]
    async fn an_empty_answer_is_no_summary() {
        let failed = replaced(&Summariser::answering([Some("  ")]), &given()).await;
        assert!(matches!(failed, Err(CompactionError::NoSummary)), "{failed:?}");
    }

    #[tokio::test]
    async fn a_model_that_cannot_be_asked_for_the_summary_fails_the_compaction() {
        let failed = replaced(&Summariser::answering([None]), &given()).await;
        assert!(matches!(failed, Err(CompactionError::ModelUnreachable)), "{failed:?}");
    }
}
