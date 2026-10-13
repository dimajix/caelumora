//! The world the security suite runs in (issue 198), for the suite and
//! for the tests of a plugin (issue 323), behind the feature
//! `test-support`: the runtime with the ownership policy, recording every
//! question it is asked (`Recorder`), and three users - alice and bob on
//! the staff, carol a stranger to everything of theirs. What the system
//! starts from is put into the store as the system, as a setup would;
//! everything a user does goes through the public API, in that user's name.
//! A test of an action asserts the questions asked in a user's name, in
//! order (`World::asked`): which permissions the action needs.

use caelumora_model::store::query::Query;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use caelumora_model::component::ComponentConfig;
use anyhow::Error;
use tempfile::TempDir;
use chrono::Utc;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId, PersonalityKind};
use caelumora_model::channel::{moderation_components, Channel, Participant};
use caelumora_model::config::{ArtifactsConfig, BackendConfig, WorkspacesConfig};
use crate::backend::Backends;
use serde_json::json;
use caelumora_model::confidentiality::{Classification, Clearance, Compartments, ConfidentialityPolicy, ConfidentialityRules, Confidentiality, Place};
use caelumora_model::role::{RoleId, Roles};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::value::text::{Description, Introduction};
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::group::{Group, GroupId};
use caelumora_model::security::{ActionType, Grant, OwnershipSecurityManager, Reclassified, SecurityAttributes, SecurityContext, SecurityManager, Subject};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::{ComponentStore, StoreError};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::user::{User, UserId};
use crate::component::ComponentRegistry;
use crate::runtime::{RuntimeBuilder, RuntimeManager};
use crate::plugin::Plugin;
use crate::agent::state::AgentStateHandle;
use crate::agent::turn::Turn;
use crate::channel::ChannelLoop;
use crate::actor::user::ChannelWatch;
use crate::component::{ComponentFacets, ComponentKind};
use crate::llm::{LLMClient, LLMError, LLMResult};
use crate::view::ViewsFactory;
use caelumora_model::actor::{ActorEvent, EventContext};
use caelumora_model::agent::AgentChannelState;
use caelumora_model::context::ModelProfile;
use caelumora_model::message::{Message, MessageEvent};
use caelumora_model::tool::Tool;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use crate::session::{SessionError, SessionLoop, SessionManager};
use crate::error::refusal;
use crate::view::Viewer;
use crate::workspace::WorkspaceError;
use caelumora_model::config::AppConfig;

/// The ownership policy, recording every question it is asked - who acted
/// with whose rights and for whom, what, of whose entity, and what it
/// answered - so that a test says which permissions an action needs (issue
/// 198). Recorded as `principal as caller, asked by initiator`, each part
/// only where it differs from the one before (issue 255).
#[derive(Debug)]
pub struct Recorder {
    policy: OwnershipSecurityManager,
    /// The flow decisions (issue 304), recorded in the same list, so that
    /// the order of an action's questions shows both.
    rules: ConfidentialityRules,
    pub asked: Mutex<Vec<String>>,
    /// The names actors are recorded by.
    pub names: Mutex<HashMap<ActorId, String>>,
}

impl Recorder {
    pub fn name(&self, actor: ActorId) -> String {
        match actor {
            ActorId::System => "system".to_string(),
            other => self.names.lock().unwrap().get(&other).cloned().unwrap_or_else(|| other.to_string()),
        }
    }

    /// `action` as recorded: its kind, and whose entity it is about.
    fn label(&self, action: &ActionType<'_>) -> String {
        let kind = action.describe().split('(').next().unwrap_or_default().to_string();
        match action {
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => format!("{kind}({}'s)", self.name(*author)),
            ActionType::Reclassify(Reclassified::Message { author, .. }) => format!("{kind}({}'s message)", self.name(*author)),
            ActionType::List(_) => action.describe(),
            _ => match attributes_of(action) {
                Some(attributes) => format!("{kind}({}'s)", self.name(attributes.owner_id)),
                None => kind,
            },
        }
    }

    /// Records `what` was asked in `context`, and the answer.
    fn record(&self, context: &SecurityContext, what: String, allowed: bool) {
        let acting = match (context.principal_id, context.caller_id) {
            (principal, caller) if caller == principal || caller == ActorId::None => self.name(principal),
            (principal, caller) => format!("{} as {}", self.name(principal), self.name(caller)),
        };
        let who = match context.initiator_id {
            initiator if initiator == context.principal_id => acting,
            initiator => format!("{acting}, asked by {}", self.name(initiator)),
        };
        let answer = if allowed { "yes" } else { "no" };
        self.asked.lock().unwrap().push(format!("{who}: {what} {answer}"));
    }
}

