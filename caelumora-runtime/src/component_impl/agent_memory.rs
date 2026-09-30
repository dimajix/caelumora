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
//! within the session, and beyond it only where the entry's session permits
//! it, then and now (issue 134). Only those can be
//! forgotten here. Both tools are
//! `ToolVisibility::HIDDEN`: like private notes, memory is the agent's
//! own, seen by its owner in the context panel only. Its entries are
//! numbered differently from notes, on purpose: see `AgentMemory::remember`.

use crate::agent_memory::AgentMemory;
use crate::agent_turn::Turn;
use crate::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::session_reader::SessionReader;
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::tool::{classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::memory::{Memory, MemoryId};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::security::{SecurityContext, SecurityManager};
use caelumora_model::tool::{Tool, ToolVisibility, ToolParameters};
use serde_json::Value;
use std::sync::Arc;

pub const AGENT_MEMORY_KIND: &str = "agent_memory";

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const FORGET_TOOL_NAME: &str = "forget";
pub const RECLASSIFY_MEMORY_TOOL_NAME: &str = "reclassify_memory";

pub struct AgentMemoryComponent {
    /// The policy that decides what may be recalled, asked in
    /// `security_context` - the agent's (issue 131).
    security_manager: Arc<dyn SecurityManager>,
    security_context: SecurityContext,
    /// The agent's window to its session: whose it is, and whom it lets
    /// read what is remembered in it (issue 134).
    session_reader: Arc<dyn SessionReader>,
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
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
            Tool {
                name: FORGET_TOOL_NAME.to_string(),
                description: "Forget one of the things you remember, by its number.".to_string(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number("The number of what you remember"))],
                    ["id"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
            Tool {
                name: RECLASSIFY_MEMORY_TOOL_NAME.to_string(),
                description: "Change how one of the things you remember is classified, by its number: its \
                    level and the compartments it is meant for, never above this channel. It decides where \
                    you recall it. Releasing it - a lower level, or further compartments - makes it readable by a wider circle; a user has to approve it."
                    .to_string(),
                parameters: reclassify_parameters("what you remember"),
                requires_approval: true,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
            },
        ]
    }

    async fn execute(&self, name: &str, params: &Value, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            REMEMBER_TOOL_NAME => self.execute_remember(params, turn),
            FORGET_TOOL_NAME => self.execute_forget(params, turn).await,
            RECLASSIFY_MEMORY_TOOL_NAME => self.execute_reclassify(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        Ok(value.into())
    }
}

impl AgentMemoryComponent {
    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    fn execute_remember(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentMemory::remember(turn, text_argument(params)?, self.session_reader.as_ref());
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `reclassify_memory(id, level, compartments?)`: reclassifies an entry
    /// that may be recalled here, as the policy allows.
    async fn execute_reclassify(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentMemory::reclassify(turn, id, classification.clone(), self.session_reader.as_ref(), self.security_manager.as_ref(), &self.security_context).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
    }

    /// `forget(id)`: forgets an entry that may be recalled here.
    async fn execute_forget(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        if AgentMemory::forget(turn, id, self.session_reader.as_ref(), self.security_manager.as_ref(), &self.security_context).await {
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
        let recalled = AgentMemory::recalled(turn, self.session_reader.as_ref(), self.security_manager.as_ref(), &self.security_context).await;
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        // Once for every channel the entries come from, so that knowledge
        // from another session can be told (issue 134).
        let source = KnowledgeSource::Memory { agent: turn.agent_id(), digest: AgentMemory::digest(turn) };
        let mut knowledge: Vec<KnowledgeRef> = Vec::new();
        for memory in &recalled {
            if !knowledge.iter().any(|k| k.channel == memory.origin.channel) {
                knowledge.push(KnowledgeRef { channel: memory.origin.channel, source });
            }
        }
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
                session_reader: context.session_reader.clone(),
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
    use caelumora_model::confidentiality::{ChannelRef, Classification, Confidentiality};

    /// A channel of the tests: where it is, and how it is classified.
    type At = (ChannelRef, Classification);
    use crate::tool::ToolError;
    use caelumora_model::security::{ActionType, OwnershipSecurityManager};
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::store::StateStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use caelumora_model::privacy::{PrivacyConfig, SessionPrivacy};
    use crate::session_reader::{ChannelHistory, ChannelInfo, ChannelNotes, PublishedNotes, ReadError, ReadResult};

    /// The context of an envelope routed in the channel `at`.
    fn in_channel(at: &At) -> caelumora_model::actor::EventContext {
        caelumora_model::actor::EventContext::new(at.0.session_id, at.0.channel_id, ActorId::None, String::new())
    }

    fn channel(session: SessionId, level: Confidentiality) -> At {
        (ChannelRef { session_id: session, channel_id: ChannelId::new() }, Classification::general(level))
    }

    /// The component of an agent owned by `owner`, asking `policy`.
    fn component(policy: Arc<dyn SecurityManager>, owner: ActorId) -> AgentMemoryComponent {
        AgentMemoryComponent {
            security_manager: policy,
            security_context: SecurityContext::new(owner, Default::default(), Default::default()),
            session_reader: crate::session_reader::detached(),
        }
    }

    async fn handle(agent: AgentId) -> Arc<AgentStateHandle> {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        AgentStates::new(store, Arc::new(())).get(agent, Default::default()).await.unwrap()
    }

    /// Remembers `text` in a turn in channel `at`, and completes the turn.
    async fn remember(memory: &AgentMemoryComponent, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At, text: &str) {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new());
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": text }), &mut turn).await.unwrap();
        turn.commit_turn().await;
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(memory: &AgentMemoryComponent, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At) -> Prompt {
        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new());
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
            turn.next_turn(in_channel(&team), Default::default(), team.1.clone(), Vec::new());
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), &mut turn).await.unwrap();
            assert_eq!(result.value["id"], 1);
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
        turn.next_turn(in_channel(&elsewhere), Default::default(), elsewhere.1.clone(), Vec::new());
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.is_err());
        turn.next_turn(in_channel(&team), Default::default(), team.1.clone(), Vec::new());
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

    #[tokio::test]
    async fn an_entry_released_to_another_compartment_is_recalled_there() {
        use caelumora_model::confidentiality::{Compartment, Compartments};
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_compartments(compartments));
        // As the agent's components are given it: its owner's, called by the agent.
        let memory = AgentMemoryComponent {
            security_manager: policy,
            security_context: SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default()).next(ActorId::Agent(agent)),
            session_reader: crate::session_reader::detached(),
        };
        let session = SessionId::new();
        let in_compartments = |names: &[&str]| -> At {
            (ChannelRef { session_id: session, channel_id: ChannelId::new() }, Classification::new(Confidentiality::Internal, names.iter().map(|n| Compartment::new(*n))).unwrap())
        };
        let (hr, finance) = (in_compartments(&["hr"]), in_compartments(&["finance"]));
        remember(&memory, &handle, agent, hr.clone(), "The budget is tight.").await;
        assert!(recall(&memory, &handle, agent, finance.clone()).await.text.is_empty(), "HR's, not finance's");

        let mut state = AgentChannelState::default();
        let mut turn = Turn::new(agent, "agent".to_string(), &mut state, handle.clone(), Default::default());
        turn.next_turn(in_channel(&hr), Default::default(), hr.1.clone(), Vec::new());
        let above = serde_json::json!({ "id": 1, "level": "confidential", "compartments": ["hr"] });
        assert!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &above, &mut turn).await.is_err(), "never above the channel");
        let unknown = serde_json::json!({ "id": 7, "level": "internal", "compartments": ["hr"] });
        assert!(matches!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &unknown, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        let released = serde_json::json!({ "id": 1, "level": "internal", "compartments": ["hr", "finance"] });
        memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &released, &mut turn).await.expect("the agent releases what it remembers");
        turn.commit_turn().await;

        let there = recall(&memory, &handle, agent, finance).await;
        assert!(there.text.contains("The budget is tight."), "released to finance: {}", there.text);
        assert!(recall(&memory, &handle, agent, hr).await.text.contains("The budget is tight."), "and still HR's");
    }

    /// A session reader that knows only privacy: its own session's, and
    /// that of others as `sessions` says - `None` for one that is gone. A
    /// session it does not know cannot be looked up.
    #[derive(Debug)]
    struct Privacies {
        own: SessionPrivacy,
        sessions: std::sync::Mutex<Vec<(SessionId, Option<SessionPrivacy>)>>,
    }

    impl Privacies {
        fn set(&self, id: SessionId, privacy: Option<SessionPrivacy>) {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.retain(|(s, _)| *s != id);
            sessions.push((id, privacy));
        }
    }

    #[async_trait]
    impl SessionReader for Privacies {
        fn own_channel(&self) -> Option<ChannelInfo> {
            None
        }

        fn own_privacy(&self) -> SessionPrivacy {
            self.own.clone()
        }

        async fn privacy_of(&self, id: SessionId) -> ReadResult<Option<SessionPrivacy>> {
            let sessions = self.sessions.lock().unwrap();
            match sessions.iter().find(|(s, _)| *s == id) {
                Some((_, privacy)) => Ok(privacy.clone()),
                None => Err(ReadError::Failed(anyhow::anyhow!("unknown session"))),
            }
        }

        async fn channel_notes(&self) -> ReadResult<Vec<PublishedNotes>> {
            Ok(Vec::new())
        }

        async fn readable_channels(&self) -> ReadResult<Vec<ChannelInfo>> {
            Ok(Vec::new())
        }

        async fn notes_of(&self, name: &str) -> ReadResult<(ChannelInfo, Vec<PublishedNotes>)> {
            Err(ReadError::NotReadable(name.to_string()))
        }

        async fn history_of(&self, name: &str) -> ReadResult<ChannelHistory> {
            Err(ReadError::NotReadable(name.to_string()))
        }

        async fn private_notes(&self, _agent: AgentId) -> ReadResult<Vec<ChannelNotes>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn an_entry_is_recalled_in_another_session_where_its_session_agreed_then_and_now() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let (alice, bob) = (UserId::new(), UserId::new());
        let session_of = |policy, owner: UserId| SessionPrivacy { policy, owner: ActorId::User(owner), people: [owner].into_iter().collect() };
        let (hers, other) = (SessionId::new(), SessionId::new());
        let in_session = |own: SessionPrivacy| {
            let reader = Arc::new(Privacies { own, sessions: std::sync::Mutex::new(Vec::new()) });
            let memory = AgentMemoryComponent {
                security_manager: Arc::new(OwnershipSecurityManager::new()),
                security_context: SecurityContext::new(ActorId::User(alice), Default::default(), Default::default()),
                session_reader: reader.clone(),
            };
            (memory, reader)
        };

        // Remembered in a session of hers that lets her other sessions read it ...
        let (writer, _) = in_session(session_of(PrivacyConfig::SameOwner, alice));
        let origin = channel(hers, Confidentiality::Internal);
        remember(&writer, &handle, agent, origin.clone(), "Kaya likes tea.").await;
        let remembered = handle.current().memory.current();
        assert_eq!(remembered[0].origin.session.policy, PrivacyConfig::SameOwner, "the entry keeps its session's privacy");

        // ... and recalled in another of hers, while the session still agrees.
        let (memory, reader) = in_session(session_of(PrivacyConfig::Private, alice));
        let elsewhere = channel(other, Confidentiality::Internal);
        reader.set(hers, Some(session_of(PrivacyConfig::SameOwner, alice)));
        let recalled = recall(&memory, &handle, agent, elsewhere.clone()).await;
        assert!(recalled.text.contains("Kaya likes tea."), "{}", recalled.text);
        assert_eq!(recalled.knowledge.len(), 1);
        assert_eq!(recalled.knowledge[0].channel, origin.0, "recorded with the channel it came from, of its session");

        reader.set(hers, Some(session_of(PrivacyConfig::Private, alice)));
        assert!(recall(&memory, &handle, agent, elsewhere.clone()).await.text.is_empty(), "no longer, once the session is private");
        reader.set(hers, None);
        assert!(recall(&memory, &handle, agent, elsewhere.clone()).await.text.contains("Kaya likes tea."), "of a session that is gone, what it agreed to then");
        reader.sessions.lock().unwrap().clear();
        assert!(recall(&memory, &handle, agent, elsewhere).await.text.is_empty(), "nothing, where the session cannot be looked up");

        let (bobs, reader) = in_session(session_of(PrivacyConfig::Private, bob));
        reader.set(hers, Some(session_of(PrivacyConfig::SameOwner, alice)));
        assert!(recall(&bobs, &handle, agent, channel(SessionId::new(), Confidentiality::Internal)).await.text.is_empty(), "not in someone else's session");

        // What was said while the session was private stays there, even
        // after it lets others read.
        let (private, _) = in_session(session_of(PrivacyConfig::Private, alice));
        let quiet = channel(hers, Confidentiality::Internal);
        remember(&private, &handle, agent, quiet, "Kaya is moving.").await;
        let (memory, reader) = in_session(session_of(PrivacyConfig::Private, alice));
        reader.set(hers, Some(session_of(PrivacyConfig::SameOwner, alice)));
        let recalled = recall(&memory, &handle, agent, channel(other, Confidentiality::Internal)).await;
        assert!(recalled.text.contains("Kaya likes tea.") && !recalled.text.contains("Kaya is moving."), "{}", recalled.text);
    }
}
