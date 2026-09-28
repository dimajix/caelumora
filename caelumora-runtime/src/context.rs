//! The facets that build what an agent's model is given: a
//! `ContextProvider` assembles it, `PromptProvider`s contribute prompt parts
//! (issue 47). Their implementations are components of their own, e.g.
//! `state_context_provider` and `static_system_prompt_provider`.

use crate::agent_turn::Turn;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::KnowledgeRef;
use serde::Deserialize;

/// Where a prompt part goes in the context (issue 47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptPosition {
    /// Before everything, as the system prompt. Many models put it first
    /// and cache what follows it, so it should rarely change.
    #[default]
    Start,
    /// Transiently before the newest message of the turn, never stored in
    /// the context - for what changes often, such as notes. Given as a user
    /// message: some models merge every system message into the system
    /// prompt and move it to the front, where it would invalidate the cache.
    BeforeTurnInput,
}

/// What a [`PromptProvider`] contributes: the text of its part, and the
/// knowledge from other places it contains, recorded with the turn's
/// messages (issue 17).
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub text: String,
    pub knowledge: Vec<KnowledgeRef>,
}

impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self { text, knowledge: Vec::new() }
    }
}

/// Contributes a prompt part: the system prompt, or other text the model is
/// given besides the conversation. `StateContextProvider` composes the parts
/// of the providers it depends on, by position.
#[async_trait]
pub trait PromptProvider: Send + Sync + std::fmt::Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    /// The part for the turn `context` is about to start; empty text for
    /// none.
    async fn get_prompt(&self, turn: &Turn) -> Result<Prompt>;
}

#[async_trait]
pub trait ContextProvider: Send + Sync + std::fmt::Debug {
    async fn current_context(&self, turn: &mut Turn) -> Result<()>;
}
