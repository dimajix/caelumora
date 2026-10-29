//! The world the security suite runs in (issue 198), for the suite and
//! for the tests of a plugin (issue 323), behind the feature
//! `test-support`: the runtime with the ownership policy, recording every
//! question it is asked (`Recorder`), and three users - alice and bob on
//! the staff, carol a stranger to everything of theirs. What the system
//! starts from is put into the store as the system, as a setup would;
//! everything a user does goes through the public API, in that user's name.
//! A test of an action asserts the questions asked in a user's name, in
//! order (`World::asked`): which permissions the action needs.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

mod scripted_agent;
#[cfg(test)]
mod stand_in;

pub use scripted_agent::{Script, ScriptedAgent, ScriptedAgents};
#[cfg(test)]
pub(crate) use stand_in::StandIn;

use crate::actor::user::ChannelWatch;
use crate::agent::AgentError;
use crate::channel::ChannelLoop;
use crate::component::ComponentRegistry;
use crate::controller::WorkspaceController;
use crate::error::refusal;
use crate::plugin::Plugin;
use crate::project::{ProjectError, ProjectManager};
use crate::runtime::{ActorRuntime, RuntimeManager};
use crate::session::{SessionError, SessionLoop, SessionManager};
use crate::terminal::{ByteStream, Terminal, WorkspacePath};
use crate::view::Viewer;
use crate::workspace::WorkspaceError;
use anyhow::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use caelumora_model::actor::ActorEvent;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::testing::make_personality;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::approval::{ApprovalStrategy, Approver};
use caelumora_model::channel::{moderation_components, Channel, InternalActor, Participant};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{
    Classification, Clearance, Compartments, Confidentiality, ConfidentialityPolicy,
    ConfidentialityRules, Place, Reading,
};
use caelumora_model::config::AppConfig;
use caelumora_model::config::{BackendConfig, ContentConfig, WorkspacesConfig};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::role::{RoleId, Roles};
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{
    AccessPolicy, ActionType, Admission, AllowAllPolicy, Grant, OwnershipPolicy, Reclassified,
    SecurityAttributes, SecurityContext, Subject, Verb,
};
use caelumora_model::session::testing::make_session;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::query::Query;
use caelumora_model::store::{EntityKind, Persistence, StoreError};
use caelumora_model::template::testing::{make_channel_template, make_template};
use caelumora_model::template::{
    ChannelTemplate, InternalActorTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate,
    SessionTemplateId,
};
use caelumora_model::user::testing::make_user;
use caelumora_model::user::{User, UserId};
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::value::text::{Description, Introduction};
use caelumora_model::workspace::WorkspaceId;
use futures::stream;
use futures::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::iter;
use std::mem;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

/// The ownership policy, recording every question it is asked to decide
/// (`check`; not the admission a list is narrowed by) - who acted
/// with whose rights and for whom, what, of whose entity, and what it
/// answered - so that a test says which permissions an action needs (issue
/// 198). Recorded as `principal as caller, asked by initiator`, each part
/// only where it differs from those before (issue 255): an agent acting on
/// its own initiates itself (issue 341). `check_each` stays the default, so
/// that the question the event bus asks of every party is recorded for each
/// by `check` (issue 503).
#[derive(Debug)]
pub struct Recorder {
    access_policy: OwnershipPolicy,
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
            other => {
                self.names.lock().unwrap().get(&other).cloned().unwrap_or_else(|| other.to_string())
            }
        }
    }

    /// `action` as recorded: its kind, and whose entity it is about.
    fn label(&self, action: &ActionType<'_>) -> String {
        let kind = action.describe().split('(').next().unwrap_or_default().to_string();
        match action {
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => {
                format!("{kind}({}'s)", self.name(*author))
            }
            ActionType::Reclassify(Reclassified::Message { author, .. }) => {
                format!("{kind}({}'s message)", self.name(*author))
            }
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
            (principal, caller) if caller == principal || caller == ActorId::None => {
                self.name(principal)
            }
            (principal, caller) => format!("{} as {}", self.name(principal), self.name(caller)),
        };
        let who = match context.initiator_id {
            initiator if initiator == context.principal_id || initiator == context.caller_id => {
                acting
            }
            initiator => format!("{acting}, asked by {}", self.name(initiator)),
        };
        let answer = if allowed { "yes" } else { "no" };
        self.asked.lock().unwrap().push(format!("{who}: {what} {answer}"));
    }
}