#[async_trait]
impl SecurityManager for Recorder {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        let allowed = self.policy.check(context, action).await;
        self.record(context, self.label(&action), allowed);
        allowed
    }
}

/// The flow decisions, recorded by the names the actions had before they
/// left the `SecurityManager` (issue 304): `Publish`, `CopyInformation`,
/// and `JoinChannel` with whom it clears.
impl ConfidentialityPolicy for Recorder {
    fn may_publish(&self, context: &SecurityContext, classification: &Classification, into: &Classification) -> bool {
        let allowed = self.rules.may_publish(context, classification, into);
        self.record(context, "Publish".to_string(), allowed);
        allowed
    }

    fn may_copy(&self, context: &SecurityContext, from: &Place, into: &Place) -> bool {
        let allowed = self.rules.may_copy(context, from, into);
        self.record(context, "CopyInformation".to_string(), allowed);
        allowed
    }

    fn cleared(&self, context: &SecurityContext, participant: ActorId, roles: &[RoleId], classification: &Classification) -> bool {
        let allowed = self.rules.cleared(context, participant, roles, classification);
        self.record(context, format!("JoinChannel({})", self.name(participant)), allowed);
        allowed
    }

    fn compartments(&self) -> &Compartments {
        self.rules.compartments()
    }

    fn roles(&self) -> Option<&Roles> {
        self.rules.roles()
    }
}

/// The attributes of the entity `action` is about, if it is about one.
pub fn attributes_of<'a>(action: &ActionType<'a>) -> Option<&'a SecurityAttributes> {
    use ActionType::*;
    match *action {
        Entity { attributes: a, .. }
        | ReadAgentContext(a) | ReadAgentTools { agent: a, .. } | ReadAgentComponents(a) | ApproveToolCall { agent: a, .. } | ShowAgentCommand { agent: a, .. } | ExecuteAgentCommand { agent: a, .. }
        | StopSession(a) | CancelChannel(a) | WakeChannel(a) | RetryMessage(a) | AnswerQuestion(a) | ShowCommand { channel: a, .. } | ExecuteCommand { channel: a, .. } => Some(a),
        _ => None,
    }
}

/// The role every user of the world holds.
pub static STAFF: LazyLock<RoleId> = LazyLock::new(RoleId::new);

/// The runtime with the ownership policy, and three users: alice and bob
/// on the staff, carol a stranger to everything of theirs.
pub struct World {
    pub policy: Arc<Recorder>,
    pub store: Arc<MemoryStore>,
    pub runtime: Arc<RuntimeManager>,
    pub manager: Arc<SessionManager>,
    pub alice: UserId,
    pub bob: UserId,
    pub carol: UserId,
    /// Where the storage "main" keeps the workspaces' files.
    pub files: TempDir,
}

impl World {
    pub async fn new() -> Self {
        Self::with_kinds(|_| {}).await
    }

    /// The world, with the component kinds `register` adds besides the
    /// built-in ones.
    pub async fn with_kinds(register: impl FnOnce(&mut ComponentRegistry)) -> Self {
        Self::with(&[], register).await
    }

