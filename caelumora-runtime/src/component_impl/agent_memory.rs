//! The `agent_memory` component (issue 62, step 3): an agent remembers what
//! it should know across its channels and sessions - kept in its
//! `AgentState`, not in one channel.
//!
//! It translates the tools `remember` and `forget` and renders the prompt
//! part; the logic is `crate::agent_memory::AgentMemory`. Changes are applied
//! to the agent's state as it is when the turn completes
//! (`Turn::commit_agent_state`), so that actors of the same agent in
//! other channels and sessions neither lose nor overwrite them. The prompt
//! part shows the entries that may be recalled in the current channel: those
//! written in a channel it may read (`ChannelClassification::readable_from`,
//! rule 2 of issue 61) - an entry from a `public` channel everywhere, from a
//! `session` channel within that session, from a `channel` channel only
//! there. Only those can be forgotten here. Both tools are
//! `ToolConfidentiality::HIDDEN`: like private notes, memory is the agent's
//! own, seen by its owner in the context panel only.

use crate::agent_turn::Turn;
use crate::agent_memory::AgentMemory;
use crate::component::{ComponentFacets, ComponentRegistry};
use crate::component_impl::notes::note_text;
use crate::context::{Prompt, PromptPosition, PromptProvider};
use crate::tool::{ToolError, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::memory::{Memory, MemoryId};
use caelumora_model::message::{KnowledgeRef, KnowledgeSource};
use caelumora_model::tool::{Tool, ToolConfidentiality, ToolParameters};
use serde_json::Value;
use std::fmt::Write;
use std::sync::Arc;

pub const AGENT_MEMORY_KIND: &str = "agent_memory";

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const FORGET_TOOL_NAME: &str = "forget";

#[derive(Debug)]
pub struct AgentMemoryComponent;

#[async_trait]
impl ToolProvider for AgentMemoryComponent {
    fn tools(&self) -> Vec<Tool> {
        vec![
            Tool {
                name: REMEMBER_TOOL_NAME.to_string(),
                description: "Remember something beyond this conversation - in your other channels and \
                    sessions too, wherever what was said here may be known. No other participant sees it; \
                    only the person responsible for you can inspect it. Do not quote or reveal it in your \
                    answers. Keep an entry short and self-contained; to change one, forget it and remember \
                    the new version."
                    .to_string(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string("What to remember"))],
                    ["text"],
                ),
                requires_approval: false,
                confidentiality: ToolConfidentiality::HIDDEN,
            },
            Tool {
                name: FORGET_TOOL_NAME.to_string(),
                description: "Forget one of the things you remember, by its number.".to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of what you remember"))],
                    ["id"],
                ),
                requires_approval: false,
                confidentiality: ToolConfidentiality::HIDDEN,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, context: &mut Turn) -> ToolResult<Value> {
        match name {
            REMEMBER_TOOL_NAME => self.execute_remember(params, context),
            FORGET_TOOL_NAME => self.execute_forget(params, context),
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

impl AgentMemoryComponent {
    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    fn execute_remember(&self, params: &Value, context: &mut Turn<'_>) -> ToolResult<Value> {
        let text = note_text(params)?;
        let id = AgentMemory::new(context).remember(text)
            .ok_or_else(|| ToolError::ExecutionFailed("nothing can be remembered here".to_string()))?;
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `forget(id)`: forgets an entry that may be recalled here.
    fn execute_forget(&self, params: &Value, context: &mut Turn<'_>) -> ToolResult<Value> {
        let id = params.get("id").and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .map(MemoryId)
            .ok_or_else(|| ToolError::InvalidParameter("'id' must be the number of something you remember".to_string()))?;
        if AgentMemory::new(context).forget(id) {
            Ok(serde_json::json!({ "status": "forgotten", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you remember nothing numbered {id} here")))
        }
    }
}

#[async_trait]
impl PromptProvider for AgentMemoryComponent {
    fn position(&self) -> PromptPosition {
        PromptPosition::BeforeTurnInput
    }

    async fn get_prompt(&self, context: &Turn) -> Result<Prompt> {
        let recalled = AgentMemory::recalled(context);
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        let knowledge = vec![KnowledgeRef {
            channel: context.channel_id,
            source: KnowledgeSource::Memory { agent: context.agent_id, digest: AgentMemory::digest(context) },
        }];
        Ok(Prompt { text: render(&recalled), knowledge })
    }
}

/// The prompt part for what the agent recalls here.
fn render(recalled: &[Memory]) -> String {
    let mut text = String::from(
        "[What you remember - from this and your other conversations, as far as it may be known here. \
        No other participant sees it; only the person responsible for you can inspect it. Do not quote or \
        reveal it in your answers. Change it with `remember` and `forget`.]\n",
    );
    for memory in recalled {
        let _ = writeln!(text, "- #{}: {}", memory.id, memory.text);
    }
    text
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        AGENT_MEMORY_KIND,
        Box::new(|_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(AgentMemoryComponent));
            facets.insert::<dyn PromptProvider>(Arc::new(AgentMemoryComponent));
            Ok(facets)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::AgentStates;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::{ChannelClassification, Confidentiality};
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::store::StateStore;
    use caelumora_model::store_impl::memory::MemoryStore;

    fn channel(session: SessionId, level: Confidentiality) -> ChannelClassification {
        ChannelClassification { session_id: session, channel_id: ChannelId::new(), level }
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(handle: &Arc<crate::agent_state::AgentStateHandle>, agent: AgentId, at: ChannelClassification) -> Prompt {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone());
        turn.set_classification(Some(at));
        AgentMemoryComponent.get_prompt(&turn).await.unwrap()
    }

    #[tokio::test]
    async fn what_is_remembered_is_recalled_wherever_its_origin_may_be_read() {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        let states = AgentStates::new(store, Arc::new(()));
        let agent = AgentId::new();
        let handle = states.get(agent, Default::default()).await.unwrap();
        let memory = AgentMemoryComponent;
        let (session, other_session) = (SessionId::new(), SessionId::new());
        let (team, public, elsewhere) = (channel(session, Confidentiality::Session), channel(session, Confidentiality::Public), channel(other_session, Confidentiality::Session));

        // A turn in a session-level channel remembers something ...
        let mut state = AgentChannelState::default();
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone());
            turn.set_classification(Some(team));
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), &mut turn).await.unwrap();
            assert_eq!(result["id"], 1);
            assert!(turn.agent_state().current().memory.current().is_empty(), "not before the turn completes");
            turn.commit_agent_state().await.unwrap();
        }

        // ... which another actor of the agent recalls in a channel that may
        // read that one, and not where it may not be known.
        let same_session = recall(&handle, agent, channel(session, Confidentiality::Channel)).await;
        assert!(same_session.text.contains("#1: Kaya likes tea."), "{}", same_session.text);
        assert_eq!(same_session.knowledge.len(), 1, "recorded as knowledge read");
        assert!(recall(&handle, agent, public).await.text.is_empty(), "a public channel may not read a session-level one");
        assert!(recall(&handle, agent, elsewhere).await.text.is_empty(), "another session");

        // Forgotten only where it may be recalled.
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone());
        turn.set_classification(Some(elsewhere));
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.is_err());
        turn.set_classification(Some(team));
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
        turn.commit_agent_state().await.unwrap();
        assert!(handle.current().memory.current().is_empty());
    }
}