#[async_trait]
impl AccessPolicy for Recorder {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        let allowed = self.access_policy.check(context, action).await;
        self.record(context, self.label(&action), allowed);
        allowed
    }

    /// The ownership policy's, unrecorded: what a list is narrowed by is
    /// decided again by `check` for whatever it lets through.
    fn admission(
        &self,
        context: &SecurityContext,
        kind: EntityKind,
        verb: Verb,
    ) -> Option<Admission> {
        self.access_policy.admission(context, kind, verb)
    }
}

/// The flow decisions, recorded by the names the actions had before they
/// left the `AccessPolicy` (issue 304): `Publish`, `CopyInformation`,
/// and `JoinChannel` with whom it clears.
impl ConfidentialityPolicy for Recorder {
    fn may_publish(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        let allowed = self.rules.may_publish(context, classification, into);
        self.record(context, "Publish".to_string(), allowed);
        allowed
    }

    /// Recorded as allowed if anything may be copied.
    fn copyable(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        session: &SessionPrivacy,
        into: &Place,
    ) -> Reading {
        let reading = self.rules.copyable(context, channel, session, into);
        self.record(context, "CopyInformation".to_string(), reading != Reading::Nothing);
        reading
    }

    fn cleared(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        roles: &[RoleId],
        classification: &Classification,
    ) -> bool {
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
        | ReadAgentContext(a)
        | ReadAgentTools { agent: a, .. }
        | ReadAgentComponents(a)
        | ApproveToolCall(a)
        | ShowAgentCommand { agent: a, .. }
        | ExecuteAgentCommand { agent: a, .. }
        | StopSession(a)
        | CancelChannel(a)
        | WakeChannel(a)
        | RetryMessage(a)
        | AnswerQuestion(a)
        | ShowCommand { channel: a, .. }
        | ExecuteCommand { channel: a, .. } => Some(a),
        _ => None,
    }
}

/// A runtime of a test, built from its parts as `RuntimeManager::new` builds
/// one (issue 375): each part has a default, and a test names only those it
/// differs in - `TestRuntime { plugins: vec![&Notes], ..Default::default() }`.
/// Its secrets are always none, under its access policy.
pub struct TestRuntime<'a> {
    /// What the entities and the states are kept in: new, empty stores in
    /// memory that permit everything.
    pub store: Persistence,
    /// Allows everything.
    pub access_policy: Arc<dyn AccessPolicy>,
    /// The default rules.
    pub confidentiality: Arc<dyn ConfidentialityPolicy>,
    pub config: AppConfig,
    /// The component kinds; if none, those of `test_registry`, resolving in
    /// `store`.
    pub registry: Option<ComponentRegistry>,
    /// Agents that answer "ok" (`ScriptedAgents`).
    pub plugins: Vec<&'a dyn Plugin>,
}

/// The plugin a `TestRuntime` runs its agents with unless a test names others.
static ANSWERING_OK: LazyLock<ScriptedAgents> =
    LazyLock::new(|| ScriptedAgents(Script::Answers("ok".to_string())));

impl Default for TestRuntime<'_> {
    fn default() -> Self {
        Self {
            store: Persistence::in_memory(),
            access_policy: Arc::new(AllowAllPolicy::new()),
            confidentiality: Arc::new(ConfidentialityRules::default()),
            config: AppConfig::default(),
            registry: None,
            plugins: vec![&*ANSWERING_OK],
        }
    }
}

impl TestRuntime<'_> {
    /// The runtime, for a test that it is built in.
    pub fn build(self) -> Arc<RuntimeManager> {
        Arc::new(self.try_build().expect("the runtime of a test is built"))
    }

    /// The runtime, or why it could not be built - for a test of that.
    ///
    /// # Errors
    /// As `RuntimeManager::new`.
    pub fn try_build(self) -> Result<RuntimeManager> {
        let registry = match self.registry {
            Some(registry) => registry,
            None => test_registry(&self.store),
        };
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&self.access_policy)));
        RuntimeManager::new(
            self.store,
            secrets,
            self.access_policy,
            self.confidentiality,
            &self.config,
            registry,
            &self.plugins,
        )
    }
}

/// The role every user of the world holds.
pub static STAFF: LazyLock<RoleId> = LazyLock::new(RoleId::new);

