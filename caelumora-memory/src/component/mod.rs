//! The `agent_memory` component (issues 62, 518): an agent remembers what it should know across
//! its channels and sessions - kept by the plugin for the agent, not in one channel.
//!
//! It translates the tools `remember`, `update_memory` and `forget` and renders the prompt part;
//! the logic is `turn::TurnMemory`. Changes are staged on the turn and committed by the plugin to
//! the agent's memory as it is when the turn completes, so that actors of the same agent in other
//! channels and sessions neither lose nor overwrite them. The prompt part shows the entries that
//! may be recalled in the current channel, which the plugin hands the work through its
//! `MemoryReader` (issue 131). Under the ownership policy that is the flow rule of issue 133 - the
//! entry's channel's classification must flow into this one's - within the session, and beyond it
//! only where the entry's session permits it, then and now (issue 134). Only those can be updated
//! or forgotten here. Its tools are `ToolVisibility::HIDDEN`: like private notes, memory is the
//! agent's own, seen by its owner in the context panel only. Its entries are numbered differently
//! from notes, on purpose: see `TurnMemory::remember`.

mod turn;

use crate::extension::{MemoryReader, MemoryWriter};
use crate::memory::{AgentMemory, AgentMemoryId, Memory, MemoryId};
use crate::NAMESPACE;
use async_trait::async_trait;
use caelumora_agents::components::notes::NOTE_PRESENTATION;
use caelumora_agents::prompt::{
    texts_of, writing_for_the_model, ContextError, PositionParams, Prompt, PromptPosition,
    PromptProvider,
};
use caelumora_agents::situation::Situation;
use caelumora_agents::tool::{
    classification_argument, described, id_argument, reclassify_parameters, text_argument,
    tool_text, ToolError, ToolOutput, ToolProvider, ToolResult,
};
use caelumora_agents::turn::Turn;
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::{KnowledgeRef, KnowledgeSource};
use caelumora_domain::prompt_catalogue::{context, PromptError, PromptTexts};
use caelumora_domain::question::Answers;
use caelumora_domain::tool::{Tool, ToolParameters, ToolVisibility};
use serde_json::Value;
use std::fmt;
use std::fmt::Debug;
use std::sync::Arc;
use turn::TurnMemory;

pub const AGENT_MEMORY_KIND: &str = "agent_memory";

pub const REMEMBER_TOOL_NAME: &str = "remember";
pub const UPDATE_MEMORY_TOOL_NAME: &str = "update_memory";
pub const FORGET_TOOL_NAME: &str = "forget";
pub const RECLASSIFY_MEMORY_TOOL_NAME: &str = "reclassify_memory";

/// The text of its prompt part in the prompt catalogue.
const TEXT: &str = "memory.agent_memory";

/// What may be recalled and how an entry is reclassified, the plugin decides, asked in the context
/// of the work it serves - the agent's, with its owner's rights (issues 131, 255, 516, 518).
pub struct AgentMemoryComponent {
    /// The texts of its prompt part (issue 57).
    prompts: PromptTexts,
    /// Where its prompt part goes: before the turn's input unless configured otherwise.
    position: PromptPosition,
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

