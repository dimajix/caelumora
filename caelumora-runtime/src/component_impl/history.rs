use anyhow::Result;
use std::sync::Arc;
use async_trait::async_trait;
use caelumora_model::agent::AgentState;
use caelumora_model::message::Message;
use crate::component::{
    ComponentFacets, ComponentRegistry, TurnContext
};
use crate::component_impl::message::{MessageProcessor, PerceiveInProcessor};


pub const STATE_HISTORY_WRITER_KIND: &str = "state_history_writer";

#[derive(Debug)]
pub struct StateHistoryWriter {
}

#[async_trait]
impl MessageProcessor for StateHistoryWriter {
    fn name(&self) -> &str {
        STATE_HISTORY_WRITER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, state: &mut AgentState, _context: &mut TurnContext) -> Result<Vec<Message>> {
        state.history.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryWriter {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_HISTORY_WRITER_KIND,
        Arc::new(|descriptor| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateHistoryWriter { }));
            Ok(capabilities)
        }),
    );
}

