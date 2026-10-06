//! The `agent_memory` component (issue 62, step 3): an agent remembers what
//! it should know across its channels and sessions - kept in its
//! `AgentState`, not in one channel.
//!
//! It translates the tools `remember` and `forget` and renders the prompt
//! part; the logic is `crate::agent_memory::AgentMemory`. Changes are applied
//! to the agent's state as it is when the turn completes
//! (`Turn::commit`), so that actors of the same agent in
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
use crate::context::{render_sections, Prompt, PromptPosition, PromptProvider, Section};
use crate::work::AgentWork;
use crate::tool::{classification_argument, id_argument, reclassify_parameters, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
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
}

impl std::fmt::Debug for AgentMemoryComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMemoryComponent").field("security_context", &self.security_context).finish()
    }
}

#[async_trait]
impl ToolProvider for AgentMemoryComponent {
    fn description(&self) -> String {
        "Your memory: what you should know across all your channels and sessions.".to_string()
    }

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
        // What these answer tells only what the model asked for: classified
        // as the turn (issue 168).
        Ok(ToolOutput::classified(value, turn.label()))
    }
}

impl AgentMemoryComponent {
    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    fn execute_remember(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentMemory::remember(turn, text_argument(params)?);
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `reclassify_memory(id, level, compartments?)`: reclassifies an entry
    /// that may be recalled here, as the policy allows.
    async fn execute_reclassify(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentMemory::reclassify(turn, id, classification.clone(), self.security_manager.as_ref(), &self.security_context).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
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

    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt> {
        let recalled = AgentMemory::recalled(work, self.security_manager.as_ref(), &self.security_context).await;
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        // Once for every channel the entries come from, so that knowledge
        // from another session can be told (issue 134).
        let source = KnowledgeSource::Memory { agent: work.agent_id(), digest: AgentMemory::digest(work) };
        let mut knowledge: Vec<KnowledgeRef> = Vec::new();
        for memory in &recalled {
            if !knowledge.iter().any(|k| k.channel == memory.origin.channel) {
                let from_there = recalled.iter().filter(|m| m.origin.channel == memory.origin.channel).map(|m| &m.classification);
                knowledge.push(KnowledgeRef { channel: memory.origin.channel, session: memory.origin.session.id, source, classification: Classification::join_all(from_there) });
            }
        }
        Ok(Prompt::of_knowledge(render(&recalled), knowledge))
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
                security_manager: Arc::clone(&context.security_manager),
                security_context: context.security_context.clone(),
            });
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<AgentMemoryComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_state::{AgentStateHandle, AgentStates};
    use crate::runtime::RuntimeManager;
    use crate::session::SessionLoop;
    use crate::tool::ToolError;
    use crate::view::ViewsFactory;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::channel::Participant;
    use caelumora_model::command::default_commands;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::privacy::{PrivacyConfig, SessionPrivacy};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{ActionType, OwnershipSecurityManager, SecurityAttributes};
    use caelumora_model::session::{ChannelId, Session, SessionId};
    use caelumora_model::store::StateStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use std::collections::HashMap;

    /// A channel of the tests: where it is - its session and itself - and
    /// how it is classified.
    type At = ((SessionId, ChannelId), Classification);

    /// The context of an envelope routed in the channel `at`.
    fn in_channel(at: &At) -> caelumora_model::actor::EventContext {
        caelumora_model::actor::EventContext::new(at.0.0, at.0.1, ActorId::None, String::new())
    }

    fn channel(session: SessionId, level: Confidentiality) -> At {
        ((session, ChannelId::new()), Classification::general(level))
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

    /// The sessions of a test, as a turn's views reach them: stored in the
    /// runtime's store, and running - without channels - so that a turn
    /// in one of them reads its privacy from its loop.
    struct Sessions {
        runtime: Arc<RuntimeManager>,
        loops: HashMap<SessionId, Arc<SessionLoop>>,
    }

    impl Sessions {
        async fn under(policy: Arc<dyn SecurityManager>) -> Self {
            let store = Arc::new(MemoryStore::new());
            let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
            let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, default_commands(), Default::default(), Vec::new(), crate::component::ComponentRegistry::built_in(store)).unwrap());
            Self { runtime, loops: HashMap::new() }
        }

        /// Session `id` as privacy sees it as `privacy`, stored and running.
        async fn add(&mut self, id: SessionId, privacy: SessionPrivacy) -> &mut Self {
            let session = session_with(id, &privacy);
            self.runtime.entity_store().create_session(&SecurityContext::system(), &session).await.unwrap();
            self.loops.insert(id, SessionLoop::new(session, vec![], Arc::clone(&self.runtime)).await.unwrap());
            self
        }

        /// Session `id` lets others read as `privacy` says from now on -
        /// stored, as a change to a running session is stored first - or
        /// is gone (`None`).
        async fn set(&mut self, id: SessionId, privacy: Option<SessionPrivacy>) {
            let store = self.runtime.entity_store();
            match privacy {
                Some(privacy) => {
                    let session = session_with(id, &privacy);
                    if store.update_session(&SecurityContext::system(), &session).await.is_err() {
                        // Gone meanwhile: back again.
                        store.create_session(&SecurityContext::system(), &session).await.unwrap();
                    }
                    self.loops.insert(id, SessionLoop::new(session, vec![], Arc::clone(&self.runtime)).await.unwrap());
                }
                None => {
                    store.delete_session(&SecurityContext::system(), id).await.unwrap();
                    store.purge_session(&SecurityContext::system(), id).await.unwrap();
                    self.loops.remove(&id);
                }
            }
        }

        /// What a turn of `memory`'s agent in channel `at` navigates.
        fn views_in(&self, at: &At, memory: &AgentMemoryComponent) -> ViewsFactory {
            let session = Arc::clone(self.loops.get(&at.0.0).expect("a session of the test"));
            ViewsFactory::in_session(session, memory.security_context.clone())
        }
    }

