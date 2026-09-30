//! The `agent_memory` component (issue 62, step 3): an agent remembers what
//! it should know across its channels and sessions - kept in its
//! `AgentState`, not in one channel.
//!
//! It translates the tools `remember` and `forget` and renders the prompt
//! part; the logic is `crate::agent_memory::AgentMemory`. Changes are applied
//! to the agent's state as it is when the turn completes
//! (`Turn::commit_turn`), so that actors of the same agent in
//! other channels and sessions neither lose nor overwrite them. The prompt
//! part shows the entries that may be recalled in the current channel: those
//! written in a channel it may read, as the policy decides when the
//! component asks in the agent's name (`AccessQuestions::may_channel_read`,
//! issue 131). Under the ownership policy that is the flow rule of issue
//! 133 - the entry's channel's classification must flow into this one's -
//! within the session, and from public channels beyond it. Only those can be
//! forgotten here. Both tools are
//! `ToolConfidentiality::HIDDEN`: like private notes, memory is the agent's
//! own, seen by its owner in the context panel only. Its entries are
//! numbered differently from notes, on purpose: see `AgentMemory::remember`.

use crate::agent_memory::AgentMemory;
use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::tool::{id_argument, text_argument, ToolError, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::memory::{Memory, MemoryId};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::tool::{Tool, ToolConfidentiality, ToolParameters};
use serde_json::Value;
use std::sync::Arc;

pub const AGENT_MEMORY_KIND: &str = "agent_memory";

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const FORGET_TOOL_NAME: &str = "forget";

pub struct AgentMemoryComponent {
    /// The policy that decides what may be recalled, asked in
    /// `security_context` - the agent's (issue 131).
    security_manager: Arc<dyn SecurityManager>,
    security_context: SecurityContext,
}

impl std::fmt::Debug for AgentMemoryComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMemoryComponent").field("security_context", &self.security_context).finish()
    }
}

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

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<Value> {
        match name {
            REMEMBER_TOOL_NAME => self.execute_remember(params, turn),
            FORGET_TOOL_NAME => self.execute_forget(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }
    }
}

