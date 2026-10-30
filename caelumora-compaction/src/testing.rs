//! What the tests of the crate share: the texts, a strategy that answers as it is told, and a turn
//! whose context can be compacted.

use crate::{CompactionError, CompactionStrategy, Replacement, NAMESPACE, PROMPTS};
use async_trait::async_trait;
use caelumora_agents::llm::unlabelled_message;
use caelumora_agents::prompt::{NAMESPACE as AGENTS, PROMPTS as AGENTS_PROMPTS};
use caelumora_agents::turn::Turn;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptTexts, CORE, CORE_PROMPTS};
use std::sync::{Arc, Mutex};

/// The texts of the core, of the agents and of compaction, as the runtime and the plugins add them,
/// as a model of the default variant is given them - for what a test makes itself.
pub(crate) fn prompts() -> PromptTexts {
    let mut catalogue = PromptCatalogue::new();
    for (namespace, texts) in [(CORE, CORE_PROMPTS), (AGENTS, AGENTS_PROMPTS), (NAMESPACE, PROMPTS)]
    {
        catalogue
            .add(namespace, texts)
            .unwrap_or_else(|e| panic!("the texts of '{namespace}' cannot be read: {e}"));
    }
    PromptTexts::new(Arc::new(catalogue), None)
}

/// The text the strategy puts in the place of a part, unless it fails.
pub(crate) const SUMMARY: &str = "They want release notes.";

/// A strategy that replaces every part by `SUMMARY`, its call measured as 40 tokens of prompt and 8
/// of answer - or fails with what `fails` returns -, keeping what it was asked: the part, and what
/// was given again.
#[derive(Debug, Default)]
pub(crate) struct Scripted {
    fails: Option<fn() -> CompactionError>,
    pub(crate) asked: Mutex<Vec<(Vec<Message>, Vec<String>)>>,
}

impl Scripted {
    /// A strategy that fails with what `fails` returns.
    pub(crate) fn failing(fails: fn() -> CompactionError) -> Self {
        Self { fails: Some(fails), ..Self::default() }
    }

    /// How often it was asked.
    pub(crate) fn times_asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

#[async_trait]
impl CompactionStrategy for Scripted {
    async fn replace(
        &self,
        _: &Turn<'_>,
        part: Vec<Message>,
        given: &[String],
    ) -> Result<Replacement, CompactionError> {
        self.asked.lock().unwrap().push((part, given.to_vec()));
        if let Some(fails) = self.fails {
            return Err(fails());
        }
        let call = Message {
            role: MessageRole::Assistant,
            content: Some(SUMMARY.to_string()),
            prompt_tokens: Some(40),
            completion_tokens: Some(8),
            ..unlabelled_message()
        };
        Ok(Replacement { text: SUMMARY.to_string(), call })
    }
}

/// Records in `turn` two calls of `agent`'s model, each with its result, long enough that a
/// compaction keeping a tenth of the context replaces the first.
pub(crate) fn two_calls(turn: &mut Turn<'_>, agent: AgentId) {
    for (content, role) in [
        ("a", MessageRole::Assistant),
        ("r", MessageRole::Tool),
        ("b", MessageRole::Assistant),
        ("s", MessageRole::Tool),
    ] {
        turn.record_said(Message {
            sender_id: ActorId::Agent(agent),
            role,
            content: Some(content.repeat(40)),
            ..Message::new(Classification::PUBLIC)
        });
    }
}