    /// A session `id` that privacy sees as `privacy`.
    fn session_with(id: SessionId, privacy: &SessionPrivacy) -> Session {
        let now = chrono::Utc::now();
        Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            id,
            name: String::new(),
            participants: privacy.people.iter().map(|user| Participant::User(*user)).collect(),
            channels: Vec::new(),
            commands: Default::default(),
            privacy: privacy.policy,
            created_at: now,
            updated_at: now,
            security_attributes: SecurityAttributes { owner_id: privacy.owner, contributors: Vec::new() },
            deleted: None,
        }
    }

    fn session_of(policy: PrivacyConfig, owner: UserId) -> SessionPrivacy {
        SessionPrivacy { id: SessionId::new(), policy, owner: ActorId::User(owner), people: [owner].into_iter().collect() }
    }

    /// Remembers `text` in a turn in channel `at`, and completes the turn.
    /// Remembers `text` in a turn in channel `at` that was told something
    /// classified as the channel.
    async fn remember(memory: &AgentMemoryComponent, sessions: &Sessions, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At, text: &str) {
        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(handle)).expect("a turn was begun").with_views(sessions.views_in(&at, memory));
        turn.see(&at.1);
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": text }), &mut turn).await.unwrap();
        turn.commit(false).await;
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(memory: &AgentMemoryComponent, sessions: &Sessions, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At) -> Prompt {
        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new(), Default::default());
        let turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(handle)).expect("a turn was begun").with_views(sessions.views_in(&at, memory));
        memory.get_prompt(&turn.work()).await.unwrap()
    }

    #[tokio::test]
    async fn what_is_remembered_is_recalled_wherever_its_origin_may_be_read() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let memory = component(Arc::clone(&policy), ActorId::User(UserId::new()));
        let (session, other_session) = (SessionId::new(), SessionId::new());
        let mut sessions = Sessions::under(policy).await;
        sessions.add(session, SessionPrivacy::default()).await.add(other_session, SessionPrivacy::default()).await;
        let (team, public, elsewhere) = (channel(session, Confidentiality::Internal), channel(session, Confidentiality::Public), channel(other_session, Confidentiality::Internal));

        // A turn in an internal channel remembers something it was told
        // there ...
        let mut state = AgentChannelState::default();
        {
            state.begin_turn(in_channel(&team), Default::default(), team.1.clone(), Vec::new(), Default::default());
            let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&team, &memory));
            turn.see(&team.1);
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), &mut turn).await.unwrap();
            assert_eq!(result.value["id"], 1);
            assert!(turn.agent_state().current().memory.current().is_empty(), "not before the turn completes");
            turn.commit(false).await;
        }

        // ... which another actor of the agent recalls in a channel that may
        // read that one, and not where it may not be known.
        let same_session = recall(&memory, &sessions, &handle, agent, channel(session, Confidentiality::Confidential)).await;
        assert!(same_session.text.contains("#1: Kaya likes tea."), "{}", same_session.text);
        assert_eq!(same_session.knowledge.len(), 1, "recorded as knowledge read");
        assert!(recall(&memory, &sessions, &handle, agent, public).await.text.is_empty(), "a public channel may not read an internal one");
        assert!(recall(&memory, &sessions, &handle, agent, elsewhere.clone()).await.text.is_empty(), "another session, private");

        // Forgotten only where it may be recalled.
        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&elsewhere), Default::default(), elsewhere.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&elsewhere, &memory));
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.is_err());
        state.begin_turn(in_channel(&team), Default::default(), team.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&team, &memory));
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.unwrap();
        turn.commit(false).await;
        assert!(handle.current().memory.current().is_empty());
    }

    #[tokio::test]
    async fn an_entry_remembered_and_forgotten_in_one_turn_is_gone_once_it_completes() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let memory = component(Arc::clone(&policy), ActorId::User(UserId::new()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(policy).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let team = channel(session, Confidentiality::Internal);

        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&team), Default::default(), team.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&team, &memory));
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "draft" }), &mut turn).await.unwrap();
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.expect("remembered in this turn");
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), &mut turn).await.is_err(), "forgotten already");
        let forgotten = memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), &mut turn).await;
        assert!(matches!(forgotten, Err(ToolError::InvalidParameter(_))), "a forgotten entry is not reclassified");
        turn.commit(false).await;
        assert!(handle.current().memory.current().is_empty());
    }

    #[tokio::test]
    async fn a_turn_in_no_session_recalls_nothing_and_remembers_as_private() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let memory = component(Arc::new(OwnershipSecurityManager::new()), ActorId::User(UserId::new()));
        let at = channel(SessionId::new(), Confidentiality::Internal);
        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun");
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "alone" }), &mut turn).await.unwrap();
        turn.commit(false).await;
        assert_eq!(handle.current().memory.current()[0].origin.session, SessionPrivacy { id: at.0.0, ..SessionPrivacy::default() }, "its session, private: nobody agreed to anything");
        state.begin_turn(in_channel(&at), Default::default(), at.1.clone(), Vec::new(), Default::default());
        let turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun");
        assert!(memory.get_prompt(&turn.work()).await.unwrap().text.is_empty(), "no session to recall in");
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
        let mut sessions = Sessions::under(Arc::clone(&policy)).await;
        sessions.add(session, SessionPrivacy::default()).await;
        // Public, so that the built-in rule would let every channel read it.
        let (origin, reader) = (channel(session, Confidentiality::Public), channel(session, Confidentiality::Internal));
        remember(&component(Arc::clone(&policy), owner), &sessions, &handle, agent, origin.clone(), "Kaya likes tea.").await;

        let asked_by_owner = recall(&component(Arc::clone(&policy), owner), &sessions, &handle, agent, reader.clone()).await;
        assert!(asked_by_owner.text.contains("Kaya likes tea."), "{}", asked_by_owner.text);
        let asked_by_stranger = recall(&component(Arc::clone(&policy), stranger), &sessions, &handle, agent, reader).await;
        assert!(asked_by_stranger.text.is_empty(), "the policy refuses, whatever the levels say: {}", asked_by_stranger.text);
        let in_its_own_channel = recall(&component(policy, stranger), &sessions, &handle, agent, origin).await;
        assert!(in_its_own_channel.text.contains("Kaya likes tea."), "a channel always reads itself");
    }

    #[tokio::test]
    async fn what_a_turn_remembers_is_classified_as_what_it_has_seen() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let memory = component(Arc::clone(&policy), ActorId::User(UserId::new()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(policy).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let (confidential, public) = (channel(session, Confidentiality::Confidential), channel(session, Confidentiality::Public));

        // A turn in a confidential channel that has seen only what is
        // internal remembers it as internal (issue 168) ...
        let mut state = AgentChannelState::default();
        {
            state.begin_turn(in_channel(&confidential), Default::default(), confidential.1.clone(), Vec::new(), Default::default());
            let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&confidential, &memory));
            turn.see(&Classification::general(Confidentiality::Internal));
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), &mut turn).await.unwrap();
            assert_eq!(result.classification, Some(Classification::general(Confidentiality::Internal)), "its answer as the turn");
            turn.commit(false).await;
        }
        let remembered = handle.current().memory.current();
        assert_eq!(remembered[0].classification, Classification::general(Confidentiality::Internal));

        // ... so that an internal channel recalls it, a public one not.
        assert!(recall(&memory, &sessions, &handle, agent, channel(session, Confidentiality::Internal)).await.text.contains("Kaya likes tea."));
        assert!(recall(&memory, &sessions, &handle, agent, public).await.text.is_empty());

        // What is recalled is recorded with its classification, and is
        // part of what the turn has seen.
        let prompt = recall(&memory, &sessions, &handle, agent, confidential).await;
        assert_eq!(prompt.knowledge[0].classification, Some(Classification::general(Confidentiality::Internal)));
        assert_eq!(prompt.classification, Some(Classification::PUBLIC), "nothing but what it recalls");
    }

    #[tokio::test]
    async fn an_entry_released_to_another_compartment_is_recalled_there() {
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules};
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_rules(ConfidentialityRules::new(compartments)));
        // As the agent's components are given it: its owner's, called by the agent.
        let memory = AgentMemoryComponent {
            security_manager: Arc::clone(&policy),
            security_context: SecurityContext::new(ActorId::User(UserId::new()), Default::default(), Default::default()).next(ActorId::Agent(agent)),
        };
        let session = SessionId::new();
        let mut sessions = Sessions::under(policy).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let in_compartments = |names: &[&str]| -> At {
            ((session, ChannelId::new()), Classification::new(Confidentiality::Internal, names.iter().map(|n| Compartment::new(*n))).unwrap())
        };
        let (hr, finance) = (in_compartments(&["hr"]), in_compartments(&["finance"]));
        remember(&memory, &sessions, &handle, agent, hr.clone(), "The budget is tight.").await;
        assert!(recall(&memory, &sessions, &handle, agent, finance.clone()).await.text.is_empty(), "HR's, not finance's");

        let mut state = AgentChannelState::default();
        state.begin_turn(in_channel(&hr), Default::default(), hr.1.clone(), Vec::new(), Default::default());
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle)).expect("a turn was begun").with_views(sessions.views_in(&hr, &memory));
        let above = serde_json::json!({ "id": 1, "level": "confidential", "compartments": ["hr"] });
        assert!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &above, &mut turn).await.is_err(), "never above the channel");
        let unknown = serde_json::json!({ "id": 7, "level": "internal", "compartments": ["hr"] });
        assert!(matches!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &unknown, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        let released = serde_json::json!({ "id": 1, "level": "internal", "compartments": ["hr", "finance"] });
        memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &released, &mut turn).await.expect("the agent releases what it remembers");
        turn.commit(false).await;

        let there = recall(&memory, &sessions, &handle, agent, finance).await;
        assert!(there.text.contains("The budget is tight."), "released to finance: {}", there.text);
        assert!(recall(&memory, &sessions, &handle, agent, hr).await.text.contains("The budget is tight."), "and still HR's");
    }

    #[tokio::test]
    async fn an_entry_is_recalled_in_another_session_where_its_session_agreed_then_and_now() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let (alice, bob) = (UserId::new(), UserId::new());
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let memory = component(Arc::clone(&policy), ActorId::User(alice));
        let (hers, other, bobs) = (SessionId::new(), SessionId::new(), SessionId::new());
        let mut sessions = Sessions::under(policy).await;
        sessions.add(hers, session_of(PrivacyConfig::SameOwner, alice)).await
            .add(other, session_of(PrivacyConfig::Private, alice)).await
            .add(bobs, session_of(PrivacyConfig::Private, bob)).await;

        // Remembered in a session of hers that lets her other sessions read it ...
        let origin = channel(hers, Confidentiality::Internal);
        remember(&memory, &sessions, &handle, agent, origin.clone(), "Kaya likes tea.").await;
        let remembered = handle.current().memory.current();
        assert_eq!(remembered[0].origin.session.policy, PrivacyConfig::SameOwner, "the entry keeps its session's privacy");

        // ... and recalled in another of hers, while the session still agrees.
        let elsewhere = channel(other, Confidentiality::Internal);
        let recalled = recall(&memory, &sessions, &handle, agent, elsewhere.clone()).await;
        assert!(recalled.text.contains("Kaya likes tea."), "{}", recalled.text);
        assert_eq!(recalled.knowledge.len(), 1);
        assert_eq!((recalled.knowledge[0].session, recalled.knowledge[0].channel), origin.0, "recorded with the channel it came from, of its session");

        sessions.set(hers, Some(session_of(PrivacyConfig::Private, alice))).await;
        assert!(recall(&memory, &sessions, &handle, agent, elsewhere.clone()).await.text.is_empty(), "no longer, once the session is private");
        sessions.set(hers, None).await;
        assert!(recall(&memory, &sessions, &handle, agent, elsewhere.clone()).await.text.contains("Kaya likes tea."), "of a session that is gone, what it agreed to then");

        let in_bobs = recall(&memory, &sessions, &handle, agent, channel(bobs, Confidentiality::Internal)).await;
        assert!(in_bobs.text.is_empty(), "not in someone else's session");

        // What was said while the session was private stays there, even
        // after it lets others read.
        sessions.set(hers, Some(session_of(PrivacyConfig::Private, alice))).await;
        remember(&memory, &sessions, &handle, agent, channel(hers, Confidentiality::Internal), "Kaya is moving.").await;
        sessions.set(hers, Some(session_of(PrivacyConfig::SameOwner, alice))).await;
        let recalled = recall(&memory, &sessions, &handle, agent, channel(other, Confidentiality::Internal)).await;
        assert!(recalled.text.contains("Kaya likes tea.") && !recalled.text.contains("Kaya is moving."), "{}", recalled.text);
    }
}
