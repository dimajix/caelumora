//! The `agent_memory` component (issue 62, step 3): an agent remembers what
//! it should know across its channels and sessions - kept in its
//! `AgentState`, not in one channel.
//!
//! It translates the tools `remember`, `update_memory` and `forget` and
//! renders the prompt part; the logic is `crate::memory::AgentMemory`. Changes are applied
//! to the agent's state as it is when the turn completes
//! (`Turn::commit`), so that actors of the same agent in
//! other channels and sessions neither lose nor overwrite them. The prompt
//! part shows the entries that may be recalled in the current channel: those
//! written in a channel it may read, as the policy decides when the
//! component asks in the agent's name (`ConfidentialityQuestions::may_copy_information`,
//! issue 131). Under the ownership policy that is the flow rule of issue
//! 133 - the entry's channel's classification must flow into this one's -
//! within the session, and beyond it only where the entry's session permits
//! it, then and now (issue 134). Only those can be
//! updated or forgotten here. Its tools are
//! `ToolVisibility::HIDDEN`: like private notes, memory is the agent's
//! own, seen by its owner in the context panel only. Its entries are
//! numbered differently from notes, on purpose: see `AgentMemory::remember`.

use crate::prompt::NAMESPACE;
use crate::components::notes::NOTE_PRESENTATION;
use crate::memory::AgentMemory;
use crate::turn::Turn;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry, ComponentKind};
use crate::prompt::{ContextError, Prompt, PromptPosition, PromptProvider, texts_of, writing_for_the_model};
use caelumora_model::prompt_catalogue::{context, PromptError, PromptTexts};
use crate::situation::Situation;
use crate::tool::{described, classification_argument, id_argument, reclassify_parameters, tool_text, text_argument, ToolError, ToolOutput, ToolProvider, ToolResult};
use async_trait::async_trait;
use caelumora_model::question::Answers;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy};
use caelumora_model::memory::{Memory, MemoryId};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::security::AccessPolicy;
use caelumora_model::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::sync::Arc;
use std::fmt;
use std::fmt::Debug;

pub const AGENT_MEMORY_KIND: &str = "agent_memory";

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const UPDATE_MEMORY_TOOL_NAME: &str = "update_memory";
pub const FORGET_TOOL_NAME: &str = "forget";
pub const RECLASSIFY_MEMORY_TOOL_NAME: &str = "reclassify_memory";

/// The text of its prompt part in the prompt catalogue.
const TEXT: &str = "agents.agent_memory";

pub struct AgentMemoryComponent {
    /// The policy that decides what may be recalled, asked in the context
    /// of the work it serves - the agent's, with its owner's rights (issues
    /// 131, 255).
    access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where (issue 304): what it recalls, how it reclassifies.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// What its tool set is for and its tools, as the prompt catalogue
    /// words them.
    description: String,
    tools: Vec<Tool>,
}

impl Debug for AgentMemoryComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentMemoryComponent").finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolProvider for AgentMemoryComponent {
    fn description(&self) -> String {
        self.description.clone()
    }

    fn tools(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    async fn execute(&self, name: &str, params: &Value, _answers: Option<&Answers>, turn: &mut Turn) -> ToolResult<ToolOutput> {
        let value = match name {
            REMEMBER_TOOL_NAME => self.execute_remember(params, turn).await,
            UPDATE_MEMORY_TOOL_NAME => self.execute_update(params, turn).await,
            FORGET_TOOL_NAME => self.execute_forget(params, turn).await,
            RECLASSIFY_MEMORY_TOOL_NAME => self.execute_reclassify(params, turn).await,
            _ => Err(ToolError::UnknownTool(name.to_string())),
        }?;
        // What these answer tells only what the model asked for: classified
        // as the turn (issue 168).
        Ok(ToolOutput::classified(value, turn.situation().label()))
    }
}

