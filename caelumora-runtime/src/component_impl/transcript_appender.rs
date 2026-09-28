//! The `transcript_appender` component: keeps the agent's transcript up to
//! date with what it perceives.

use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::message::{MessageProcessor, PerceiveInProcessor};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::Message;
use std::sync::Arc;

pub const TRANSCRIPT_APPENDER_KIND: &str = "transcript_appender";

#[derive(Debug)]
pub struct TranscriptAppender {
}

#[async_trait]
impl MessageProcessor for TranscriptAppender {
    fn name(&self) -> &str {
        TRANSCRIPT_APPENDER_KIND
    }

    async fn process_messages(&self, input: Vec<Message>, turn: &mut Turn) -> Result<Vec<Message>> {
        turn.extend_transcript(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for TranscriptAppender {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        TRANSCRIPT_APPENDER_KIND,
        Box::new(|_descriptor, _context| {
            let mut capabilities = ComponentFacets::empty();
            capabilities.insert::<dyn PerceiveInProcessor>(Arc::new(TranscriptAppender { }));
            Ok(capabilities)
        }),
    );
}