/// The runtime with the ownership policy, and three users: alice and bob
/// on the staff, carol a stranger to everything of theirs.
pub struct World {
    pub access_policy: Arc<Recorder>,
    pub store: Persistence,
    pub runtime: Arc<RuntimeManager>,
    pub manager: Arc<SessionManager>,
    /// Made beside `manager`, as the programs make it (issue 467).
    pub projects: Arc<ProjectManager>,
    pub alice: UserId,
    pub bob: UserId,
    pub carol: UserId,
    /// Where the storage "main" keeps the workspaces' files, the storage
    /// "shell" in `shell` below it, and the storage of content "kept" what
    /// it keeps in `content` below it (issue 282).
    pub files: TempDir,
}

impl World {
    pub async fn new() -> Self {
        Self::with(&[], |_| {}).await
    }

    /// The world with `plugins` (issue 323), and the component kinds
    /// `register` adds besides the built-in ones. It runs no agent unless a
    /// plugin brings them: in the agents' tests,
    /// `caelumora_agents::testing::TestAgents` does (issues 370, 373).
    pub async fn with(
        plugins: &[&dyn Plugin],
        register: impl FnOnce(&mut ComponentRegistry),
    ) -> Self {
        let roles = Roles::new([(
            *STAFF,
            "staff".to_string(),
            vec![Clearance { level: Confidentiality::Internal, compartment: None }],
        )])
        .unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        // The policy learns who takes part where from the store it guards
        // (issue 23), bound once the store is made.
        let memberships = Arc::new(StoreMemberships::new());
        let recorder = Arc::new(Recorder {
            access_policy: OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(
                &memberships,
            )),
            rules,
            asked: Mutex::new(Vec::new()),
            names: Mutex::new(HashMap::new()),
        });
        let access_policy: Arc<dyn AccessPolicy> = Arc::<Recorder>::clone(&recorder);
        let store = Persistence::in_memory_with(
            Arc::clone(&access_policy),
            Arc::<Recorder>::clone(&recorder),
        );
        memberships.bind(&store.entity_stores().unwrap().groups);
        let mut registry = test_registry(&store);
        register(&mut registry);
        let files = tempfile::tempdir().unwrap();
        let shell = BackendConfig::new(
            "local",
            json!({ "root": files.path().join("shell"), "shell": true, "destination": { "level": "internal" } }),
        );
        let workspaces = WorkspacesConfig {
            storage: [
                ("main".to_string(), BackendConfig::new("local", json!({ "root": files.path() }))),
                ("shell".to_string(), shell),
            ]
            .into(),
            ..Default::default()
        };
        let content = ContentConfig {
            storage: [(
                "kept".to_string(),
                BackendConfig::new("local", json!({ "root": files.path().join("content") })),
            )]
            .into(),
        };
        let config = AppConfig { workspaces, content, ..Default::default() };
        let runtime = TestRuntime {
            store: store.clone(),
            access_policy,
            confidentiality: Arc::<Recorder>::clone(&recorder),
            config,
            registry: Some(registry),
            plugins: plugins.to_vec(),
        }
        .build();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let projects = Arc::new(ProjectManager::new(Arc::clone(&manager)));
        let mut world = Self {
            access_policy: recorder,
            store,
            runtime,
            manager,
            projects,
            alice: UserId::new(),
            bob: UserId::new(),
            carol: UserId::new(),
            files,
        };
        world.alice = world.user("alice").await;
        world.bob = world.user("bob").await;
        world.carol = world.user("carol").await;
        world
    }

    pub fn system() -> SecurityContext {
        SecurityContext::system()
    }

    /// What `user` does, in their name.
    /// The context of `user` acting for themselves, as their controller
    /// gives it (issue 304): with the role every user of the world holds.
    pub fn as_user(user: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(user), SessionId::default(), ChannelId::default())
            .with_roles(vec![*STAFF], None)
    }

    pub async fn user(&self, name: &str) -> UserId {
        let user = User { roles: vec![*STAFF], ..make_user(name) };
        self.runtime.entity_stores().users.create(&Self::system(), &user).await.unwrap();
        self.access_policy.names.lock().unwrap().insert(ActorId::User(user.id), name.to_string());
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
        self.access_policy.asked.lock().unwrap().clear();
        let result = action.await;
        let asked = mem::take(&mut *self.access_policy.asked.lock().unwrap());
        // Agents made by the action are recorded by their names, too.
        for agent in self
            .runtime
            .entity_stores()
            .agents
            .list(&Self::system(), &Query::all())
            .await
            .map(|page| page.items)
            .unwrap()
        {
            self.access_policy
                .names
                .lock()
                .unwrap()
                .insert(ActorId::Agent(agent.id), agent.name.name.to_string());
        }
        let names = self.access_policy.names.lock().unwrap().clone();
        let asked = asked
            .into_iter()
            .filter(|line| !line.starts_with("system:"))
            .map(|line| {
                names
                    .iter()
                    .fold(line, |line, (actor, name)| line.replace(&actor.to_string(), name))
            })
            .collect();
        (result, asked)
    }

    /// A personality of `owner` - the system's if `None` - shared with
    /// `contributors`.
    pub async fn personality(
        &self,
        name: &str,
        owner: Option<UserId>,
        contributors: &[UserId],
    ) -> Personality {
        let personality = Personality {
            description: Some(Description::new(format!("{name}, privately described")).unwrap()),
            introduction: Some(Introduction::new(format!("I am {name}.")).unwrap()),
            security_attributes: attributes(owner, contributors),
            ..make_personality(name)
        };
        self.runtime
            .entity_stores()
            .personalities
            .create(&Self::system(), &personality)
            .await
            .unwrap();
        personality
    }

    /// A template of `owner` - the system's if `None` - shared with
    /// `contributors`: its owner and an agent `name` of `personality`, of
    /// the owner's scope, in one channel.
    pub async fn template(
        &self,
        owner: Option<UserId>,
        contributors: &[UserId],
        agent: &str,
        personality: PersonalityId,
    ) -> SessionTemplateId {
        let template = SessionTemplate {
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent {
                    name: Name::new(agent).unwrap(),
                    scope: ScopeTemplate::UserScope,
                    personality_id: personality,
                },
            ],
            channels: vec![ChannelTemplate {
                components: quiet(),
                participants: vec!["user".to_string(), agent.to_string()],
                internal_actors: vec![InternalActorTemplate::Approver {
                    strategy: ApprovalStrategy::Any,
                }],
                ..make_channel_template("main")
            }],
            security_attributes: attributes(owner, contributors),
            ..make_template(&format!("with-{agent}"))
        };
        self.runtime
            .entity_stores()
            .session_templates
            .create(&Self::system(), &template)
            .await
            .unwrap();
        template.id
    }

    /// An agent of `owner` and `personality`, as made before.
    pub async fn agent(&self, name: &str, owner: UserId, personality: &Personality) -> AgentId {
        let agent = Agent::new(
            ScopedName::new(name, Scope::GlobalScope).unwrap(),
            personality.id,
            ActorId::User(owner),
        );
        self.runtime.entity_stores().agents.create(&Self::system(), &agent).await.unwrap();
        self.access_policy.names.lock().unwrap().insert(ActorId::Agent(agent.id), name.to_string());
        agent.id
    }

    /// A session of alice with bob in it, and a channel "main" of both of
    /// them and `agents`; the session also has `waiting`, in no channel.
    /// Made as `session_of` makes it.
    pub async fn shared_session(
        &self,
        agents: &[AgentId],
        waiting: &[AgentId],
    ) -> Arc<SessionLoop> {
        self.session_of(&[self.bob], agents, waiting).await
    }

    /// A session of alice with `others` in it, and a channel "main" of
    /// them all and `agents`, its founders; the session also has `waiting`,
    /// in no channel. Made by alice as the runtime makes a session
    /// (`SessionManager::create`) - with a project of its own -, then
    /// opened by her.
    pub async fn session_of(
        &self,
        others: &[UserId],
        agents: &[AgentId],
        waiting: &[AgentId],
    ) -> Arc<SessionLoop> {
        let alice = ActorId::User(self.alice);
        let users: Vec<Participant> =
            iter::once(self.alice).chain(others.iter().copied()).map(Participant::User).collect();
        let in_channel: Vec<Participant> =
            users.iter().cloned().chain(agents.iter().map(|a| Participant::Agent(*a))).collect();
        let in_session: Vec<Participant> = in_channel
            .iter()
            .cloned()
            .chain(waiting.iter().map(|a| Participant::Agent(*a)))
            .collect();
        let session = make_session("shared", self.alice);
        let mut channel = Channel::build(
            session.id,
            alice,
            Name::new("main").unwrap(),
            quiet(),
            Classification::general(Confidentiality::Internal),
            None,
        );
        channel.participants = in_channel;
        channel.internal_actors =
            vec![InternalActor::Approver(Approver::new(ApprovalStrategy::Any))];
        let session =
            Session { participants: in_session.clone(), channels: vec![channel.id], ..session };
        let id = session.id;
        self.manager
            .create(&Self::as_user(self.alice), session, vec![channel])
            .await
            .expect("the session is made");
        self.manager.open(&Self::as_user(self.alice), id).await.expect("alice opens her session")
    }

    /// The ids of the participants of `channel` as `user` sees them.
    pub async fn participants(
        &self,
        user: UserId,
        session: SessionId,
        channel: ChannelId,
    ) -> Vec<ActorId> {
        let view = self
            .manager
            .views(Viewer::new(Self::as_user(user)))
            .session(session)
            .await
            .unwrap()
            .channel(channel)
            .await
            .unwrap();
        view.participants().await.iter().map(|m| m.id()).collect()
    }
}

