//! The `state_history_appender` component: keeps the agent's history up to
//! date with what it perceives.

use crate::actor::TurnContext;
use crate::component::{
    ComponentFacets, ComponentRegistry
};
use crate::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::Message;
use std::sync::Arc;

pub const STATE_HISTORY_APPENDER_KIND: &str = "state_history_appender";

#[derive(Debug)]
pub struct StateHistoryAppender {
}

#[async_trait]
impl MessageProcessor for StateHistoryAppender {
    fn name(&self) -> &str {
        STATE_HISTORY_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, context: &mut TurnContext) -> Result<Vec<Message>> {
        context.extend_state_history(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryAppender {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_HISTORY_APPENDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateHistoryAppender { }));
            Ok(capabilities)
        }),
    );
}

