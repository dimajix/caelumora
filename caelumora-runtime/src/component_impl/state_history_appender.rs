//! The `state_history_appender` component: keeps the agent's history up to
//! date with what it perceives.

use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentKind, ComponentRegistry};
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
    async fn process_messages(&self, input: Vec<Message>, turn: &mut Turn) -> Result<Vec<Message>> {
        turn.extend_history(input.clone());
        Ok(input)
    }
}

impl PerceiveInProcessor for StateHistoryAppender {}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(STATE_HISTORY_APPENDER_KIND).provides::<dyn PerceiveInProcessor>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PerceiveInProcessor>(Arc::new(StateHistoryAppender { }));
            Ok(facets)
        },
    );
}


#[cfg(test)]
mod tests {
    use super::*;
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
    async fn state_history_appender_is_passthrough_and_persists_history() {
        let mut agent_channel_state = AgentChannelState::default();
        let mut turn = Turn::new(
            AgentId::default(),
            String::new(), &mut agent_channel_state, AgentStateHandle::detached(), Default::default());
        let processor = StateHistoryAppender {};
        let input = vec![message("hello")];

        let output = processor
            .process_messages(input.clone(), &mut turn)
            .await
            .expect("history processor should succeed");

        assert_eq!(output.len(), input.len());
        assert_eq!(output[0].id, input[0].id);

        let history = agent_channel_state.history.messages();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, input[0].id);
    }
}
