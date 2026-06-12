use anyhow::Result;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use caelumora_model::agent::AgentState;
use caelumora_model::message::Message;
use crate::component::{ContextProvider, MessageProcessor, PerceiveInProcessor, TurnContext, STATE_CONTEXT_APPENDER_KIND};

#[derive(Debug)]
pub struct StateContextAppender {
    pub state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl MessageProcessor for StateContextAppender {
    fn name(&self) -> &str {
        STATE_CONTEXT_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, _context: &mut TurnContext) -> Result<Vec<Message>> {
        let mut state_guard = self.state.lock().await;
        state_guard.context.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateContextAppender {}

#[derive(Debug)]
pub struct StateContextProvider {
    pub state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn current_context(&self) -> Result<Vec<Message>> {
        let state_guard = self.state.lock().await;
        Ok(state_guard.context.messages())
    }
}