    async fn execute(
        &self,
        name: &str,
        params: &Value,
        _answers: Option<&Answers>,
        turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
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
    /// The component, its texts from `prompts`, its prompt part at `position`.
    ///
    /// # Errors
    /// `PromptError` if one of its texts cannot be had.
    pub(crate) fn new(position: PromptPosition, prompts: PromptTexts) -> Result<Self, PromptError> {
        let tools = vec![
            described(
                &prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                Tool {
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
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                Tool {
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
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                Tool {
                    name: FORGET_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: ToolParameters::object([("id", ToolParameters::number())], ["id"]),
                    requires_approval: false,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
            described(
                &prompts,
                NAMESPACE,
                AGENT_MEMORY_KIND,
                Tool {
                    name: RECLASSIFY_MEMORY_TOOL_NAME.to_string(),
                    description: String::new(),
                    parameters: reclassify_parameters(&prompts)?,
                    requires_approval: true,
                    visibility: ToolVisibility::HIDDEN,
                    classification: None,
                    presentation: NOTE_PRESENTATION,
                },
                (),
            )?,
        ];
        let description = tool_text(&prompts, NAMESPACE, AGENT_MEMORY_KIND, "description", ())?;
        Ok(Self { prompts, position, description, tools })
    }

    /// `remember(text)`: remembers `text`, taken over when the turn
    /// completes.
    async fn execute_remember(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id = TurnMemory::remember(turn, text_argument(params)?).await?;
        Ok(serde_json::json!({ "status": "remembered", "id": id.0 }))
    }

    /// `update_memory(id, text)`: changes the text of an entry that may be
    /// recalled here.
    async fn execute_update(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        let text = text_argument(params)?;
        if TurnMemory::update(turn, id, text).await? {
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
        TurnMemory::reclassify(turn, id, classification.clone()).await?;
        Ok(
            serde_json::json!({ "status": "reclassified", "id": id.0, "classification": classification.to_string() }),
        )
    }

    /// `forget(id)`: forgets an entry that may be recalled here.
    async fn execute_forget(&self, params: &Value, turn: &mut Turn<'_>) -> ToolResult<Value> {
        let id: MemoryId = id_argument(params)?;
        if TurnMemory::forget(turn, id).await? {
            Ok(serde_json::json!({ "status": "forgotten", "id": id.0 }))
        } else {
            Err(ToolError::InvalidParameter(format!("you remember nothing numbered {id} here")))
        }
    }
}

#[async_trait]
impl PromptProvider for AgentMemoryComponent {
    fn position(&self) -> PromptPosition {
        self.position
    }

    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        let Some(memory) = situation.reader::<dyn MemoryReader>().await else {
            return Ok(Prompt::default());
        };
        let recalled = memory.recalled();
        if recalled.is_empty() {
            return Ok(Prompt::default());
        }
        // Once for every channel the entries come from, so that knowledge
        // from another session can be told (issue 134); the agent's own, so private.
        let id = AgentMemoryId::of(situation.agent_id());
        let source = KnowledgeSource::entity::<AgentMemory>(id, memory.digest());
        let mut knowledge: Vec<KnowledgeRef> = Vec::new();
        for entry in recalled {
            if !knowledge.iter().any(|k| k.channel == entry.origin.channel) {
                let from_there = recalled
                    .iter()
                    .filter(|m| m.origin.channel == entry.origin.channel)
                    .map(|m| &m.classification);
                knowledge.push(KnowledgeRef {
                    channel: entry.origin.channel,
                    session: entry.origin.session.id,
                    source: source.clone(),
                    classification: Classification::join_all(from_there),
                    private: true,
                });
            }
        }
        Ok(Prompt::of_knowledge(render(&self.prompts, recalled)?, knowledge))
    }
}

/// The prompt part of `prompts` for what the agent `recalled` here.
fn render(prompts: &PromptTexts, recalled: &[Memory]) -> Result<String, PromptError> {
    let recalled: Vec<_> =
        recalled.iter().map(|m| context! { id => m.id.to_string(), text => m.text }).collect();
    prompts.render(TEXT, context! { recalled, remember => REMEMBER_TOOL_NAME, update => UPDATE_MEMORY_TOOL_NAME, forget => FORGET_TOOL_NAME })
}

pub fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        writing_for_the_model(
            ComponentKind::new(AGENT_MEMORY_KIND)
                .params::<PositionParams>()
                .reads::<dyn MemoryReader>()
                .writes::<dyn MemoryWriter>()
                .provides::<dyn ToolProvider>()
                .provides::<dyn PromptProvider>(),
        ),
        |descriptor, context| async move {
            let texts = texts_of(&context).await?;
            let position =
                PositionParams::position_of(&descriptor.params, PromptPosition::BeforeTurnInput)?;
            let component = Arc::new(AgentMemoryComponent::new(position, texts)?);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::<AgentMemoryComponent>::clone(&component));
            facets.insert::<dyn PromptProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
#[cfg(feature = "kernel")]
mod tests {
    use super::*;
    use crate::library::MemoryLibrary;
    use crate::memory::{AgentMemory, MemoryChange};
    use crate::plugin::MemoryPlugin;
    use crate::PROMPTS;
    use caelumora_agents::library::AgentsLibrary;
    use caelumora_agents::testing::prompts_with;
    use caelumora_agents::tool::ToolError;
    use caelumora_api::runtime::ActorRuntime;
    use caelumora_api::staging::CommitError;
    use caelumora_domain::actor::{ActorId, EventContext};
    use caelumora_domain::agent::{AgentChannelState, AgentId};
    use caelumora_domain::channel::testing::{bare_channel, main_channel};
    use caelumora_domain::channel::{ChannelId, Participant};
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use caelumora_domain::confidentiality::{Compartment, Compartments, Place, Reading};
    use caelumora_domain::ledger::Change;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::privacy::{PrivacyConfig, SessionPrivacy};
    use caelumora_domain::role::{RoleId, Roles};
    use caelumora_domain::session::testing::{bare_session, make_session};
    use caelumora_domain::session::{Session, SessionId};
    use caelumora_domain::turn::{StagedChange, StagedChanges};
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::work::Work;
    use caelumora_kernel::kernel::{AgentRuntime, Kernel, ParticipantRuntime};
    use caelumora_kernel::session::testing::{
        detached_participant_in, owned, participant_runtime_in, spec,
    };
    use caelumora_kernel::session::{ChannelSpec, SessionLoop, SessionManager};
    use caelumora_kernel::testing::TestKernel;
    use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
    use caelumora_security::{Policies, SecurityContext};
    use caelumora_store::testing::record_users;
    use caelumora_store::{EntityStore, EntityStores, Persistence};
    use std::collections::HashMap;
    use std::ops::Deref;
    use std::slice;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// A channel of the tests: where it is - its session and itself - and
    /// how it is classified.
    type At = ((SessionId, ChannelId), Classification);

    /// The context of an envelope routed in the channel `at`.
    fn in_channel(at: &At) -> EventContext {
        EventContext::new(at.0 .0, at.0 .1, ActorId::None, String::new())
    }

    /// The component of an agent, and the agent's owner: whom its turns act for, who owns its
    /// memory (issue 255).
    struct Remembering {
        component: AgentMemoryComponent,
        owner: ActorId,
    }

    impl Remembering {
        /// Begins a turn of `agent` in `state`, in channel `at`, as its actor
        /// begins one.
        fn begin(&self, state: &mut AgentChannelState, at: &At) {
            state.begin_turn(Work::new(in_channel(at), None, at.1.clone(), Default::default()));
        }

        /// The runtime a turn of `agent` in channel `at` acts through: its actor's there, run by
        /// its session, acting with its owner's rights (issue 255). The agent takes no part, so the
        /// test gives the runtime the context the kernel builds for a participant (`acting_as`,
        /// issue 516).
        fn runtime_in(&self, sessions: &Sessions, at: &At, agent: AgentId) -> ParticipantRuntime {
            let context = SecurityContext::new(self.owner, at.0 .0, at.0 .1).next(
                ActorId::Agent(agent),
                self.owner,
                self.owner,
            );
            sessions.runtime_in(at, agent).acting_as(context)
        }
    }

    impl Deref for Remembering {
        type Target = AgentMemoryComponent;
        fn deref(&self) -> &AgentMemoryComponent {
            &self.component
        }
    }

    /// The component of an agent owned by `owner`; what it may recall and reclassify its turn's
    /// runtime decides, by the policies the sessions of the test run under (issue 516).
    fn component(owner: ActorId) -> Remembering {
        Remembering {
            component: AgentMemoryComponent::new(
                PromptPosition::BeforeTurnInput,
                prompts_with(NAMESPACE, PROMPTS),
            )
            .unwrap(),
            owner,
        }
    }

    /// The sessions of a test, as a turn reaches them: made as the kernel makes a session
    /// (`SessionManager::create`, issue 486) and running, with every channel a turn works in,
    /// through which it reaches its session and reads its privacy from its loop. Each is changed,
    /// deleted and restored by its owner.
    struct Sessions {
        kernel: Arc<Kernel>,
        stores: EntityStores,
        /// The agents' memories, read as the kernel's system.
        memories: Arc<dyn EntityStore<AgentMemory>>,
        manager: Arc<SessionManager>,
        loops: HashMap<SessionId, Arc<SessionLoop>>,
        /// How many were made, for their names.
        made: usize,
    }

    impl Sessions {
        /// The sessions of a kernel with the memory's plugin and library under the ownership
        /// policy, which learns who belongs to a group from its store, with `rules` as the
        /// confidentiality policy (`owned`).
        fn under(rules: ConfidentialityRules) -> Self {
            Self::policed(Arc::new(rules))
        }

        /// As `under`, with `confidentiality` as the confidentiality policy: what a turn's
        /// runtime decides what is recalled by (issue 516).
        fn policed(confidentiality: Arc<dyn ConfidentialityPolicy>) -> Self {
            let store = Persistence::in_memory();
            let memories = store.entity_store::<AgentMemory>(&Policies::permitting_all()).unwrap();
            let (kernel, manager) = owned(TestKernel {
                store,
                confidentiality,
                plugins: vec![&MemoryPlugin],
                libraries: vec![&AgentsLibrary, &MemoryLibrary],
                ..Default::default()
            });
            let stores = kernel.entity_stores().clone();
            Self { kernel, stores, memories, manager, loops: HashMap::new(), made: 0 }
        }

        /// What `agent` remembers now, as it is stored.
        async fn remembered(&self, agent: AgentId) -> Vec<Memory> {
            let system = SecurityContext::system();
            match self.memories.get(&system, AgentMemoryId::of(agent)).await {
                Ok(memory) => memory.chain.current(),
                Err(_) => Vec::new(),
            }
        }

        /// A user named `name`, stored (`record_users`): to own sessions and
        /// the agent, or to take part.
        async fn user(&self, name: &str) -> UserId {
            let user = make_user(name);
            record_users(&self.stores, &[&user]).await;
            user.id
        }

        /// Creates a session of `owner`, who takes part in it, letting others
        /// read as `policy` says: running with its main channel.
        async fn create(&mut self, owner: UserId, policy: PrivacyConfig) -> SessionId {
            self.made += 1;
            let session = make_session(&format!("s{}", self.made), owner);
            let session = Session {
                privacy: policy,
                participants: vec![Participant::User(owner)],
                ..session
            };
            let channels = vec![main_channel(&session)];
            let running = self.manager.create(&as_user(owner), session, channels).await.unwrap();
            self.loops.insert(running.id(), Arc::clone(&running));
            running.id()
        }

        /// Creates a channel of session `id` classified as `classification`,
        /// by the session's owner, with everyone in the session: where a turn
        /// works.
        async fn create_channel(&self, id: SessionId, classification: Classification) -> At {
            let session = self.running(id);
            let participants =
                session.session().participants.iter().map(Participant::actor_id).collect();
            let spec = ChannelSpec {
                classification: classification.clone(),
                ..spec(&format!("c{}", session.channels().len()), participants)
            };
            let channel = session.create_channel(&owner_of(session), spec).await.unwrap();
            ((id, channel.id()), classification)
        }

        /// Session `id` lets others read as `policy` says from now on.
        async fn set_privacy(&self, id: SessionId, policy: PrivacyConfig) {
            let session = self.running(id);
            session.update_privacy(&owner_of(session), policy).await.unwrap();
        }

        /// Deletes session `id`: there is none such for whoever looks it up.
        async fn delete(&mut self, id: SessionId) {
            let session = self.loops.remove(&id).expect("a running session of the test");
            assert!(self.manager.delete(&owner_of(&session), id).await.unwrap());
        }

        /// Restores the deleted session `id` of `owner` and runs it again.
        async fn restore(&mut self, id: SessionId, owner: UserId) {
            self.manager.restore(&as_user(owner), id).await.unwrap();
            let running = self.manager.open(&as_user(owner), id).await.unwrap();
            self.loops.insert(id, running);
        }

        /// The runtime a turn of `agent` in channel `at` acts through: its actor's in that
        /// channel, run by its session.
        fn runtime_in(&self, at: &At, agent: AgentId) -> ParticipantRuntime {
            let channel = self.running(at.0 .0).channel(at.0 .1).expect("a channel of the test");
            participant_runtime_in(&channel, ActorId::Agent(agent))
        }

        fn running(&self, id: SessionId) -> &Arc<SessionLoop> {
            self.loops.get(&id).expect("a running session of the test")
        }
    }

    fn as_user(user: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(user), Default::default(), Default::default())
    }

    /// What the owner of `session` acts in.
    fn owner_of(session: &SessionLoop) -> SecurityContext {
        SecurityContext::new(
            session.session().security_attributes.owner_id,
            Default::default(),
            Default::default(),
        )
    }

    /// Remembers `text` in a turn in channel `at` that was told something
    /// classified as the channel.
    async fn remember(
        memory: &Remembering,
        sessions: &Sessions,
        agent: AgentId,
        at: At,
        text: &str,
    ) {
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(sessions, &at, agent))),
        )
        .expect("a turn was begun");
        turn.see(&at.1);
        memory
            .execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": text }), None, &mut turn)
            .await
            .unwrap();
        turn.take_over().await;
    }

    /// The prompt part of another actor of `agent`, in a turn in channel `at`.
    async fn recall(memory: &Remembering, sessions: &Sessions, agent: AgentId, at: At) -> Prompt {
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at);
        let turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(sessions, &at, agent))),
        )
        .expect("a turn was begun");
        memory.get_prompt(&turn.situation()).await.unwrap()
    }

    #[tokio::test]
    async fn what_is_remembered_is_recalled_wherever_its_origin_may_be_read() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let other_session = sessions.create(owner, PrivacyConfig::Private).await;
        let team = sessions
            .create_channel(session, Classification::general(Confidentiality::Internal))
            .await;
        let public = sessions
            .create_channel(session, Classification::general(Confidentiality::Public))
            .await;
        let elsewhere = sessions
            .create_channel(other_session, Classification::general(Confidentiality::Internal))
            .await;

        // A turn in an internal channel remembers something it was told
        // there ...
        let mut state = AgentChannelState::default();
        {
            memory.begin(&mut state, &team);
            let mut turn = Turn::of(
                &mut state,
                Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &team, agent))),
            )
            .expect("a turn was begun");
            turn.see(&team.1);
            let result = memory
                .execute(
                    REMEMBER_TOOL_NAME,
                    &serde_json::json!({ "text": "Kaya likes tea." }),
                    None,
                    &mut turn,
                )
                .await
                .unwrap();
            assert_eq!(result.value["id"], 1);
            assert!(sessions.remembered(agent).await.is_empty(), "not before the turn completes");
            turn.take_over().await;
        }

        // ... which another actor of the agent recalls in a channel that may
        // read that one, and not where it may not be known.
        let same_session = recall(
            &memory,
            &sessions,
            agent,
            sessions
                .create_channel(session, Classification::general(Confidentiality::Confidential))
                .await,
        )
        .await;
        assert!(same_session.text.contains("#1: Kaya likes tea."), "{}", same_session.text);
        assert_eq!(same_session.knowledge.len(), 1, "recorded as knowledge read");
        assert!(
            recall(&memory, &sessions, agent, public).await.text.is_empty(),
            "a public channel may not read an internal one"
        );
        assert!(
            recall(&memory, &sessions, agent, elsewhere.clone()).await.text.is_empty(),
            "another session, private"
        );

        // Forgotten only where it may be recalled.
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &elsewhere);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &elsewhere, agent))),
        )
        .expect("a turn was begun");
        assert!(memory
            .execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn)
            .await
            .is_err());
        memory.begin(&mut state, &team);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &team, agent))),
        )
        .expect("a turn was begun");
        memory
            .execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn)
            .await
            .unwrap();
        turn.take_over().await;
        assert!(sessions.remembered(agent).await.is_empty());
    }

    #[tokio::test]
    async fn an_entry_is_updated_only_where_it_is_recalled_and_stands_where_it_was_updated() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let other_session = sessions.create(owner, PrivacyConfig::Private).await;
        let team = sessions
            .create_channel(session, Classification::general(Confidentiality::Internal))
            .await;
        let confidential = sessions
            .create_channel(session, Classification::general(Confidentiality::Confidential))
            .await;
        let elsewhere = sessions
            .create_channel(other_session, Classification::general(Confidentiality::Internal))
            .await;
        remember(&memory, &sessions, agent, team.clone(), "Kaya likes tea.").await;

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &elsewhere);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &elsewhere, agent))),
        )
        .expect("a turn was begun");
        let there = memory
            .execute(
                UPDATE_MEMORY_TOOL_NAME,
                &serde_json::json!({ "id": 1, "text": "Kaya likes coffee." }),
                None,
                &mut turn,
            )
            .await;
        assert!(
            matches!(there, Err(ToolError::InvalidParameter(_))),
            "not where it is not recalled"
        );

        // Updated in a turn that has seen what is confidential ...
        memory.begin(&mut state, &confidential);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &confidential, agent))),
        )
        .expect("a turn was begun");
        turn.see(&confidential.1);
        let updated = memory
            .execute(
                UPDATE_MEMORY_TOOL_NAME,
                &serde_json::json!({ "id": 1, "text": "Kaya likes coffee." }),
                None,
                &mut turn,
            )
            .await
            .unwrap();
        assert_eq!(updated.value, serde_json::json!({ "status": "updated", "id": 1 }));
        turn.take_over().await;
        let remembered = sessions.remembered(agent).await;
        assert_eq!(
            (remembered[0].id, remembered[0].text.as_str(), remembered[0].origin.channel),
            (MemoryId(1), "Kaya likes coffee.", confidential.0 .1),
            "its number kept, written where it was updated"
        );
        assert_eq!(remembered[0].classification, confidential.1);

        // ... is recalled where that may be known, and no longer where only the old text might.
        assert!(recall(&memory, &sessions, agent, confidential)
            .await
            .text
            .contains("#1: Kaya likes coffee."));
        assert!(recall(&memory, &sessions, agent, team).await.text.is_empty());
    }

    #[tokio::test]
    async fn an_entry_remembered_and_forgotten_in_one_turn_is_gone_once_it_completes() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let team = sessions
            .create_channel(session, Classification::general(Confidentiality::Internal))
            .await;

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &team);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &team, agent))),
        )
        .expect("a turn was begun");
        memory
            .execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "draft" }), None, &mut turn)
            .await
            .unwrap();
        memory
            .execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn)
            .await
            .expect("remembered in this turn");
        assert!(
            memory
                .execute(FORGET_TOOL_NAME, &serde_json::json!({ "id": 1 }), None, &mut turn)
                .await
                .is_err(),
            "forgotten already"
        );
        let forgotten = memory
            .execute(
                RECLASSIFY_MEMORY_TOOL_NAME,
                &serde_json::json!({ "id": 1, "level": "public" }),
                None,
                &mut turn,
            )
            .await;
        assert!(
            matches!(forgotten, Err(ToolError::InvalidParameter(_))),
            "a forgotten entry is not reclassified"
        );
        turn.take_over().await;
        assert!(sessions.remembered(agent).await.is_empty());
    }

    /// Issue 518: the memory is reached through the work's session, as the plugin binds it; a
    /// turn that reaches none neither recalls nor remembers.
    #[tokio::test]
    async fn a_turn_in_no_session_neither_recalls_nor_remembers() {
        let agent = AgentId::new();
        let sessions = Sessions::under(Default::default());
        let memory = component(ActorId::User(UserId::new()));
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        let at = ((session.id, channel.id), Classification::general(Confidentiality::Internal));
        let kernel = Arc::clone(&sessions.kernel);
        let runtime = detached_participant_in(kernel, session, channel, ActorId::Agent(agent));
        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &at);
        let mut turn = Turn::of(&mut state, Arc::new(AgentRuntime::in_test(runtime)))
            .expect("a turn was begun");
        let refused = memory
            .execute(REMEMBER_TOOL_NAME, &serde_json::json!({ "text": "alone" }), None, &mut turn)
            .await;
        assert!(matches!(refused, Err(ToolError::ExecutionFailed(_))), "nothing to remember in");
        assert!(
            memory.get_prompt(&turn.situation()).await.unwrap().text.is_empty(),
            "nor to recall from"
        );
    }

    /// Lets channels read each other until it is told to refuse, and records in whose name it is
    /// asked, by the caller and the principal - and asks nothing else: what an agent remembered is
    /// its own, recalled whoever may read its channel now (issue 264).
    #[derive(Debug, Default)]
    struct Switched {
        refusing: AtomicBool,
        asked: Mutex<Vec<(ActorId, ActorId)>>,
        compartments: Compartments,
    }
    impl ConfidentialityPolicy for Switched {
        fn may_publish(&self, _: &SecurityContext, _: &Classification, _: &Classification) -> bool {
            true
        }
        fn copyable(
            &self,
            context: &SecurityContext,
            _: ChannelId,
            _: &SessionPrivacy,
            _: &Place,
        ) -> Reading {
            self.asked.lock().unwrap().push((context.caller_id, context.principal_id));
            match self.refusing.load(Ordering::SeqCst) {
                true => Reading::Nothing,
                false => Reading::Whole,
            }
        }
        fn cleared(
            &self,
            _: &SecurityContext,
            _: ActorId,
            _: &[RoleId],
            _: &Classification,
        ) -> bool {
            true
        }
        fn compartments(&self) -> &Compartments {
            &self.compartments
        }
        fn roles(&self) -> Option<&Roles> {
            None
        }
    }

    #[tokio::test]
    async fn the_policy_decides_what_is_recalled_asked_in_the_agents_name() {
        let agent = AgentId::new();
        let policy = Arc::new(Switched::default());
        let mut sessions = Sessions::policed(Arc::<Switched>::clone(&policy));
        let owner = sessions.user("owner").await;
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let memory = component(ActorId::User(owner));
        // Public, so that the built-in rule would let every channel read it.
        let origin = sessions
            .create_channel(session, Classification::general(Confidentiality::Public))
            .await;
        let reader = sessions
            .create_channel(session, Classification::general(Confidentiality::Internal))
            .await;
        remember(&memory, &sessions, agent, origin.clone(), "Kaya likes tea.").await;

        let permitted = recall(&memory, &sessions, agent, reader.clone()).await;
        assert!(permitted.text.contains("Kaya likes tea."), "{}", permitted.text);
        policy.refusing.store(true, Ordering::SeqCst);
        let refused = recall(&memory, &sessions, agent, reader).await;
        assert!(refused.text.is_empty(), "the policy refuses, whatever the levels say");
        let in_its_own_channel = recall(&memory, &sessions, agent, origin).await;
        assert!(
            in_its_own_channel.text.contains("Kaya likes tea."),
            "a channel always reads itself"
        );
        let asked = policy.asked.lock().unwrap();
        assert!(
            !asked.is_empty()
                && asked
                    .iter()
                    .all(|asked| *asked == (ActorId::Agent(agent), ActorId::User(owner))),
            "in the agent's name, with its owner's rights: {asked:?}"
        );
    }

    #[tokio::test]
    async fn what_a_turn_remembers_is_classified_as_what_it_has_seen() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let confidential = sessions
            .create_channel(session, Classification::general(Confidentiality::Confidential))
            .await;
        let public = sessions
            .create_channel(session, Classification::general(Confidentiality::Public))
            .await;

        // A turn in a confidential channel that has seen only what is
        // internal remembers it as internal (issue 168) ...
        let mut state = AgentChannelState::default();
        {
            memory.begin(&mut state, &confidential);
            let mut turn = Turn::of(
                &mut state,
                Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &confidential, agent))),
            )
            .expect("a turn was begun");
            turn.see(&Classification::general(Confidentiality::Internal));
            let result = memory
                .execute(
                    REMEMBER_TOOL_NAME,
                    &serde_json::json!({ "text": "Kaya likes tea." }),
                    None,
                    &mut turn,
                )
                .await
                .unwrap();
            assert_eq!(
                result.classification,
                Some(Classification::general(Confidentiality::Internal)),
                "its answer as the turn"
            );
            turn.take_over().await;
        }
        let remembered = sessions.remembered(agent).await;
        assert_eq!(
            remembered[0].classification,
            Classification::general(Confidentiality::Internal)
        );

        // ... so that an internal channel recalls it, a public one not.
        assert!(recall(
            &memory,
            &sessions,
            agent,
            sessions
                .create_channel(session, Classification::general(Confidentiality::Internal))
                .await
        )
        .await
        .text
        .contains("Kaya likes tea."));
        assert!(recall(&memory, &sessions, agent, public).await.text.is_empty());

        // What is recalled is recorded with its classification, and is
        // part of what the turn has seen.
        let prompt = recall(&memory, &sessions, agent, confidential).await;
        assert_eq!(
            prompt.knowledge[0].classification,
            Some(Classification::general(Confidentiality::Internal))
        );
        assert_eq!(
            prompt.classification,
            Some(Classification::PUBLIC),
            "nothing but what it recalls"
        );
    }

    #[tokio::test]
    async fn an_entry_released_to_another_compartment_is_recalled_there() {
        let agent = AgentId::new();
        let compartments = Compartments::new([
            (Compartment::new("hr"), vec![]),
            (Compartment::new("finance"), vec![]),
        ])
        .unwrap();
        let rules = ConfidentialityRules::new(compartments);
        let mut sessions = Sessions::under(rules.clone());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let in_compartment = |name: &str| {
            Classification::new(Confidentiality::Internal, [Compartment::new(name)]).unwrap()
        };
        let hr = sessions.create_channel(session, in_compartment("hr")).await;
        let finance = sessions.create_channel(session, in_compartment("finance")).await;
        remember(&memory, &sessions, agent, hr.clone(), "The budget is tight.").await;
        assert!(
            recall(&memory, &sessions, agent, finance.clone()).await.text.is_empty(),
            "HR's, not finance's"
        );

        let mut state = AgentChannelState::default();
        memory.begin(&mut state, &hr);
        let mut turn = Turn::of(
            &mut state,
            Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &hr, agent))),
        )
        .expect("a turn was begun");
        let above = serde_json::json!({ "id": 1, "level": "confidential", "compartments": ["hr"] });
        assert!(
            memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &above, None, &mut turn).await.is_err(),
            "never above the channel"
        );
        let unknown = serde_json::json!({ "id": 7, "level": "internal", "compartments": ["hr"] });
        assert!(matches!(
            memory.execute(RECLASSIFY_MEMORY_TOOL_NAME, &unknown, None, &mut turn).await,
            Err(ToolError::InvalidParameter(_))
        ));
        let released =
            serde_json::json!({ "id": 1, "level": "internal", "compartments": ["hr", "finance"] });
        memory
            .execute(RECLASSIFY_MEMORY_TOOL_NAME, &released, None, &mut turn)
            .await
            .expect("the agent releases what it remembers");
        turn.take_over().await;

        let there = recall(&memory, &sessions, agent, finance).await;
        assert!(there.text.contains("The budget is tight."), "released to finance: {}", there.text);
        assert!(
            recall(&memory, &sessions, agent, hr).await.text.contains("The budget is tight."),
            "and still HR's"
        );
    }

    #[tokio::test]
    async fn an_entry_is_recalled_in_another_session_where_its_session_agreed_then_and_now() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let (alice, bob) = (sessions.user("alice").await, sessions.user("bob").await);
        let memory = component(ActorId::User(alice));
        let hers = sessions.create(alice, PrivacyConfig::SameOwner).await;
        let other = sessions.create(alice, PrivacyConfig::Private).await;
        let bobs = sessions.create(bob, PrivacyConfig::Private).await;

        // Remembered in a session of hers that lets her other sessions read it ...
        let origin =
            sessions.create_channel(hers, Classification::general(Confidentiality::Internal)).await;
        remember(&memory, &sessions, agent, origin.clone(), "Kaya likes tea.").await;
        let remembered = sessions.remembered(agent).await;
        assert_eq!(
            remembered[0].origin.session.policy,
            PrivacyConfig::SameOwner,
            "the entry keeps its session's privacy"
        );

        // ... and recalled in another of hers, while the session still agrees.
        let elsewhere = sessions
            .create_channel(other, Classification::general(Confidentiality::Internal))
            .await;
        let recalled = recall(&memory, &sessions, agent, elsewhere.clone()).await;
        assert!(recalled.text.contains("Kaya likes tea."), "{}", recalled.text);
        assert_eq!(recalled.knowledge.len(), 1);
        assert_eq!(
            (recalled.knowledge[0].session, recalled.knowledge[0].channel),
            origin.0,
            "recorded with the channel it came from, of its session"
        );

        sessions.set_privacy(hers, PrivacyConfig::Private).await;
        assert!(
            recall(&memory, &sessions, agent, elsewhere.clone()).await.text.is_empty(),
            "no longer, once the session is private"
        );
        sessions.delete(hers).await;
        assert!(
            recall(&memory, &sessions, agent, elsewhere.clone())
                .await
                .text
                .contains("Kaya likes tea."),
            "of a session that is gone, what it agreed to then"
        );

        let in_bobs = recall(
            &memory,
            &sessions,
            agent,
            sessions.create_channel(bobs, Classification::general(Confidentiality::Internal)).await,
        )
        .await;
        assert!(in_bobs.text.is_empty(), "not in someone else's session");

        // What was said while the session was private stays there, even
        // after it lets others read.
        sessions.restore(hers, alice).await;
        remember(
            &memory,
            &sessions,
            agent,
            sessions.create_channel(hers, Classification::general(Confidentiality::Internal)).await,
            "Kaya is moving.",
        )
        .await;
        sessions.set_privacy(hers, PrivacyConfig::SameOwner).await;
        let recalled = recall(
            &memory,
            &sessions,
            agent,
            sessions
                .create_channel(other, Classification::general(Confidentiality::Internal))
                .await,
        )
        .await;
        assert!(
            recalled.text.contains("Kaya likes tea.") && !recalled.text.contains("Kaya is moving."),
            "{}",
            recalled.text
        );
    }

    /// Issue 518: the plugin commits only what is the work's to change, whatever a turn staged -
    /// an entry recalled where the work reads, or a new one of a number it reserved and never
    /// gave, classified within the work -, all or none, and derives where a change was made from
    /// the work's place.
    #[tokio::test]
    async fn the_plugin_commits_only_what_is_the_works_to_change() {
        let agent = AgentId::new();
        let mut sessions = Sessions::under(Default::default());
        let owner = sessions.user("owner").await;
        let memory = component(ActorId::User(owner));
        let session = sessions.create(owner, PrivacyConfig::Private).await;
        let internal = Classification::general(Confidentiality::Internal);
        let team = sessions.create_channel(session, internal.clone()).await;
        let public = sessions
            .create_channel(session, Classification::general(Confidentiality::Public))
            .await;
        remember(&memory, &sessions, agent, team.clone(), "Kaya likes tea.").await;
        let commit = |at: &At, kind: &'static str, changes: &[MemoryChange]| {
            let runtime = AgentRuntime::in_test(memory.runtime_in(&sessions, at, agent));
            let mut staged = StagedChanges::default();
            for change in changes {
                staged.push(change).unwrap();
            }
            let classification = at.1.clone();
            async move {
                let initiator = ActorId::User(owner);
                let after = MessageId::new();
                runtime.commit_staged(initiator, classification, &staged, kind, after).await
            }
        };
        let reserve = || async {
            let runtime: Arc<dyn ActorRuntime> =
                Arc::new(AgentRuntime::in_test(memory.runtime_in(&sessions, &team, agent)));
            let writer = runtime.writer::<dyn MemoryWriter>(ActorId::User(owner), team.1.clone());
            writer.await.expect("its writer").reserve_id().await.unwrap()
        };
        let invalid = |result| matches!(result, Err(CommitError::Invalid(_)));
        let new = |id, classification| {
            MemoryChange(Change::Add { id, text: "new".into(), classification })
        };

        let forget = MemoryChange(Change::Remove { id: MemoryId(1) });
        let update = MemoryChange(Change::Update {
            id: MemoryId(1),
            text: "Kaya likes coffee.".into(),
            classification: Classification::PUBLIC,
        });
        assert!(
            invalid(commit(&public, MemoryChange::KIND, &[update]).await),
            "not recalled there"
        );
        assert!(invalid(commit(&public, MemoryChange::KIND, slice::from_ref(&forget)).await));
        let made_up = new(MemoryId(9), internal.clone());
        assert!(invalid(commit(&team, MemoryChange::KIND, &[made_up]).await), "never reserved");
        let given = new(MemoryId(1), internal.clone());
        assert!(invalid(commit(&team, MemoryChange::KIND, &[given]).await), "given already");
        let twice = new(reserve().await, internal.clone());
        let both = [twice.clone(), twice];
        assert!(invalid(commit(&team, MemoryChange::KIND, &both).await), "one number, one entry");
        let above = new(reserve().await, Classification::general(Confidentiality::Confidential));
        assert!(invalid(commit(&team, MemoryChange::KIND, &[above]).await), "not above the work");
        let unknown = commit(&team, "nothing", &[]).await;
        assert!(matches!(unknown, Err(CommitError::NotOffered(_))), "no writer commits it");
        assert_eq!(sessions.remembered(agent).await.len(), 1, "nothing of it committed");

        let here = new(reserve().await, Classification::PUBLIC);
        commit(&public, MemoryChange::KIND, &[here]).await.expect("its own to remember");
        let remembered = sessions.remembered(agent).await;
        assert_eq!(remembered[1].origin.channel, public.0 .1, "made where the work reads from");
        commit(&team, MemoryChange::KIND, &[forget]).await.expect("recalled where it is forgotten");
        assert_eq!(sessions.remembered(agent).await.len(), 1);
    }

    #[test]
    fn every_entry_acted_on_is_shown_as_a_note_to_the_owner_alone() {
        let tools = AgentMemoryComponent::new(
            PromptPosition::BeforeTurnInput,
            prompts_with(NAMESPACE, PROMPTS),
        )
        .unwrap()
        .tools();
        assert!(
            tools
                .iter()
                .all(|t| (t.presentation, t.visibility)
                    == (NOTE_PRESENTATION, ToolVisibility::HIDDEN))
        );
    }
}
