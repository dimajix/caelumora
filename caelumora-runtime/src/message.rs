//! The facets of the agent loop's message stages: components that see the
//! messages an agent perceives (`PerceiveInProcessor`) or emits
//! (`EmitOutProcessor`), and may change them.

use crate::agent_turn::Turn;
use async_trait::async_trait;
use caelumora_model::message::Message;

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn process_messages(&self, input: Vec<Message>, context: &mut Turn) -> anyhow::Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}
