use async_trait::async_trait;
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use caelumora_model::message::{ActorId, Message};
use caelumora_model::security::SecurityContext;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug)]
pub struct TurnContext<'a> {
    pub scratch: HashMap<String, Value>,
    pub context: Vec<Message>,
    pub messages: Vec<Message>,
    pub security_context: SecurityContext,
    pub agent_id: AgentId,
    pub agent_name: String,
    pub agent_state: &'a mut AgentState,
    pub agent_channel_state: &'a mut AgentChannelState,
}

impl<'a> TurnContext<'a> {
    pub fn new(agent_id: AgentId, agent_name:String, agent_state: &'a mut AgentState, agent_channel_state: &'a mut AgentChannelState, security_context: SecurityContext) -> Self {
        Self {
            scratch: Default::default(),
            context: vec![],
            messages: vec![],
            security_context,
            agent_id,
            agent_name,
            agent_state,
            agent_channel_state,
        }
    }
}

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
    async fn process_messages(&self, messages: Vec<Message>, security_context: &SecurityContext) -> BoxStream<'static, anyhow::Result<MessageEvent>>;
}
