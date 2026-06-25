use crate::component::{
    ComponentFacets, ComponentRegistry, TurnContext
};
use crate::component_impl::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::message::Message;
use std::sync::Arc;


pub const STATE_HISTORY_WRITER_KIND: &str = "state_history_writer";

#[derive(Debug)]
pub struct StateHistoryWriter {
}

#[async_trait]
impl MessageProcessor for StateHistoryWriter {
    fn name(&self) -> &str {
        STATE_HISTORY_WRITER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, state: &mut AgentChannelState, _context: &mut TurnContext) -> Result<Vec<Message>> {
        state.history.extend(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryWriter {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_HISTORY_WRITER_KIND,
        Arc::new(|_descriptor| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateHistoryWriter { }));
            Ok(capabilities)
        }),
    );
}

