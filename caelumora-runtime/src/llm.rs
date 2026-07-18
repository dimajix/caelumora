use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

#[derive(Debug, thiserror::Error)]
pub enum LLMError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
}

pub type LLMResult<T> = Result<T, LLMError>;

#[async_trait]
pub trait LLMClient: Send + Sync + Debug {
    /// It takes the conversation context and returns a stream of string chunks.
    async fn chat_completion(&self, context: &[Message], tools: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>>;
}