impl AgentMemoryComponent {
    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    fn execute_remember(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentMemory::remember(turn, text_argument(params)?);
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `forget(id)`: forgets an entry that may be recalled here.
    async fn execute_forget(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        if AgentMemory::forget(turn, id, self.security_manager.as_ref(), &self.security_context).await {
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

    async fn get_prompt(&self, turn: &Turn) -> Result<Prompt> {
        let recalled = AgentMemory::recalled(turn, self.security_manager.as_ref(), &self.security_context).await;
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        let knowledge = vec![KnowledgeRef {
            channel: turn.channel_id(),
            source: KnowledgeSource::Memory { agent: turn.agent_id(), digest: AgentMemory::digest(turn) },
        }];
        Ok(Prompt { text: render(&recalled), knowledge })
    }
}

/// The prompt part for what the agent recalls here.
fn render(recalled: &[Memory]) -> String {
    let preamble = format!(
        "[What you remember - from this and your other conversations, as far as it may be known here. \
        No other participant sees it; only the person responsible for you can inspect it. Do not quote or \
        reveal it in your answers. Change it with `{REMEMBER_TOOL_NAME}` and `{FORGET_TOOL_NAME}`.]"
    );
    render_sections(&preamble, &[Section::lines(recalled.iter().map(|m| format!("#{}: {}", m.id, m.text)).collect())])
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new(AGENT_MEMORY_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>(),
        |_descriptor, context| {
            let component = Arc::new(AgentMemoryComponent {
                security_manager: context.security_manager.clone(),
                security_context: context.security_context.clone(),
            });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(component.clone());
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::{AgentStateHandle, AgentStates};
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::confidentiality::{ChannelClassification, Classification, Confidentiality};
    use caelumora_model::security::{ActionType, OwnershipSecurityManager};
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::store::StateStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::UserId;

    fn channel(session: SessionId, level: Confidentiality) -> ChannelClassification {
        ChannelClassification { session_id: session, channel_id: ChannelId::new(), classification: Classification::general(level) }
    }

    /// The component of an agent owned by `owner`, asking `policy`.
    fn component(policy: Arc<dyn SecurityManager>, owner: ActorId) -> AgentMemoryComponent {
        AgentMemoryComponent {
            security_manager: policy,
            security_context: SecurityContext::new(owner, Default::default(), Default::default()),
        }
    }

    async fn handle(agent: AgentId) -> Arc<AgentStateHandle> {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        AgentStates::new(store, Arc::new(())).get(agent, Default::default()).await.unwrap()
    }

    /// Remembers `text` in a turn in channel `at`, and completes the turn.
    async fn remember(memory: &AgentMemoryComponent, handle: &Arc<AgentStateHandle>, agent: AgentId, at: ChannelClassification, text: &str) {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(Default::default(), Default::default(), at, Vec::new());
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": text }), &mut turn).await.unwrap();
        turn.commit_turn().await;
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(memory: &AgentMemoryComponent, handle: &Arc<AgentStateHandle>, agent: AgentId, at: ChannelClassification) -> Prompt {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(Default::default(), Default::default(), at, Vec::new());
        memory.get_prompt(&turn).await.unwrap()
    }

    #[tokio::test]
    async fn what_is_remembered_is_recalled_wherever_its_origin_may_be_read() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let memory = component(Arc::new(OwnershipSecurityManager::new()), ActorId::User(UserId::new()));
        let (session, other_session) = (SessionId::new(), SessionId::new());
        let (team, public, elsewhere) = (channel(session, Confidentiality::Internal), channel(session, Confidentiality::Public), channel(other_session, Confidentiality::Internal));

        // A turn in an internal channel remembers something ...
        let mut state = AgentChannelState::default();
        {
            let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
            turn.next_turn(Default::default(), Default::default(), team.clone(), Vec::new());
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), &mut turn).await.unwrap();
            assert_eq!(result["id"], 1);
            assert!(turn.agent_state().current().memory.current().is_empty(), "not before the turn completes");
            turn.commit_turn().await;
        }

        // ... which another actor of the agent recalls in a channel that may
        // read that one, and not where it may not be known.
        let same_session = recall(&memory, &handle, agent, channel(session, Confidentiality::Confidential)).await;
        assert!(same_session.text.contains("#1: Kaya likes tea."), "{}", same_session.text);
        assert_eq!(same_session.knowledge.len(), 1, "recorded as knowledge read");
        assert!(recall(&memory, &handle, agent, public).await.text.is_empty(), "a public channel may not read an internal one");
        assert!(recall(&memory, &handle, agent, elsewhere.clone()).await.text.is_empty(), "another session");

        // Forgotten only where it may be recalled.
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(Default::default(), Default::default(), elsewhere, Vec::new());
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.is_err());
        turn.next_turn(Default::default(), Default::default(), team, Vec::new());
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
        turn.commit_turn().await;
        assert!(handle.current().memory.current().is_empty());
    }

    /// Lets channels read each other only in the name of `owner`.
    #[derive(Debug)]
    struct OnlyFor(ActorId);
    #[async_trait]
    impl SecurityManager for OnlyFor {
        async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
            matches!(action, ActionType::ReadChannel { .. }) && context.initiator_id == self.0
        }
    }

    #[tokio::test]
    async fn the_policy_decides_what_is_recalled_asked_in_the_agents_name() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let (owner, stranger) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let policy: Arc<dyn SecurityManager> = Arc::new(OnlyFor(owner));
        let session = SessionId::new();
        // Public, so that the built-in rule would let every channel read it.
        let (origin, reader) = (channel(session, Confidentiality::Public), channel(session, Confidentiality::Internal));
        remember(&component(policy.clone(), owner), &handle, agent, origin.clone(), "Kaya likes tea.").await;

        let asked_by_owner = recall(&component(policy.clone(), owner), &handle, agent, reader.clone()).await;
        assert!(asked_by_owner.text.contains("Kaya likes tea."), "{}", asked_by_owner.text);
        let asked_by_stranger = recall(&component(policy.clone(), stranger), &handle, agent, reader).await;
        assert!(asked_by_stranger.text.is_empty(), "the policy refuses, whatever the levels say: {}", asked_by_stranger.text);
        let in_its_own_channel = recall(&component(policy, stranger), &handle, agent, origin).await;
        assert!(in_its_own_channel.text.contains("Kaya likes tea."), "a channel always reads itself");
    }
}
