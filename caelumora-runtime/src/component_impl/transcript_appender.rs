//! The `transcript_appender` component: keeps the agent's transcript up to
//! date with what it perceives.

use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
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
    async fn process_messages(&self, input: Vec<Message>, turn: &mut Turn) -> Result<Vec<Message>> {
        turn.extend_transcript(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for TranscriptAppender {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(TRANSCRIPT_APPENDER_KIND).provides::<dyn PerceiveInProcessor>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PerceiveInProcessor>(Arc::new(TranscriptAppender { }));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_impl::state_context_provider::StateContextProvider;
    use crate::context::ContextProvider;
    use crate::agent_state::AgentStateHandle;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::message::{MessageId, MessageRole};

    fn message(content: &str) -> Message {
        Message {
            id: MessageId::new(),
            role: MessageRole::User,
            content: Some(content.to_string()),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    #[tokio::test]
    async fn transcript_appender_is_passthrough_and_extends_the_transcript() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(
            AgentId::default(),
            String::new(), &mut agent_channel_state, AgentStateHandle::detached(), Default::default());
        let appender = TranscriptAppender {};
        let provider = StateContextProvider {
            prompt_providers: vec![],
        };
        let input = vec![message("ctx")];

        let output = appender
            .process_messages(input.clone(), &mut turn)
            .await
            .expect("context appender should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        // Perceived between turns, given in the next.
        turn.begin();
        provider
            .current_context(&mut turn)
            .await
            .expect("context provider should succeed");
        let given = turn.full_context().unwrap().messages;
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].id, input[0].id);
    }
}