    /// The world with `plugins` (issue 323), and the component kinds
    /// `register` adds besides the built-in ones.
    pub async fn with(plugins: &[&dyn Plugin], register: impl FnOnce(&mut ComponentRegistry)) -> Self {
        let roles = Roles::new([(*STAFF, "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        // The policy learns who takes part where from the store it guards
        // (issue 23), bound once the store is made.
        let memberships = Arc::new(StoreMemberships::new());
        let recorder = Arc::new(Recorder { policy: OwnershipSecurityManager::with_memberships(Arc::<StoreMemberships>::clone(&memberships)), rules, asked: Mutex::new(Vec::new()), names: Mutex::new(HashMap::new()) });
        let policy: Arc<dyn SecurityManager> = Arc::<Recorder>::clone(&recorder);
        let store = Arc::new(MemoryStore::with_policies(Arc::clone(&policy), Arc::<Recorder>::clone(&recorder)));
        memberships.bind(&store.entity_stores().groups);
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
        register(&mut registry);
        let files = tempfile::tempdir().unwrap();
        let shell = BackendConfig::new("local", json!({ "root": files.path().join("shell"), "shell": true, "destination": { "level": "internal" } }));
        let workspaces = WorkspacesConfig { storage: [("main".to_string(), BackendConfig::new("local", json!({ "root": files.path() }))), ("shell".to_string(), shell)].into(), ..Default::default() };
        let artifacts = ArtifactsConfig { storage: [("kept".to_string(), BackendConfig::new("local", json!({ "root": files.path().join("artifacts") })))].into(), ..Default::default() };
        let config = AppConfig { workspaces, artifacts, ..Default::default() };
        let mut builder = RuntimeBuilder::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::<Recorder>::clone(&recorder), &config, registry, Backends::with_plugins(plugins)).unwrap();
        for plugin in plugins {
            builder.plugin(*plugin).unwrap();
        }
        let runtime = Arc::new(builder.build().unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let mut world = Self { policy: recorder, store, runtime, manager, alice: UserId::new(), bob: UserId::new(), carol: UserId::new(), files };
        world.alice = world.user("alice").await;
        world.bob = world.user("bob").await;
        world.carol = world.user("carol").await;
        world
    }

    /// A turn of `agent`, of `owner`, in the first channel of `session`,
    /// begun in `state` - for what a component does in a turn.
    pub fn turn<'a>(session: &SessionLoop, agent: AgentId, owner: UserId, state: &'a mut AgentChannelState) -> Turn<'a> {
        let channel = &session.channels()[0];
        let context = SecurityContext::new(ActorId::User(owner), session.id(), channel.id()).next(ActorId::Agent(agent), ActorId::User(owner)).with_roles(vec![*STAFF], None);
        state.begin_turn(EventContext::new(session.id(), channel.id(), ActorId::None, String::new()), context, channel.classification(), Vec::new(), Default::default());
        Turn::of(agent, agent.to_string(), state, AgentStateHandle::detached(), views_in(session, channel, ActorId::Agent(agent))).expect("a turn was begun")
    }

    pub fn system() -> SecurityContext {
        SecurityContext::system()
    }

    /// What `user` does, in their name.
    /// The context of `user` acting for themselves, as their controller
    /// gives it (issue 304): with the role every user of the world holds.
    pub fn as_user(user: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(user), SessionId::default(), ChannelId::default()).with_roles(vec![*STAFF], None)
    }

