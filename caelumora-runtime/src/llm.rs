use std::fmt::Debug;
use async_trait::async_trait;
use anyhow::Result;
use futures::stream::BoxStream;
use caelumora_model::message::{Message, MessageEvent};


#[async_trait]
pub trait LLMClient: Send + Sync + Debug {
    /// It takes the conversation context and returns a stream of string chunks.
    async fn chat_completion(&self, context: &[Message]) -> BoxStream<'static, Result<MessageEvent>>;
}