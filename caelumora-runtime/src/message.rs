use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::security::SecurityContext;
use futures::stream::BoxStream;
use std::fmt::Debug;


#[async_trait]
pub trait MessageFilter: Send + Sync {
    async fn process(&self, message: Message) -> anyhow::Result<Message>;
}


#[async_trait]
pub trait MessageProcessor: Send + Sync + Debug {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;
    async fn process_messages(&self, messages: Vec<Message>, security_context: &SecurityContext) -> BoxStream<'static, anyhow::Result<MessageEvent>>;
}
