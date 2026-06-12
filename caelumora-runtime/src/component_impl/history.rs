use anyhow::Result;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use caelumora_model::agent::AgentState;
use caelumora_model::message::Message;
use crate::component::{MessageProcessor, PerceiveInProcessor, TurnContext, STATE_HISTORY_WRITER_KIND};

#[derive(Debug)]
pub struct StateHistoryWriter {
    pub state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl MessageProcessor for StateHistoryWriter {
    fn name(&self) -> &str {
        STATE_HISTORY_WRITER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, _context: &mut TurnContext) -> Result<Vec<Message>> {
        let mut state_guard = self.state.lock().await;
        state_guard.history.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryWriter {}

