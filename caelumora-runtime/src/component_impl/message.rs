use crate::component::TurnContext;
use async_trait::async_trait;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::message::Message;

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn process_messages(&self, input: Vec<Message>, state: &mut AgentChannelState, context: &mut TurnContext) -> anyhow::Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait PlanPreProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}
