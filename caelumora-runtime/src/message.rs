use crate::security::SecurityContext;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message};
use futures::stream::BoxStream;
use std::fmt::Debug;

#[async_trait]
pub trait MessageFilter: Send + Sync {
    async fn process(&self, message: Message) -> anyhow::Result<Message>;
}


#[derive(Debug, Clone)]
pub enum MessageEvent {
    /// A partial piece of a message (e.g., a single sentence or a chunk of tokens)
    Partial(Message),
    /// A complete message that is now ready to be routed/processed
    Complete(Message),
    /// A signal that this agent has finished its current turn
    EndOfTurn(Vec<Message>),
}

#[async_trait]
pub trait MessageProcessor: Send + Sync + Debug {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;
    async fn process_messages(&mut self, messages: Vec<Message>, security_context: &SecurityContext) -> BoxStream<'static, anyhow::Result<MessageEvent>>;
}