    pub async fn user(&self, name: &str) -> UserId {
        let user = User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: vec![*STAFF], deleted: None };
        self.runtime.entity_stores().users.create(&Self::system(), &user).await.unwrap();
        self.policy.names.lock().unwrap().insert(ActorId::User(user.id), name.to_string());
        user.id
    }

    /// What the policy was asked in someone's name while `action` ran - a
    /// user's, or an agent's on its owner's behalf - in order. What the
    /// runtime asks in its own name is left out: it may do everything, and
    /// asks only to read what it needs. So is what it asks in someone's name
    /// on its own, while the action runs - whether a command may still run
    /// (issue 254): no action asks it.
    pub async fn asked<T>(&self, action: impl Future<Output = T>) -> (T, Vec<String>) {
        let (result, asked) = self.recorded(action).await;
        (result, asked.into_iter().filter(|line| !line.contains(", asked by system:")).collect())
    }

    /// What the policy was asked in someone's name while `action` ran, by
    /// the action or by the runtime on its own, in order.
    pub async fn recorded<T>(&self, action: impl Future<Output = T>) -> (T, Vec<String>) {
        self.policy.asked.lock().unwrap().clear();
        let result = action.await;
        let asked = std::mem::take(&mut *self.policy.asked.lock().unwrap());
        // Agents made by the action are recorded by their names, too.
        for agent in self.runtime.entity_stores().agents.list(&Self::system(), &Query::all()).await.map(|page| page.items).unwrap() {
            self.policy.names.lock().unwrap().insert(ActorId::Agent(agent.id), agent.name.name.to_string());
        }
        let names = self.policy.names.lock().unwrap().clone();
        let asked = asked.into_iter()
            .filter(|line| !line.starts_with("system:"))
            .map(|line| names.iter().fold(line, |line, (actor, name)| line.replace(&actor.to_string(), name)))
            .collect();
        (result, asked)
    }

    /// A personality of `owner` - the system's if `None` - shared with
    /// `contributors`.
    pub async fn personality(&self, name: &str, owner: Option<UserId>, contributors: &[UserId]) -> Personality {
        let personality = Personality {
            id: PersonalityId::new(),
            name: ScopedName::new(name, Scope::GlobalScope).unwrap(),
            kind: PersonalityKind::Internal,
            roles: None,
            description: Some(Description::new(format!("{name}, privately described")).unwrap()),
            introduction: Some(Introduction::new(format!("I am {name}.")).unwrap()),
            components: Default::default(),
            security_attributes: attributes(owner, contributors),
            reasoning_visibility: Default::default(),
            deleted: None,
        };
        self.runtime.entity_stores().personalities.create(&Self::system(), &personality).await.unwrap();
        personality
    }

    /// A template of `owner` - the system's if `None` - shared with
    /// `contributors`: its owner and an agent `name` of `personality`, of
    /// the owner's scope, in one channel.
    pub async fn template(&self, owner: Option<UserId>, contributors: &[UserId], agent: &str, personality: PersonalityId) -> SessionTemplateId {
        let template = SessionTemplate {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionTemplateId::new(),
            name: ScopedName::new(format!("with-{agent}"), Scope::GlobalScope).unwrap(),
            description: None,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent { name: Name::new(agent).unwrap(), scope: ScopeTemplate::UserScope, personality_id: personality },
            ],
            channels: vec![ChannelTemplate {
                summary: None, briefing: None, language: None,
                approval_rules: Vec::new(),
                uses_workspace: true,
                name: Name::new("main").unwrap(),
                components: quiet(),
                classification: Classification::general(Confidentiality::Internal),
                participants: vec!["user".to_string(), agent.to_string()],
            }],
            security_attributes: attributes(owner, contributors),
            commands: Default::default(),
            privacy: Default::default(),
            deleted: None,
        };
        self.runtime.entity_stores().session_templates.create(&Self::system(), &template).await.unwrap();
        template.id
    }

    /// An agent of `owner` and `personality`, as made before.
    pub async fn agent(&self, name: &str, owner: UserId, personality: &Personality) -> AgentId {
        let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), personality, ActorId::User(owner));
        self.runtime.entity_stores().agents.create(&Self::system(), &agent).await.unwrap();
        self.policy.names.lock().unwrap().insert(ActorId::Agent(agent.id), name.to_string());
        agent.id
    }

    /// A session of alice with bob in it, and a channel "main" of both of
    /// them and `agents`; the session also has `waiting`, in no channel.
    /// Stored as a setup would, then opened by alice.
    pub async fn shared_session(&self, agents: &[AgentId], waiting: &[AgentId]) -> Arc<SessionLoop> {
        self.session_of(&[self.bob], agents, waiting).await
    }

    /// A session of alice with `others` in it, and a channel "main" of
    /// them all and `agents`; the session also has `waiting`, in no
    /// channel. Stored as a setup would, then opened by alice.
    pub async fn session_of(&self, others: &[UserId], agents: &[AgentId], waiting: &[AgentId]) -> Arc<SessionLoop> {
        let alice = ActorId::User(self.alice);
        let users: Vec<Participant> = std::iter::once(self.alice).chain(others.iter().copied()).map(Participant::User).collect();
        let in_channel: Vec<Participant> = users.iter().cloned().chain(agents.iter().map(|a| Participant::Agent(*a))).collect();
        let in_session: Vec<Participant> = in_channel.iter().cloned().chain(waiting.iter().map(|a| Participant::Agent(*a))).collect();
        let channel = Channel::build(SessionId::new(), alice, Name::new("main").unwrap(), quiet(), Classification::general(Confidentiality::Internal), None, in_channel);
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: channel.session_id,
            name: Name::new("shared").unwrap(),
            commands: Default::default(),
            privacy: Default::default(),
            participants: in_session.clone(),
            channels: vec![channel.id],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::owned_by(alice).granting(Subject::Group(GroupId::of_session(channel.session_id)), Grant::Read),
            deleted: None,
        };
        // Each with the group of its participants, as the runtime stores them
        // (issue 286).
        self.runtime.entity_stores().sessions.create(&Self::system(), &session).await.unwrap();
        self.runtime.entity_stores().groups.create(&Self::system(), &Group::of_session(&session)).await.unwrap();
        self.runtime.entity_stores().channels.create(&Self::system(), &channel).await.unwrap();
        self.runtime.entity_stores().groups.create(&Self::system(), &Group::of_channel(&channel)).await.unwrap();
        self.manager.open_session(&Self::as_user(self.alice), session.id).await.expect("alice opens her session")
    }

    /// The ids of the members of `channel` as `user` sees them.
    pub async fn members(&self, user: UserId, session: SessionId, channel: ChannelId) -> Vec<ActorId> {
        let view = self.manager.views(Viewer::new(Self::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
        view.members().await.iter().map(|m| m.id()).collect()
    }
}

/// Owned by `owner` - the system if `None` - read by `readers`.
pub fn attributes(owner: Option<UserId>, readers: &[UserId]) -> SecurityAttributes {
    let owned = SecurityAttributes::owned_by(owner.map(ActorId::User).unwrap_or(ActorId::System));
    readers.iter().fold(owned, |attributes, reader| attributes.granting(Subject::Actor(ActorId::User(*reader)), Grant::Read))
}

/// A channel in which nobody is asked to respond unless mentioned: no agent
/// starts a turn, and no model is called.
pub fn quiet() -> ComponentConfig {
    moderation_components(None, Some("mentions_only_policy"))
}

/// Whether `error` says that what was asked for is not there - which is
/// also what is answered for what the caller may not see or reach.
pub fn not_found(error: &Error) -> bool {
    matches!(error.downcast_ref::<StoreError>(), Some(StoreError::NotFound { .. }))
        || matches!(error.downcast_ref::<WorkspaceError>(), Some(WorkspaceError::NotFound(_)))
        || matches!(error.downcast_ref::<SessionError>(), Some(SessionError::NotFound(_) | SessionError::TemplateNotFound(_)))
}

/// A refusal, with what was refused (issue 323).
pub fn denied(error: &Error) -> bool {
    refusal(error).is_some()
}

/// What `actor` navigates from `channel` of `session`, as its turns do
/// (`ActorRuntime::views`).
pub fn views_in(session: &SessionLoop, channel: &Arc<ChannelLoop>, actor: ActorId) -> ViewsFactory {
    session.views_of(channel, actor)
}

/// The kind of the model the agents of a test talk to unless they name one
/// (`test_registry`).
pub const UNASKED_MODEL_KIND: &str = "unasked_model";

/// The model of a test that asks none: a call fails, as one to a model
/// that cannot be reached.
#[derive(Debug)]
struct Unasked;

#[async_trait]
impl LLMClient for Unasked {
    async fn chat_completion(&self, _: &[Message], _: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>> {
        let refused = LLMError::Connection { endpoint: UNASKED_MODEL_KIND.to_string(), reason: "the model of a test is not to be asked".to_string() };
        futures::stream::iter([Err(refused)]).boxed()
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// The component kinds the tests build with, resolving references to
/// preconfigured components in `component_store`: the built-in ones, and
/// a model that is not to be asked (`UNASKED_MODEL_KIND`) as the default -
/// the core names none, each program names its own (issue 329), and so
/// do the tests.
pub fn test_registry(component_store: Arc<dyn ComponentStore>) -> ComponentRegistry {
    let mut registry = ComponentRegistry::built_in(component_store);
    registry.register(ComponentKind::new(UNASKED_MODEL_KIND).provides::<dyn LLMClient>(), |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn LLMClient>(Arc::new(Unasked));
        Ok(facets)
    });
    registry.set_default_kind::<dyn LLMClient>(UNASKED_MODEL_KIND).expect("registered as providing it");
    registry
}

/// A model that answers its first call with `answer`, and no other.
#[derive(Debug)]
pub struct Once(pub Mutex<Option<Message>>);

#[async_trait]
impl LLMClient for Once {
    async fn chat_completion(&self, _: &[Message], _: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>> {
        match self.0.lock().unwrap().take() {
            Some(answer) => futures::stream::iter([Ok(MessageEvent::Complete(answer))]).boxed(),
            None => futures::stream::pending().boxed(),
        }
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// A model component of the kind `kind` that answers its first call with
/// `answer`, registered with `registry`.
pub fn register_once(registry: &mut ComponentRegistry, kind: &str, answer: Message) {
    let model = Arc::new(Once(Mutex::new(Some(answer))));
    registry.register(ComponentKind::new(kind).provides::<dyn LLMClient>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn LLMClient>(Arc::<Once>::clone(&model));
        Ok(facets)
    });
}

/// Waits until the policy of `world` was asked `line`.
pub async fn asked_for(world: &World, line: &str) {
    timeout(Duration::from_secs(5), async {
        loop {
            let names = world.policy.names.lock().unwrap().clone();
            let asked = world.policy.asked.lock().unwrap().iter()
                .map(|l| names.iter().fold(l.clone(), |l, (actor, name)| l.replace(&actor.to_string(), name)))
                .any(|l| l == line);
            if asked {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("asked");
}

/// What `watch` is shown next of the kind `wanted`.
pub async fn shown(watch: &mut ChannelWatch, wanted: &str) -> ActorEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            let envelope = watch.events.recv().await.expect("the channel runs");
            if envelope.event.kind() == wanted {
                break envelope.event;
            }
        }
    }).await.unwrap_or_else(|_| panic!("{wanted} is shown"))
}