/// Owned by `owner` - the system if `None` - read by `readers`.
pub fn attributes(owner: Option<UserId>, readers: &[UserId]) -> SecurityAttributes {
    let owned = SecurityAttributes::owned_by(owner.map(ActorId::User).unwrap_or(ActorId::System));
    readers.iter().fold(owned, |attributes, reader| {
        attributes.granting(Subject::Actor(ActorId::User(*reader)), Grant::Read)
    })
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
        || matches!(
            error.downcast_ref::<AgentError>(),
            Some(AgentError::NotFound(_) | AgentError::Store(StoreError::NotFound { .. }))
        )
        || matches!(
            error.downcast_ref::<SessionError>(),
            Some(SessionError::NotFound(_) | SessionError::TemplateNotFound(_))
        )
        || matches!(error.downcast_ref::<ProjectError>(), Some(ProjectError::NotFound(_)))
}

/// A refusal, with what was refused (issue 323).
pub fn denied(error: &Error) -> bool {
    refusal(error).is_some()
}

/// The runtime of `actor` in `channel`, as its turns act through it and
/// navigate from it (`ActorRuntime::views`).
pub fn actor_runtime_in(channel: &Arc<ChannelLoop>, actor: ActorId) -> ActorRuntime {
    channel.new_actor_runtime(actor)
}