impl AgentMemoryComponent {
    /// The component, deciding by `access_policy` and `confidentiality`;
    /// its texts from `prompts`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub fn new(access_policy: Arc<dyn AccessPolicy>, confidentiality: Arc<dyn ConfidentialityPolicy>, prompts: PromptTexts) -> Result<Self, PromptError> {
        let tools = vec![
            described(&prompts, NAMESPACE, AGENT_MEMORY_KIND, Tool {
                name: REMEMBER_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("text", ToolParameters::string())],
                    ["text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, AGENT_MEMORY_KIND, Tool {
                name: UPDATE_MEMORY_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number()), ("text", ToolParameters::string())],
                    ["id", "text"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, AGENT_MEMORY_KIND, Tool {
                name: FORGET_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: ToolParameters::object(
                    [("id", ToolParameters::number())],
                    ["id"],
                ),
                requires_approval: false,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
            described(&prompts, NAMESPACE, AGENT_MEMORY_KIND, Tool {
                name: RECLASSIFY_MEMORY_TOOL_NAME.to_string(),
                description: String::new(),
                parameters: reclassify_parameters(&prompts)?,
                requires_approval: true,
                visibility: ToolVisibility::HIDDEN,
                classification: None,
                presentation: NOTE_PRESENTATION,
            }, ())?,
        ];
        let description = tool_text(&prompts, NAMESPACE, AGENT_MEMORY_KIND, "description", ())?;
        Ok(Self { access_policy, confidentiality, prompts, description, tools })
    }

    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    async fn execute_remember(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = AgentMemory::remember(turn, text_argument(params)?).await;
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `update_memory(id, text)`: changes the text of an entry that may be
    /// recalled here.
    async fn execute_update(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        let text = text_argument(params)?;
        if AgentMemory::update(turn, id, text, self.confidentiality.as_ref()).await {
            Ok(serde_json::json!({ "status": "updated", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you remember nothing numbered {id} here")))
        }
    }

    /// `reclassify_memory(id, level, compartments?)`: reclassifies an entry
    /// that may be recalled here, as the policy allows.
    async fn execute_reclassify(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        let classification = classification_argument(params)?;
        AgentMemory::reclassify(turn, id, classification.clone(), self.access_policy.as_ref(), self.confidentiality.as_ref()).await?;
        Ok(serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }))
    }

    /// `forget(id)`: forgets an entry that may be recalled here.
    async fn execute_forget(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        if AgentMemory::forget(turn, id, self.confidentiality.as_ref()).await {
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

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let recalled = AgentMemory::recalled(situation, self.confidentiality.as_ref()).await;
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        // Once for every channel the entries come from, so that knowledge
        // from another session can be told (issue 134).
        let source = KnowledgeSource::Memory { agent: situation.agent_id(), digest: AgentMemory::digest(situation) };
        let mut knowledge: Vec<KnowledgeRef> = Vec::new();
        for memory in &recalled {
            if !knowledge.iter().any(|k| k.channel == memory.origin.channel) {
                let from_there = recalled.iter().filter(|m| m.origin.channel == memory.origin.channel).map(|m| &m.classification);
                knowledge.push(KnowledgeRef { channel: memory.origin.channel, session: memory.origin.session.id, source, classification: Classification::join_all(from_there) });
            }
        }
        Ok(Prompt::of_knowledge(render(&self.prompts, &recalled)?, knowledge))
    }
}

/// The prompt part of `prompts` for what the agent `recalled` here.
fn render(prompts: &PromptTexts, recalled: &[Memory]) -> Result<String, PromptError> {
    let recalled: Vec<_> = recalled.iter().map(|m| context! { id => m.id.to_string(), text => m.text }).collect();
    prompts.render(TEXT, context! { recalled, remember => REMEMBER_TOOL_NAME, update => UPDATE_MEMORY_TOOL_NAME, forget => FORGET_TOOL_NAME })
}

pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        writing_for_the_model(ComponentKind::new(AGENT_MEMORY_KIND).provides::<dyn ToolProvider>().provides::<dyn PromptProvider>().uses::<dyn AccessPolicy>().uses::<dyn ConfidentialityPolicy>()),
        |_descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let component = Arc::new(AgentMemoryComponent::new(
                Arc::clone(context.service::<dyn AccessPolicy>()?),
                Arc::clone(context.service::<dyn ConfidentialityPolicy>()?),
                texts,
            )?);
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
    use caelumora_model::confidentiality::{ConfidentialityRules, Compartments, Reading, Place};
    use caelumora_runtime::transport::Discard;
    use crate::testing::{self, TestAgents};
    use caelumora_runtime::testing::TestRuntime;
    use caelumora_runtime::agent::state::{AgentStateHandle, AgentStates};
    use caelumora_runtime::runtime::{ActorRuntime, RuntimeManager};
    use caelumora_runtime::session::SessionLoop;
    use caelumora_runtime::session::testing::{actor_runtime_in, main_channel, running_channel};
    use crate::testing::detached_actor_runtime;
    use crate::tool::ToolError;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_model::agent::{AgentChannelState, AgentId};
    use caelumora_model::channel::Participant;
    use caelumora_model::confidentiality::{Classification, Confidentiality};
    use caelumora_model::privacy::{PrivacyConfig, SessionPrivacy};
    use caelumora_model::security::{OwnershipPolicy, SecurityAttributes, SecurityContext};
    use caelumora_model::session::{ChannelId, Session, SessionId};
    use caelumora_model::store::StateStore;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::user::UserId;
    use std::collections::HashMap;
    use caelumora_model::role::{RoleId, Roles};
    use caelumora_model::value::name::Name;
    use chrono::Utc;
    use std::ops::Deref;
    use caelumora_model::work::Work;

    /// A channel of the tests: where it is - its session and itself - and
    /// how it is classified.
    type At = ((SessionId, ChannelId), Classification);

    /// The context of an envelope routed in the channel `at`.
    fn in_channel(at: &At) -> EventContext {
        EventContext::new(at.0.0, at.0.1, ActorId::None, String::new())
    }

    fn channel(session: SessionId, level: Confidentiality) -> At {
        ((session, ChannelId::new()), Classification::general(level))
    }

    /// The component of an agent, asking `access_policy`, and the agent's owner:
    /// whom its turns act for (issue 255).
    struct Memory {
        component: AgentMemoryComponent,
        owner: ActorId,
    }

    impl Memory {
        /// Begins a turn of `agent` in `state`, in channel `at`, as its actor
        /// begins one: the agent acting with its owner's rights.
        fn begin(&self, state: &mut AgentChannelState, at: &At, agent: AgentId) {
            let context = SecurityContext::new(self.owner, at.0.0, at.0.1).next(ActorId::Agent(agent), self.owner, self.owner);
            state.begin_turn(Work::new(in_channel(at), None, context, at.1.clone(), Default::default()));
        }
    }

    impl Deref for Memory {
        type Target = AgentMemoryComponent;
        fn deref(&self) -> &AgentMemoryComponent {
            &self.component
        }
    }

    /// The component of an agent owned by `owner`, asking `access_policy`, the
    /// flow decided by the rules of no compartments.
    fn component(access_policy: Arc<dyn AccessPolicy>, owner: ActorId) -> Memory {
        Memory { component: AgentMemoryComponent::new(access_policy, Arc::new(ConfidentialityRules::default()), testing::prompts()).unwrap(), owner }
    }

    /// The component of an agent owned by `owner`, the flow decided by
    /// `confidentiality` (issue 304).
    fn component_under(confidentiality: Arc<dyn ConfidentialityPolicy>, owner: ActorId) -> Memory {
        Memory { component: AgentMemoryComponent::new(Arc::new(OwnershipPolicy::new()), confidentiality, testing::prompts()).unwrap(), owner }
    }

    async fn handle(agent: AgentId) -> Arc<AgentStateHandle> {
        let store: Arc<dyn StateStore> = Arc::new(MemoryStore::new());
        AgentStates::new(store, Arc::new(())).get(agent, Default::default()).await.unwrap()
    }

    /// The sessions of a test, as a turn reaches them: stored in the
    /// runtime's store, and running with their main channel and every
    /// channel a turn works in, through which it reaches its session and
    /// reads its privacy from its loop.
    struct Sessions {
        runtime: Arc<RuntimeManager>,
        loops: HashMap<SessionId, Arc<SessionLoop>>,
    }

    impl Sessions {
        async fn under(access_policy: Arc<dyn AccessPolicy>) -> Self {
            let runtime = TestRuntime { access_policy, plugins: vec![&TestAgents], ..Default::default() }.build();
            Self { runtime, loops: HashMap::new() }
        }

        /// Session `id` as privacy sees it as `privacy`, stored and running.
        async fn add(&mut self, id: SessionId, privacy: SessionPrivacy) -> &mut Self {
            let session = session_with(id, &privacy);
            self.runtime.entity_stores().sessions.create(&SecurityContext::system(), &session).await.unwrap();
            self.loops.insert(id, SessionLoop::new(session.clone(), vec![main_channel(&session)], Arc::clone(&self.runtime)).await.unwrap());
            self
        }

        /// Session `id` lets others read as `privacy` says from now on -
        /// stored, as a change to a running session is stored first - or
        /// is gone (`None`).
        async fn set(&mut self, id: SessionId, privacy: Option<SessionPrivacy>) {
            let sessions = &self.runtime.entity_stores().sessions;
            match privacy {
                Some(privacy) => {
                    let session = session_with(id, &privacy);
                    if sessions.update(&SecurityContext::system(), &session).await.is_err() {
                        // Gone meanwhile: back again.
                        sessions.create(&SecurityContext::system(), &session).await.unwrap();
                    }
                    self.loops.insert(id, SessionLoop::new(session.clone(), vec![main_channel(&session)], Arc::clone(&self.runtime)).await.unwrap());
                }
                None => {
                    sessions.delete(&SecurityContext::system(), id).await.unwrap();
                    sessions.purge(&SecurityContext::system(), id).await.unwrap();
                    self.loops.remove(&id);
                }
            }
        }

        /// The runtime a turn in channel `at` acts through: in that
        /// channel, run by its session.
        async fn runtime_in(&self, at: &At) -> ActorRuntime {
            let session = self.loops.get(&at.0.0).expect("a session of the test");
            let channel = running_channel(session, at.0.1, at.1.clone()).await;
            actor_runtime_in(&channel, ActorId::None)
        }
    }

    /// A session `id` that privacy sees as `privacy`.
    fn session_with(id: SessionId, privacy: &SessionPrivacy) -> Session {
        let now = Utc::now();
        Session {
            summary: None, briefing: None, language: None, time_zone: None,
            approval_rules: Vec::new(),
            workspace: None,
            id,
            name: Name::new("s").unwrap(),
            participants: privacy.people.iter().map(|user| Participant::User(*user)).collect(),
            channels: vec![ChannelId::new()],
            commands: Default::default(),
            privacy: privacy.policy,
            created_at: now,
            updated_at: now,
            security_attributes: SecurityAttributes::owned_by(privacy.owner),
            deleted: None,
        }
    }

    fn session_of(policy: PrivacyConfig, owner: UserId) -> SessionPrivacy {
        SessionPrivacy { id: SessionId::new(), policy, owner: ActorId::User(owner), people: [owner].into_iter().collect() }
    }

    /// Remembers `text` in a turn in channel `at`, and completes the turn.
    /// Remembers `text` in a turn in channel `at` that was told something
    /// classified as the channel.
    async fn remember(memory: &Memory, sessions: &Sessions, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At, text: &str) {
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(handle), sessions.runtime_in(&at).await).expect("a turn was begun");
        turn.see(&at.1);
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": text }), None, &mut turn).await.unwrap();
        turn.commit(false, &Discard).await;
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(memory: &Memory, sessions: &Sessions, handle: &Arc<AgentStateHandle>, agent: AgentId, at: At) -> Prompt {
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at, agent);
        let turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(handle), sessions.runtime_in(&at).await).expect("a turn was begun");
        memory.get_prompt(&turn.situation()).await.unwrap()
    }

    #[tokio::test]
    async fn what_is_remembered_is_recalled_wherever_its_origin_may_be_read() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let memory = component(Arc::clone(&access_policy), ActorId::User(UserId::new()));
        let (session, other_session) = (SessionId::new(), SessionId::new());
        let mut sessions = Sessions::under(access_policy).await;
        sessions.add(session, SessionPrivacy::default()).await.add(other_session, SessionPrivacy::default()).await;
        let (team, public, elsewhere) = (channel(session, Confidentiality::Internal), channel(session, Confidentiality::Public), channel(other_session, Confidentiality::Internal));

        // A turn in an internal channel remembers something it was told
        // there ...
        let mut state = AgentChannelState::default();
        {
            memory.begin(&mut state, &team, agent);
            let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&team).await).expect("a turn was begun");
            turn.see(&team.1);
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), None, &mut turn).await.unwrap();
            assert_eq!(result.value["id"], 1);
            assert!(turn.situation().agent_state().current().memory.current().is_empty(), "not before the turn completes");
            turn.commit(false, &Discard).await;
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
        memory.begin(&mut state, &elsewhere, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&elsewhere).await).expect("a turn was begun");
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.is_err());
        memory.begin(&mut state, &team, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&team).await).expect("a turn was begun");
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.unwrap();
        turn.commit(false, &Discard).await;
        assert!(handle.current().memory.current().is_empty());
    }

    #[tokio::test]
    async fn an_entry_is_updated_only_where_it_is_recalled_and_stands_where_it_was_updated() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let memory = component(Arc::clone(&access_policy), ActorId::User(UserId::new()));
        let (session, other_session) = (SessionId::new(), SessionId::new());
        let mut sessions = Sessions::under(access_policy).await;
        sessions.add(session, SessionPrivacy::default()).await.add(other_session, SessionPrivacy::default()).await;
        let (team, confidential, elsewhere) = (channel(session, Confidentiality::Internal), channel(session, Confidentiality::Confidential), channel(other_session, Confidentiality::Internal));
        remember(&memory, &sessions, &handle, agent, team.clone(), "Kaya likes tea.").await;

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &elsewhere, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&elsewhere).await).expect("a turn was begun");
        let there = memory.execute(UPDATE_MEMORY_TOOL_NAME, &serde_json::json!({ "id": 1, "text": "Kaya likes coffee." }), None, &mut turn).await;
        assert!(matches!(there, Err(ToolError::InvalidParameter(_))), "not where it is not recalled");

        // Updated in a turn that has seen what is confidential ...
        memory.begin(&mut state, &confidential, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&confidential).await).expect("a turn was begun");
        turn.see(&confidential.1);
        let updated = memory.execute(UPDATE_MEMORY_TOOL_NAME, &serde_json::json!({ "id": 1, "text": "Kaya likes coffee." }), None, &mut turn).await.unwrap();
        assert_eq!(updated.value, serde_json::json!({ "status": "updated", "id": 1 }));
        turn.commit(false, &Discard).await;
        let remembered = handle.current().memory.current();
        assert_eq!((remembered[0].id, remembered[0].text.as_str(), remembered[0].origin.channel), (MemoryId(1), "Kaya likes coffee.", confidential.0.1), "its number kept, written where it was updated");
        assert_eq!(remembered[0].classification, confidential.1);

        // ... is recalled where that may be known, and no longer where only the old text might.
        assert!(recall(&memory, &sessions, &handle, agent, confidential).await.text.contains("#1: Kaya likes coffee."));
        assert!(recall(&memory, &sessions, &handle, agent, team).await.text.is_empty());
    }

    #[tokio::test]
    async fn an_entry_remembered_and_forgotten_in_one_turn_is_gone_once_it_completes() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let memory = component(Arc::clone(&access_policy), ActorId::User(UserId::new()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(access_policy).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let team = channel(session, Confidentiality::Internal);

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &team, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&team).await).expect("a turn was begun");
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "draft" }), None, &mut turn).await.unwrap();
        memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.expect("remembered in this turn");
        assert!(memory.execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn).await.is_err(), "forgotten already");
        let forgotten = memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &serde_json::json!({ "id": 1, "level": "public" }), None, &mut turn).await;
        assert!(matches!(forgotten, Err(ToolError::InvalidParameter(_))), "a forgotten entry is not reclassified");
        turn.commit(false, &Discard).await;
        assert!(handle.current().memory.current().is_empty());
    }

    #[tokio::test]
    async fn a_turn_in_no_session_recalls_nothing_and_remembers_as_private() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let memory = component(Arc::new(OwnershipPolicy::new()), ActorId::User(UserId::new()));
        let at = channel(SessionId::new(), Confidentiality::Internal);
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), detached_actor_runtime()).expect("a turn was begun");
        memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "alone" }), None, &mut turn).await.unwrap();
        turn.commit(false, &Discard).await;
        assert_eq!(handle.current().memory.current()[0].origin.session, SessionPrivacy { id: at.0.0, ..SessionPrivacy::default() }, "its session, private: nobody agreed to anything");
        memory.begin(&mut state, &at, agent);
        let turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), detached_actor_runtime()).expect("a turn was begun");
        assert!(memory.get_prompt(&turn.situation()).await.unwrap().text.is_empty(), "no session to recall in");
    }

    /// Lets channels read each other only in the name of `owner` - and asks
    /// nothing else: what an agent remembered is its own, recalled whoever
    /// may read its channel now (issue 264).
    #[derive(Debug, Default)]
    struct OnlyFor(Option<ActorId>, Compartments);
    impl ConfidentialityPolicy for OnlyFor {
        fn may_publish(&self, _: &SecurityContext, _: &Classification, _: &Classification) -> bool { true }
        fn copyable(&self, context: &SecurityContext, _: ChannelId, _: &SessionPrivacy, _: &Place) -> Reading {
            match Some(context.principal_id) == self.0 {
                true => Reading::Whole,
                false => Reading::Nothing,
            }
        }
        fn cleared(&self, _: &SecurityContext, _: ActorId, _: &[RoleId], _: &Classification) -> bool { true }
        fn compartments(&self) -> &Compartments { &self.1 }
        fn roles(&self) -> Option<&Roles> { None }
    }

    #[tokio::test]
    async fn the_policy_decides_what_is_recalled_asked_in_the_agents_name() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let (owner, stranger) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let policy: Arc<dyn ConfidentialityPolicy> = Arc::new(OnlyFor(Some(owner), Default::default()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(Arc::new(OwnershipPolicy::new())).await;
        sessions.add(session, SessionPrivacy::default()).await;
        // Public, so that the built-in rule would let every channel read it.
        let (origin, reader) = (channel(session, Confidentiality::Public), channel(session, Confidentiality::Internal));
        remember(&component_under(Arc::clone(&policy), owner), &sessions, &handle, agent, origin.clone(), "Kaya likes tea.").await;

        let asked_by_owner = recall(&component_under(Arc::clone(&policy), owner), &sessions, &handle, agent, reader.clone()).await;
        assert!(asked_by_owner.text.contains("Kaya likes tea."), "{}", asked_by_owner.text);
        let asked_by_stranger = recall(&component_under(Arc::clone(&policy), stranger), &sessions, &handle, agent, reader).await;
        assert!(asked_by_stranger.text.is_empty(), "the policy refuses, whatever the levels say: {}", asked_by_stranger.text);
        let in_its_own_channel = recall(&component_under(policy, stranger), &sessions, &handle, agent, origin).await;
        assert!(in_its_own_channel.text.contains("Kaya likes tea."), "a channel always reads itself");
    }

    #[tokio::test]
    async fn what_a_turn_remembers_is_classified_as_what_it_has_seen() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let memory = component(Arc::clone(&access_policy), ActorId::User(UserId::new()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(access_policy).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let (confidential, public) = (channel(session, Confidentiality::Confidential), channel(session, Confidentiality::Public));

        // A turn in a confidential channel that has seen only what is
        // internal remembers it as internal (issue 168) ...
        let mut state = AgentChannelState::default();
        {
            memory.begin(&mut state, &confidential, agent);
            let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&confidential).await).expect("a turn was begun");
            turn.see(&Classification::general(Confidentiality::Internal));
            let result = memory.execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "Kaya likes tea." }), None, &mut turn).await.unwrap();
            assert_eq!(result.classification, Some(Classification::general(Confidentiality::Internal)), "its answer as the turn");
            turn.commit(false, &Discard).await;
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
        use caelumora_model::confidentiality::Compartment;
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let memory = component_under(Arc::new(ConfidentialityRules::new(compartments)), ActorId::User(UserId::new()));
        let session = SessionId::new();
        let mut sessions = Sessions::under(Arc::new(OwnershipPolicy::new())).await;
        sessions.add(session, SessionPrivacy::default()).await;
        let in_compartments = |names: &[&str]| -> At {
            ((session, ChannelId::new()), Classification::new(Confidentiality::Internal, names.iter().map(|n| Compartment::new(*n))).unwrap())
        };
        let (hr, finance) = (in_compartments(&["hr"]), in_compartments(&["finance"]));
        remember(&memory, &sessions, &handle, agent, hr.clone(), "The budget is tight.").await;
        assert!(recall(&memory, &sessions, &handle, agent, finance.clone()).await.text.is_empty(), "HR's, not finance's");

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &hr, agent);
        let mut turn = Turn::of(agent, "agent".to_string(), &mut state, Arc::clone(&handle), sessions.runtime_in(&hr).await).expect("a turn was begun");
        let above = serde_json::json!({ "id": 1, "level": "confidential", "compartments": ["hr"] });
        assert!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &above, None, &mut turn).await.is_err(), "never above the channel");
        let unknown = serde_json::json!({ "id": 7, "level": "internal", "compartments": ["hr"] });
        assert!(matches!(memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &unknown, None, &mut turn).await, Err(ToolError::InvalidParameter(_))));
        let released = serde_json::json!({ "id": 1, "level": "internal", "compartments": ["hr", "finance"] });
        memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &released, None, &mut turn).await.expect("the agent releases what it remembers");
        turn.commit(false, &Discard).await;

        let there = recall(&memory, &sessions, &handle, agent, finance).await;
        assert!(there.text.contains("The budget is tight."), "released to finance: {}", there.text);
        assert!(recall(&memory, &sessions, &handle, agent, hr).await.text.contains("The budget is tight."), "and still HR's");
    }

    #[tokio::test]
    async fn an_entry_is_recalled_in_another_session_where_its_session_agreed_then_and_now() {
        let agent = AgentId::new();
        let handle = handle(agent).await;
        let (alice, bob) = (UserId::new(), UserId::new());
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::new());
        let memory = component(Arc::clone(&access_policy), ActorId::User(alice));
        let (hers, other, bobs) = (SessionId::new(), SessionId::new(), SessionId::new());
        let mut sessions = Sessions::under(access_policy).await;
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

    #[test]
    fn every_entry_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = AgentMemoryComponent::new(Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()), testing::prompts()).unwrap().tools();
        assert!(tools.iter().all(|t| (t.presentation, t.visibility) == (NOTE_PRESENTATION, ToolVisibility::HIDDEN)));
    }
}
