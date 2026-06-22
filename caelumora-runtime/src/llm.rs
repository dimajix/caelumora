use crate::message::MessageEvent;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::Message;
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use std::fmt::Debug;

#[async_trait]
pub trait LLMClient: Send + Sync + Debug {
    /// It takes the conversation context and returns a stream of string chunks.
    async fn chat_completion(&self, context: &[Message], tools: &[Tool]) -> BoxStream<'static, Result<MessageEvent>>;
}