pub async fn asked_for(world: &World, line: &str) {
    timeout(Duration::from_secs(5), async {
        loop {
            let names = world.access_policy.names.lock().unwrap().clone();
            let asked = world
                .access_policy
                .asked
                .lock()
                .unwrap()
                .iter()
                .map(|l| {
                    names
                        .iter()
                        .fold(l.clone(), |l, (actor, name)| l.replace(&actor.to_string(), name))
                })
                .any(|l| l == line);
            if asked {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("asked");
}

/// What `watch` is shown next of the kind `wanted`.
pub async fn shown(watch: &mut ChannelWatch, wanted: &str) -> ActorEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            let envelope = watch.recv().await.expect("the channel runs");
            if envelope.event.kind() == wanted {
                break envelope.event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{wanted} is shown"))
}

/// The component kinds the tests build with, resolving the bundles a `ref`
/// names in `store`: the runtime's own (`ComponentRegistry::built_in`).
/// Those of an agent and a model are the agents' tests'
/// (`caelumora_agents::testing::test_registry`, issue 370).
pub fn test_registry(store: &Persistence) -> ComponentRegistry {
    ComponentRegistry::built_in(store.entity_stores().unwrap().component_bundles)
}

/// `lines`, as `World::asked` records them.
pub fn checks(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

/// The content of a file to write, as a terminal is given it.
pub fn file(text: &'static str) -> ByteStream {
    Box::pin(stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
}

/// The content of the file `path` of `terminal`, as text.
pub async fn read(terminal: &dyn Terminal, path: &str) -> String {
    let (_, content) = terminal.read(&WorkspacePath::parse(path).unwrap()).await.unwrap();
    let chunks: Vec<_> = content.collect().await;
    String::from_utf8(chunks.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect())
        .unwrap()
}

/// The controller through which what workspace `id` is changes (issue 344).
pub async fn workspace(world: &World, id: WorkspaceId) -> Arc<WorkspaceController> {
    world.runtime.controller_manager().workspace_controller(id).await.expect("the workspace")
}
