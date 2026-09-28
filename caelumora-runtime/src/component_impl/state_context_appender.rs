//! The `state_context_appender` component: keeps the agent's stored
//! context up to date with what it perceives.

use crate::agent_turn::TurnContext;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::Message;
use std::sync::Arc;

pub const STATE_CONTEXT_APPENDER_KIND: &str = "state_context_appender";

#[derive(Debug)]
pub struct StateContextAppender {
}

#[async_trait]
impl MessageProcessor for StateContextAppender {
    fn name(&self) -> &str {
        STATE_CONTEXT_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, context: &mut TurnContext) -> Result<Vec<Message>> {
        context.extend_state_context(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateContextAppender {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        STATE_CONTEXT_APPENDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(StateContextAppender { }));
            Ok(capabilities)
        }),
    );
}
