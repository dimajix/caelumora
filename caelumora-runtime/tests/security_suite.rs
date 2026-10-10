//! The security suite (issue 198): users with different permissions doing
//! different things, through the whole runtime - the stores checking with
//! the ownership policy, the `SessionManager`, running channels and the
//! views - as a frontend would. What the system starts from is put into the
//! store as the system, as a setup would; everything a user does goes
//! through the public API, in that user's name.
//!
//! The policy records every question it is asked (`Recorder`): a test of
//! an action asserts the questions asked in a user's name, in order - which
//! permissions the action needs. Each test names the scenario or action of
//! issue 198 it checks; the issue says what is expected and why.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Error;
use async_trait::async_trait;
use futures::StreamExt;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId, PersonalityKind};
use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};
use caelumora_model::channel::{moderation_components, Channel, HistoryAccess, Participant};
use caelumora_model::config::{ArtifactStorageConfig, ArtifactsConfig, StorageConfig, WorkspacesConfig};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, StoredComponents};
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::confidentiality::{Classification, Clearance, Compartments, ConfidentialityRules, Confidentiality};
use caelumora_model::role::{RoleId, Roles};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::value::text::{Description, Introduction};
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{ActionType, Grant, Membership, OwnershipSecurityManager, Reclassified, SecurityAttributes, SecurityContext, SecurityManager, Subject};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::{ComponentStore, EntityStore, StateStore, StoreError};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId, TemplateError, WorkspaceTemplate};
use caelumora_model::user::{User, UserId, UserProfile};
use caelumora_model::work::TurnId;
use caelumora_runtime::channel::ChannelError;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry, PRECONFIGURED_REF_KIND};
use caelumora_runtime::external::{ExternalAgent, ExternalAgentResult, ExternalEvent, ExternalStream, OutboundMessage};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop, SessionManager};
use caelumora_runtime::terminal::{ByteStream, Terminal, WorkspacePath};
use caelumora_runtime::view::{ViewError, Viewer};
use caelumora_runtime::workspace::WorkspaceError;
use caelumora_runtime::artifact::ArtifactError;
use caelumora_model::workspace::{WorkspaceChoice, WorkspaceId};

/// The ownership policy, recording every question it is asked - who acted
/// with whose rights and for whom, what, of whose entity, and what it
/// answered - so that a test says which permissions an action needs (issue
/// 198). Recorded as `principal as caller, asked by initiator`, each part
/// only where it differs from the one before (issue 255).
#[derive(Debug)]
struct Recorder {
    policy: OwnershipSecurityManager,
    asked: Mutex<Vec<String>>,
    /// The names actors are recorded by.
    names: Mutex<HashMap<ActorId, String>>,
}

impl Recorder {
    fn name(&self, actor: ActorId) -> String {
        match actor {
            ActorId::System => "system".to_string(),
            other => self.names.lock().unwrap().get(&other).cloned().unwrap_or_else(|| other.to_string()),
        }
    }

    /// `action` as recorded: its kind, and whose entity it is about.
    fn label(&self, action: &ActionType<'_>) -> String {
        let kind = action.describe().split('(').next().unwrap_or_default().to_string();
        match action {
            ActionType::JoinChannel { participant, .. } => format!("{kind}({})", self.name(*participant)),
            ActionType::ModifyMessage { author, .. } | ActionType::RevokeMessage { author, .. } => format!("{kind}({}'s)", self.name(*author)),
            ActionType::Reclassify { target: Reclassified::Message { author, .. }, .. } => format!("{kind}({}'s message)", self.name(*author)),
            _ => match attributes_of(action) {
                Some(attributes) => format!("{kind}({}'s)", self.name(attributes.owner_id)),
                None => kind,
            },
        }
    }
}

#[async_trait]
impl SecurityManager for Recorder {
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>) -> bool {
        let allowed = self.policy.check(context, action).await;
        let acting = match (context.principal_id, context.caller_id) {
            (principal, caller) if caller == principal || caller == ActorId::None => self.name(principal),
            (principal, caller) => format!("{} as {}", self.name(principal), self.name(caller)),
        };
        let who = match context.initiator_id {
            initiator if initiator == context.principal_id => acting,
            initiator => format!("{acting}, asked by {}", self.name(initiator)),
        };
        let answer = if allowed { "yes" } else { "no" };
        self.asked.lock().unwrap().push(format!("{who}: {} {answer}", self.label(&action)));
        allowed
    }

    fn role_id(&self, name: &str) -> Option<RoleId> {
        self.policy.role_id(name)
    }

    fn role_name(&self, id: RoleId) -> Option<String> {
        self.policy.role_name(id)
    }
}

/// The attributes of the entity `action` is about, if it is about one.
fn attributes_of<'a>(action: &ActionType<'a>) -> Option<&'a SecurityAttributes> {
    use ActionType::*;
    match *action {
        GetPersonality(a) | CreatePersonality(a) | UpdatePersonality(a) | DeletePersonality(a)
        | GetSessionTemplate(a) | CreateSessionTemplate(a) | UpdateSessionTemplate(a) | DeleteSessionTemplate(a)
        | CreateAgent(a) | GetAgent(a) | ReadAgentContext(a) | ReadAgentTools { agent: a, .. } | ReadAgentComponents(a) | ApproveToolCall { agent: a, .. } | ShowAgentCommand { agent: a, .. } | ExecuteAgentCommand { agent: a, .. } | UpdateAgent(a) | DeleteAgent(a)
        | CreateSession(a) | GetSession(a) | UpdateSession(a) | DeleteSession(a) | StopSession(a)
        | GetChannel(a) | CreateChannel(a) | UpdateChannel(a) | DeleteChannel(a)
        | GetUser(a) | CreateUser(a) | UpdateUser(a) | DeleteUser(a)
        | GetUserProfile(a) | CreateUserProfile(a) | UpdateUserProfile(a) | DeleteUserProfile(a)
        | CancelChannel(a) | WakeChannel(a) | RetryMessage(a) | AnswerQuestion(a)
        | GetWorkspace(a) | WriteWorkspace(a) | CreateWorkspace(a) | UpdateWorkspace(a) | DeleteWorkspace(a)
        | GetSkill(a) | CreateSkill(a) | UpdateSkill(a) | DeleteSkill(a) | UseSkill(a)
        | GetArtifact(a) | WriteArtifact(a) | CreateArtifact(a) | UpdateArtifact(a) | DeleteArtifact(a)
        | GetComponents(a) | CreateComponents(a) | UpdateComponents(a) | DeleteComponents(a)
        | ReadDeleted(_, a) | Restore(_, a) | Purge(_, a) => Some(a),
        _ => None,
    }
}

/// The role every user of the world holds.
static STAFF: std::sync::LazyLock<RoleId> = std::sync::LazyLock::new(RoleId::new);

/// The runtime with the ownership policy, and three users: alice and bob
/// on the staff, carol a stranger to everything of theirs.
struct World {
    policy: Arc<Recorder>,
    store: Arc<MemoryStore>,
    runtime: Arc<RuntimeManager>,
    manager: Arc<SessionManager>,
    alice: UserId,
    bob: UserId,
    carol: UserId,
    /// Where the storage "main" keeps the workspaces' files.
    files: tempfile::TempDir,
}

impl World {
    async fn new() -> Self {
        Self::with_kinds(|_| {}).await
    }

    /// The world, with the component kinds `register` adds besides the
    /// built-in ones.
    async fn with_kinds(register: impl FnOnce(&mut ComponentRegistry)) -> Self {
        let roles = Roles::new([(*STAFF, "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        // The policy learns who takes part where from the store it guards
        // (issue 23), bound once the store is made.
        let memberships = Arc::new(StoreMemberships::new());
        let recorder = Arc::new(Recorder { policy: OwnershipSecurityManager::with_rules(rules, Arc::<StoreMemberships>::clone(&memberships)), asked: Mutex::new(Vec::new()), names: Mutex::new(HashMap::new()) });
        let policy: Arc<dyn SecurityManager> = Arc::<Recorder>::clone(&recorder);
        let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
        let entities: Arc<dyn EntityStore> = Arc::<MemoryStore>::clone(&store);
        memberships.bind(&entities);
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let mut registry = ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        register(&mut registry);
        let files = tempfile::tempdir().unwrap();
        let shell = StorageConfig::Local { root: files.path().join("shell"), shell: true, destination: Some(Classification::general(Confidentiality::Internal)) };
        let workspaces = WorkspacesConfig { storage: [("main".to_string(), StorageConfig::Local { root: files.path().to_path_buf(), shell: false, destination: None }), ("shell".to_string(), shell)].into(), ..Default::default() };
        let artifacts = ArtifactsConfig { storage: [("kept".to_string(), ArtifactStorageConfig::Local { root: files.path().join("artifacts"), destination: None })].into(), ..Default::default() };
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, &caelumora_model::config::AppConfig { workspaces, artifacts, ..Default::default() }, registry).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let mut world = Self { policy: recorder, store, runtime, manager, alice: UserId::new(), bob: UserId::new(), carol: UserId::new(), files };
        world.alice = world.user("alice").await;
        world.bob = world.user("bob").await;
        world.carol = world.user("carol").await;
        world
    }

    fn system() -> SecurityContext {
        SecurityContext::system()
    }

    /// What `user` does, in their name.
    fn as_user(user: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(user), SessionId::default(), ChannelId::default())
    }

    async fn user(&self, name: &str) -> UserId {
        let user = User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: vec![*STAFF], deleted: None };
        self.store.create_user(&Self::system(), &user).await.unwrap();
        self.policy.names.lock().unwrap().insert(ActorId::User(user.id), name.to_string());
        user.id
    }

    /// What the policy was asked in someone's name while `action` ran - a
    /// user's, or an agent's on its owner's behalf - in order. What the
    /// runtime asks in its own name is left out: it may do everything, and
    /// asks only to read what it needs.
    async fn asked<T>(&self, action: impl std::future::Future<Output = T>) -> (T, Vec<String>) {
        self.policy.asked.lock().unwrap().clear();
        let result = action.await;
        let asked = std::mem::take(&mut *self.policy.asked.lock().unwrap());
        // Agents made by the action are recorded by their names, too.
        for agent in self.store.list_agents(&Self::system()).await.unwrap() {
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
    async fn personality(&self, name: &str, owner: Option<UserId>, contributors: &[UserId]) -> Personality {
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
        self.store.create_personality(&Self::system(), &personality).await.unwrap();
        personality
    }

    /// A template of `owner` - the system's if `None` - shared with
    /// `contributors`: its owner and an agent `name` of `personality`, of
    /// the owner's scope, in one channel.
    async fn template(&self, owner: Option<UserId>, contributors: &[UserId], agent: &str, personality: PersonalityId) -> SessionTemplateId {
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
        self.store.create_session_template(&Self::system(), &template).await.unwrap();
        template.id
    }

    /// An agent of `owner` and `personality`, as made before.
    async fn agent(&self, name: &str, owner: UserId, personality: &Personality) -> AgentId {
        let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), personality, ActorId::User(owner));
        self.store.create_agent(&Self::system(), &agent).await.unwrap();
        self.policy.names.lock().unwrap().insert(ActorId::Agent(agent.id), name.to_string());
        agent.id
    }

    /// A session of alice with bob in it, and a channel "main" of both of
    /// them and `agents`; the session also has `waiting`, in no channel.
    /// Stored as a setup would, then opened by alice.
    async fn shared_session(&self, agents: &[AgentId], waiting: &[AgentId]) -> Arc<SessionLoop> {
        self.session_of(&[self.bob], agents, waiting).await
    }

    /// A session of alice with `others` in it, and a channel "main" of
    /// them all and `agents`; the session also has `waiting`, in no
    /// channel. Stored as a setup would, then opened by alice.
    async fn session_of(&self, others: &[UserId], agents: &[AgentId], waiting: &[AgentId]) -> Arc<SessionLoop> {
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
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::owned_by(alice).granting(Subject::MemberOf(Membership::Session(channel.session_id)), Grant::Read),
            deleted: None,
        };
        self.store.create_session(&Self::system(), &session).await.unwrap();
        self.store.create_channel(&Self::system(), &channel).await.unwrap();
        self.manager.open_session(&Self::as_user(self.alice), session.id).await.expect("alice opens her session")
    }

    /// The ids of the members of `channel` as `user` sees them.
    async fn members(&self, user: UserId, session: SessionId, channel: ChannelId) -> Vec<ActorId> {
        let view = self.manager.views(Viewer::new(Self::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
        view.members().await.iter().map(|m| m.id()).collect()
    }
}

/// Owned by `owner` - the system if `None` - read by `readers`.
fn attributes(owner: Option<UserId>, readers: &[UserId]) -> SecurityAttributes {
    let owned = SecurityAttributes::owned_by(owner.map(ActorId::User).unwrap_or(ActorId::System));
    readers.iter().fold(owned, |attributes, reader| attributes.granting(Subject::Actor(ActorId::User(*reader)), Grant::Read))
}

/// A channel in which nobody is asked to respond unless mentioned: no agent
/// starts a turn, and no model is called.
fn quiet() -> caelumora_model::component::ComponentConfig {
    moderation_components(None, Some("mentions_only_policy"))
}

/// Whether `error` says that what was asked for is not there - which is
/// also what is answered for what the caller may not see.
fn not_found(error: &Error) -> bool {
    matches!(error.downcast_ref::<StoreError>(), Some(StoreError::NotFound { .. }))
}

fn denied(error: &Error) -> bool {
    matches!(error.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied))
        || matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::Forbidden { .. }))
}

// --- Personalities and agents ---------------------------------------------

/// P1: a personality the user may not read makes no agent and no session.
#[tokio::test]
async fn a_session_cannot_be_made_with_a_personality_the_user_may_not_read() {
    let world = World::new().await;
    let bobs = world.personality("bobs-own", Some(world.bob), &[]).await;
    let template = world.template(Some(world.alice), &[], "helper", bobs.id).await;

    let refused = world.manager.create_from_template(&World::as_user(world.alice), template, None, None).await.expect_err("not hers to use");
    assert!(not_found(&refused), "answered as not there, so that it cannot be probed: {refused:#}");
    assert!(world.manager.views(Viewer::new(World::as_user(world.alice))).sessions().await.unwrap().is_empty(), "no session was stored");
    assert!(world.store.list_agents(&World::system()).await.unwrap().is_empty(), "nor an agent");
}

/// P2: a personality shared with the user, or the system's, makes an agent
/// of the user; its owner gets no say over that agent.
#[tokio::test]
async fn a_shared_personality_makes_an_agent_of_whoever_uses_it() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let global = world.personality("global", None, &[]).await;
    for personality in [&shared, &global] {
        let template = world.template(Some(world.alice), &[], personality.name.name.as_str(), personality.id).await;
        let session = world.manager.create_from_template(&World::as_user(world.alice), template, None, None).await.expect("hers to use");
        let agents = world.store.list_agents(&World::as_user(world.alice)).await.unwrap();
        let agent = agents.iter().find(|a| a.personality_id == personality.id).expect("an agent of hers");
        assert_eq!(agent.security_attributes.owner_id, ActorId::User(world.alice), "owned by who made it");
        let gone = world.store.get_agent(&World::as_user(world.bob), agent.id).await.expect_err("not bob's to read");
        assert!(matches!(gone, StoreError::NotFound { .. }), "{gone}");
        let renamed = Agent { name: ScopedName::new("taken", Scope::GlobalScope).unwrap(), ..agent.clone() };
        assert!(world.store.update_agent(&World::as_user(world.bob), &renamed).await.is_err(), "nor his to change, though the personality is his");
        world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}

/// P3: an agent whose owner may no longer read its personality - a share
/// withdrawn - is not run: its session opens without it.
#[tokio::test]
async fn an_agent_whose_personality_was_withdrawn_is_left_out_of_its_channel() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    assert!(world.members(world.alice, session.id(), channel).await.contains(&ActorId::Agent(helper)), "it runs while shared");
    assert!(session.channels()[0].actor_ids().contains(&ActorId::Agent(helper)));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();

    let withdrawn = Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world.store.update_personality(&World::as_user(world.bob), &withdrawn).await.expect("bob owns it");

    let reopened = world.manager.open_session(&World::as_user(world.alice), session.id()).await.expect("the session still opens");
    assert!(!reopened.channels()[0].actor_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    assert!(reopened.channels()[0].actor_ids().contains(&ActorId::User(world.alice)), "everyone else is");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P3b: an agent whose personality is gone is not run either; its session
/// opens without it.
#[tokio::test]
async fn an_agent_whose_personality_is_gone_is_left_out_of_its_channel() {
    let world = World::new().await;
    let bobs = world.personality("bobs", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &bobs).await;
    let session = world.shared_session(&[helper], &[]).await;
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
    world.store.delete_personality(&World::as_user(world.bob), bobs.id).await.expect("bob deletes his personality");

    let reopened = world.manager.open_session(&World::as_user(world.alice), session.id()).await.expect("the session still opens");
    assert!(!reopened.channels()[0].actor_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P4: such an agent cannot be added to a channel either.
#[tokio::test]
async fn an_agent_whose_personality_was_withdrawn_cannot_join_a_channel() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let main = Arc::clone(&session.channels()[0]);

    let withdrawn = Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world.store.update_personality(&World::as_user(world.bob), &withdrawn).await.unwrap();

    let refused = main.add_participant(&World::as_user(world.alice), ActorId::Agent(helper), HistoryAccess::All).await.expect_err("it may not run");
    assert!(denied(&refused), "{refused:#}");
    assert!(!main.actor_ids().contains(&ActorId::Agent(helper)), "the channel is as it was");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P5: a personality is changed and deleted by its owner only; who it is
/// shared with reads it, a stranger does not even find it.
#[tokio::test]
async fn a_personality_is_changed_by_its_owner_and_read_by_who_it_is_shared_with() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let (alice, bob, carol) = (World::as_user(world.alice), World::as_user(world.bob), World::as_user(world.carol));

    assert!(world.store.get_personality(&alice, shared.id).await.is_ok(), "shared with her");
    let changed = Personality { description: Some(Description::new("hers now").unwrap()), ..shared.clone() };
    assert!(world.store.update_personality(&alice, &changed).await.is_err(), "but not hers to change");
    assert!(world.store.delete_personality(&alice, shared.id).await.is_err(), "nor to delete");
    assert!(matches!(world.store.get_personality(&carol, shared.id).await, Err(StoreError::NotFound { .. })), "not there for a stranger");
    assert!(world.store.list_personalities(&carol).await.unwrap().iter().all(|p| p.id != shared.id), "nor listed");
    world.store.update_personality(&bob, &changed).await.expect("its owner changes it");
}

/// P6: what of a personality is public - how its agents introduce
/// themselves - reaches whoever sees the agent, though they may not read
/// the personality.
#[tokio::test]
async fn an_agents_introduction_reaches_whoever_sees_it_but_not_its_personality() {
    let world = World::new().await;
    let hers = world.personality("hers", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &hers).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();

    assert!(world.store.get_personality(&World::as_user(world.bob), hers.id).await.is_err(), "bob may not read the personality");
    let view = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(channel).await.unwrap();
    let members = view.members().await;
    let member = members.iter().find(|m| m.id() == ActorId::Agent(helper)).expect("he sees the agent");
    assert_eq!(member.name(), Some("helper"));
    assert_eq!(member.introduction().await.unwrap().as_deref(), Some("I am hers."), "and how it introduces itself");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P7: the preconfigured components a personality includes (issue 270) -
/// shared with the agent's owner by someone else - are read in the agent's
/// name, as its channel starts, as its personality is; once the share is
/// withdrawn, the agent is not run, but its session opens without it, as
/// with a withdrawn personality (P3).
#[tokio::test]
async fn the_components_a_personality_includes_are_read_in_its_agents_name() {
    let world = World::new().await;
    let tools = StoredComponents::new("bobs-tools", ComponentConfig { component_nodes: vec![ComponentDescriptor::new("calculator", "calculator_tool")] }, attributes(Some(world.bob), &[world.alice]));
    world.store.create_components(&World::as_user(world.bob), &tools).await.expect("bob keeps his tools");
    let mut hers = world.personality("hers", Some(world.alice), &[]).await;
    hers.components = ComponentConfig { component_nodes: vec![ComponentDescriptor { params: serde_json::json!({ "id": "bobs-tools" }), ..ComponentDescriptor::new("tools", PRECONFIGURED_REF_KIND) }] };
    world.store.update_personality(&World::as_user(world.alice), &hers).await.unwrap();
    let helper = world.agent("helper", world.alice, &hers).await;

    let (session, asked) = world.asked(world.shared_session(&[helper], &[])).await;
    let personality = asked.iter().position(|line| line == "alice as helper: GetPersonality(alice's) yes").expect("its personality is read in its name");
    let components = asked.iter().position(|line| line == "alice as helper: GetComponents(bob's) yes").expect("and so are the components it includes");
    assert!(personality < components, "first the personality, then what it includes: {asked:?}");
    assert!(session.channels()[0].actor_ids().contains(&ActorId::Agent(helper)), "it runs");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();

    let withdrawn = StoredComponents { security_attributes: attributes(Some(world.bob), &[]), ..tools };
    world.store.update_components(&World::as_user(world.bob), &withdrawn).await.expect("bob owns them");
    let (reopened, asked) = world.asked(world.manager.open_session(&World::as_user(world.alice), session.id())).await;
    let reopened = reopened.expect("the session still opens: bob cannot stop alice's session so");
    assert!(asked.contains(&"alice as helper: GetComponents(bob's) no".to_string()), "{asked:?}");
    assert!(!reopened.channels()[0].actor_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    assert!(reopened.channels()[0].actor_ids().contains(&ActorId::User(world.alice)), "everyone else is");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Templates --------------------------------------------------------------

/// T1: a template is used by whoever may read it - its owner, who it is
/// shared with, everybody for the system's - and by nobody else.
#[tokio::test]
async fn a_template_is_used_only_by_who_may_read_it() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let bobs = world.template(Some(world.bob), &[], "private", global.id).await;
    let shared = world.template(Some(world.bob), &[world.alice], "shared", global.id).await;
    let systems = world.template(None, &[], "public", global.id).await;
    let alice = World::as_user(world.alice);

    let refused = world.manager.create_from_template(&alice, bobs, None, None).await.expect_err("not hers");
    assert!(not_found(&refused), "{refused:#}");
    for usable in [shared, systems] {
        let session = world.manager.create_from_template(&alice, usable, None, None).await.expect("hers to use");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(world.alice), "the session is hers, not the template's owner's");
        world.manager.stop_session(&alice, session.id()).await.unwrap();
    }
}

// --- Sessions and channels --------------------------------------------------

/// S1: a stranger finds nothing of a session - not that it exists.
#[tokio::test]
async fn a_stranger_finds_nothing_of_a_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let carol = World::as_user(world.carol);

    assert!(not_found(&world.manager.open_session(&carol, session.id()).await.expect_err("not hers to open")));
    assert!(not_found(&world.manager.stop_session(&carol, session.id()).await.expect_err("nor to stop")));
    assert!(not_found(&world.manager.delete_session(&carol, session.id()).await.expect_err("nor to delete")));
    let views = world.manager.views(Viewer::new(carol));
    assert!(matches!(views.session(session.id()).await, Err(ViewError::SessionNotFound(_))));
    assert!(views.sessions().await.unwrap().is_empty(), "nor listed");
    assert!(world.manager.views(Viewer::new(World::as_user(world.alice))).sessions().await.unwrap().len() == 1, "while it is there");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 23: a channel is read by who takes part in it now - wherever they
/// act -, not by a list copied when it was made: a participant removed is
/// told of it, and reads it no more.
#[tokio::test]
async fn a_channel_is_read_by_its_members_wherever_they_act_and_no_more_once_removed() {
    let world = World::new().await;
    let session = world.session_of(&[world.bob, world.carol], &[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let elsewhere = SecurityContext::new(ActorId::User(world.carol), SessionId::new(), ChannelId::new());
    assert!(session.channel_for(&elsewhere, main.id()).await.is_ok(), "a member, though she acts elsewhere");

    let mut events = world.runtime.event_bus().subscribe_as(World::as_user(world.carol));
    main.remove_participant(&World::as_user(world.alice), ActorId::User(world.carol)).await.expect("alice's to remove her");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if matches!(event, caelumora_model::event::GlobalEvent::UserChannelLeft(_, channel, user) if channel == main.id() && user == world.carol) {
                return;
            }
        }
    }).await.expect("she is told she was removed");
    assert!(session.channel_for(&World::as_user(world.carol), main.id()).await.is_err(), "and reads it no more");
    assert!(session.channel_for(&World::as_user(world.bob), main.id()).await.is_ok(), "while the others still do");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// S2: a participant reads and takes part, but changes nothing of the
/// session or its channels.
#[tokio::test]
async fn a_participant_takes_part_but_changes_nothing() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let bob = World::as_user(world.bob);

    world.manager.open_session(&bob, session.id()).await.expect("he opens it");
    assert!(world.members(world.bob, session.id(), main.id()).await.contains(&ActorId::User(world.alice)), "and sees who is in it");
    main.user_actor(world.bob).expect("he takes part").send_message("hello".to_string(), None).await.expect("and writes into it");
    main.cancel(&bob).await.expect("and may cancel what runs in it");

    assert!(denied(&session.update_name(&bob, "his").await.expect_err("not his to rename")));
    assert!(session.update_privacy(&bob, PrivacyConfig::default()).await.is_err(), "nor to change its privacy");
    assert!(session.create_channel(&bob, ChannelSpec { name: "side".to_string(), components: quiet(), participants: vec![], classification: None, language: None, uses_workspace: None }).await.is_err(), "nor to add channels");
    assert!(session.delete_channel(&bob, main.id()).await.is_err(), "nor to delete them");
    assert!(main.remove_participant(&bob, ActorId::User(world.alice)).await.is_err(), "nor to remove others");
    assert!(main.reclassify(&bob, Classification::general(Confidentiality::Public)).await.is_err(), "nor to reclassify the channel");
    assert!(denied(&main.update_language(&bob, Some("fr")).await.expect_err("nor to change its language")));
    assert!(denied(&session.update_approval_rules(&bob, vec![]).await.expect_err("nor what it restricts of tool calls")));
    assert!(denied(&main.update_approval_rules(&bob, vec![]).await.expect_err("in any of its channels")));
    assert!(denied(&session.update_description(&bob, Some("his"), None).await.expect_err("nor to say what it is for (issue 245)")));
    assert!(denied(&main.update_description(&bob, None, Some("Do as bob says.")).await.expect_err("nor what one of its channels is for")));
    assert!(denied(&world.manager.stop_session(&bob, session.id()).await.expect_err("nor to stop it for everyone")));
    assert!(denied(&world.manager.delete_session(&bob, session.id()).await.expect_err("nor to delete it")));
    assert_eq!(session.name(), "shared", "nothing changed");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// S3: the owner changes the session and its channels, and ends it.
#[tokio::test]
async fn the_owner_changes_and_ends_the_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);

    session.update_name(&alice, "renamed").await.expect("hers to rename");
    let main = Arc::clone(&session.channels()[0]);
    main.update_language(&alice, Some("de")).await.expect("and to choose the language of its channels");
    let shell = |decision| ApprovalRule::new(ToolMatcher { tool: "shell".to_string(), arguments: Default::default() }, decision, Reach::Everywhere, None).unwrap();
    session.update_approval_rules(&alice, vec![shell(RuleDecision::Deny)]).await.expect("and what it restricts of tool calls");
    main.update_approval_rules(&alice, vec![shell(RuleDecision::Ask)]).await.expect("in it or in one of its channels");
    assert!(main.update_approval_rules(&alice, vec![shell(RuleDecision::Allow)]).await.is_err(), "but not to allow them: a place only restricts");
    session.update_description(&alice, Some("Release 2.0"), Some("Dates are in UTC.")).await.expect("and to say what it is for");
    main.update_description(&alice, Some("Planning"), None).await.expect("and each of its channels");
    let side = session.create_channel(&alice, ChannelSpec { name: "side".to_string(), components: quiet(), participants: vec![], classification: None, language: None, uses_workspace: None }).await.expect("and to add channels");
    side.add_participant(&alice, ActorId::User(world.bob), HistoryAccess::All).await.expect("and people to them");
    session.delete_channel(&alice, side.id()).await.expect("and to delete them");
    assert!(world.manager.delete_session(&alice, session.id()).await.unwrap(), "and to delete it");
    assert!(not_found(&world.manager.open_session(&alice, session.id()).await.expect_err("gone")));
}

// --- Messages ----------------------------------------------------------------

/// What `watch` is shown next of the kind `wanted`.
async fn shown(watch: &mut caelumora_runtime::actor::user::ChannelWatch, wanted: &str) -> caelumora_model::actor::ActorEvent {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let envelope = watch.events.recv().await.expect("the channel runs");
            if envelope.event.kind() == wanted {
                break envelope.event;
            }
        }
    }).await.unwrap_or_else(|_| panic!("{wanted} is shown"))
}

/// S4: cancelling one actor (issue 235) is cancelling, asked as
/// `CancelChannel` of the channel: whoever may stop the channel may stop
/// one of its actors - bob as a participant - and who may not is refused
/// before anything of the actor is looked at, so that he does not learn who
/// is in the channel. It is offered of an agent by the same question (issue
/// 243).
#[tokio::test]
async fn cancelling_one_actor_is_for_whoever_may_cancel_the_channel() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (bob, carol) = (World::as_user(world.bob), World::as_user(world.carol));
    world.manager.open_session(&bob, session.id()).await.expect("he opens it");

    let (cancelled, asked) = world.asked(main.cancel_actor(&bob, ActorId::User(world.alice))).await;
    cancelled.expect("his to cancel, as a participant");
    assert_eq!(asked, checks(&["bob: CancelChannel(alice's) yes"]), "asked as cancelling the channel");

    let nobody = ActorId::User(UserId::new());
    let (refused, asked) = world.asked(main.cancel_actor(&carol, nobody)).await;
    assert!(denied(&refused.expect_err("not hers to cancel")), "refused, not told that nobody of that id is there");
    assert_eq!(asked, checks(&["carol: CancelChannel(alice's) no"]));

    let view = world.manager.views(Viewer::new(bob)).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
    let member = view.member(ActorId::Agent(helper)).await.expect("the agent is a member");
    let (offered, asked) = world.asked(member.may_cancel()).await;
    assert!(offered, "offered to him, of the agent");
    assert_eq!(asked, checks(&["bob: CancelChannel(alice's) yes"]), "by the same question");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M1: a message is its author's to modify and to take back (issue 234) -
/// not another participant's. Asked by the author's actor before it sends
/// the change, and again by the channel as it admits it, with whether the
/// modified message may enter the channel as classified.
#[tokio::test]
async fn a_message_is_modified_and_taken_back_by_its_author_alone() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let mut watch = bob.watch();
    alice.send_message("helo".to_string(), None).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) = shown(&mut watch, "CompleteMessageGenerated").await else { unreachable!() };
    let said = said.id;

    let (refused, asked) = world.asked(bob.modify_message(said, "bob's".to_string(), None)).await;
    assert!(denied(&refused.expect_err("not his")));
    assert_eq!(asked, checks(&["bob: ModifyMessage(alice's) no"]), "refused before anything else is asked");
    let (refused, asked) = world.asked(bob.revoke_message(said)).await;
    assert!(denied(&refused.expect_err("not his to take back")));
    assert_eq!(asked, checks(&["bob: RevokeMessage(alice's) no"]));

    let (_, asked) = world.asked(async {
        alice.modify_message(said, "hello".to_string(), None).await.expect("hers");
        shown(&mut watch, "MessageModified").await;
    }).await;
    assert_eq!(asked, checks(&[
        // Her actor, before it sends.
        "alice: ModifyMessage(alice's) yes", "alice: Publish yes",
        // The channel, as it admits it.
        "alice: Publish yes", "alice: ModifyMessage(alice's) yes",
    ]));
    let (_, asked) = world.asked(async {
        alice.revoke_message(said).await.expect("hers");
        shown(&mut watch, "MessageRevoked").await;
    }).await;
    assert_eq!(asked, checks(&["alice: RevokeMessage(alice's) yes", "alice: RevokeMessage(alice's) yes"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M3: a message is reclassified (issues 133, 239) by its author, or by the
/// channel's owner - not by another participant. Asked as `Reclassify` of
/// the message's author by the actor that sends it, and again by the
/// channel as it admits it; nothing else, since nothing of the message's
/// content enters the channel again.
#[tokio::test]
async fn a_message_is_reclassified_by_its_author_or_the_channels_owner() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let (mut hers, mut his) = (alice.watch(), bob.watch());
    alice.send_message("from alice".to_string(), None).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) = shown(&mut his, "CompleteMessageGenerated").await else { unreachable!() };
    let alices = said.id;
    bob.send_message("from bob".to_string(), None).await.expect("sent");
    // Her actor shows her what she says, too; his message is what the
    // channel routes to her.
    let bobs = loop {
        let ActorEvent::CompleteMessageGenerated(said) = shown(&mut hers, "CompleteMessageGenerated").await else { unreachable!() };
        if said.sender_id == ActorId::User(world.bob) {
            break said.id;
        }
    };
    let public = Classification::general(Confidentiality::Public);

    let (refused, asked) = world.asked(bob.reclassify_message(alices, public.clone())).await;
    assert!(denied(&refused.expect_err("not his")));
    assert_eq!(asked, checks(&["bob: Reclassify(alice's message) no"]), "refused before anything else is asked");

    let (_, asked) = world.asked(async {
        bob.reclassify_message(bobs, public.clone()).await.expect("his own");
        shown(&mut hers, "MessageReclassified").await;
    }).await;
    assert_eq!(asked, checks(&["bob: Reclassify(bob's message) yes", "bob: Reclassify(bob's message) yes"]), "his actor, then the channel");

    let internal = Classification::general(Confidentiality::Internal);
    let (_, asked) = world.asked(async {
        alice.reclassify_message(bobs, internal.clone()).await.expect("her channel");
        // His actor showed him his own before.
        while !matches!(shown(&mut his, "MessageReclassified").await, ActorEvent::MessageReclassified { classification, .. } if classification == internal) {}
    }).await;
    assert_eq!(asked, checks(&["alice: Reclassify(bob's message) yes", "alice: Reclassify(bob's message) yes"]), "the owner, another's");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M2: asking an agent to answer again (issue 234) is an act on the
/// conversation, like cancelling it: whoever takes part may - bob as well
/// as alice - asked as `RetryMessage` of the channel before anything of the
/// message is looked at but that it is there. Whether the agent answers
/// again is its own to decide; a message no agent said is refused after
/// the question. An answer under way (issue 235), or the agent's last turn
/// given up (issue 259), is asked for again under the same question, asked
/// before anything of the agent or the history is looked at.
/// It is offered of an agent by the same question (issue 242).
#[tokio::test]
async fn asking_an_agent_to_answer_again_is_for_whoever_takes_part() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let mut watch = bob.watch();
    alice.send_message("hello".to_string(), None).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) = shown(&mut watch, "CompleteMessageGenerated").await else { unreachable!() };

    let (refused, asked) = world.asked(bob.request_retry(said.id)).await;
    assert!(matches!(refused.expect_err("no agent said it").downcast_ref::<ChannelError>(), Some(ChannelError::NotAnAgentsMessage { .. })));
    assert_eq!(asked, checks(&["bob: RetryMessage(alice's) yes"]), "his to ask, as a participant");
    let (refused, asked) = world.asked(bob.request_retry_of_turn(ActorId::User(world.alice), TurnId::new())).await;
    assert!(matches!(refused.expect_err("no agent answers").downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })));
    assert_eq!(asked, checks(&["bob: RetryMessage(alice's) yes"]), "an answer under way by the same question");
    let (refused, asked) = world.asked(bob.request_retry_of_turn(ActorId::Agent(helper), TurnId::new())).await;
    assert!(matches!(refused.expect_err("neither under way nor given up").downcast_ref::<ChannelError>(), Some(ChannelError::TurnNotRetryable { .. })));
    assert_eq!(asked, checks(&["bob: RetryMessage(alice's) yes"]), "a turn given up by the same question, before the history is looked at (issue 259)");
    let view = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
    let member = view.member(ActorId::Agent(helper)).await.expect("the agent is a member");
    let (offered, asked) = world.asked(member.may_retry()).await;
    assert!(offered, "and offered to him, of the agent (issue 242)");
    assert_eq!(asked, checks(&["bob: RetryMessage(alice's) yes"]), "by the same question");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M4: nudging an agent (issue 278) asks it to respond without a message,
/// as an `@`-mention in a message would: whoever takes part may - bob as
/// well as alice -, and nothing is asked in his name; someone who does not
/// take part has no actor to nudge it through. The agent responds for its
/// owner, on the word of whoever nudged it.
#[tokio::test]
async fn nudging_an_agent_is_for_whoever_takes_part_and_it_responds_on_their_word() {
    let world = World::with_kinds(|registry| register_once(registry, "silent", caelumora_model::message::Message::new(Classification::PUBLIC))).await;
    let silent = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "silent")] },
        ..world.personality("silent", None, &[]).await
    };
    world.store.update_personality(&World::system(), &silent).await.unwrap();
    let helper = world.agent("helper", world.alice, &silent).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let bob = main.user_actor(world.bob).unwrap();
    let mut his = bob.watch();

    let (nudged, asked) = world.asked(bob.request_response(ActorId::Agent(helper))).await;
    nudged.expect("his to nudge, as a participant");
    assert!(asked.is_empty(), "nothing beyond taking part: {asked:?}");
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let envelope = his.events.recv().await.expect("the channel runs");
            if envelope.event.kind() == "TurnFinished" {
                break envelope;
            }
        }
    }).await.expect("the agent responds");
    let turn = finished.security;
    assert_eq!((turn.caller_id, turn.principal_id, turn.initiator_id), (ActorId::Agent(helper), ActorId::User(world.alice), ActorId::User(world.bob)), "for alice, on his word");
    assert!(main.user_actor(world.carol).is_none(), "carol takes no part, and has no actor to nudge through");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- What an agent saw --------------------------------------------------------

/// A1: what an agent saw is its owner's to read alone - not even who reads
/// the channel with it.
#[tokio::test]
async fn what_an_agent_saw_is_its_owners_alone() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    assert!(hers.may_read_context().await, "offered to its owner");
    assert!(hers.context().await.is_ok(), "who reads it");
    let his = member(world.bob).await;
    assert!(!his.may_read_context().await, "not offered to bob");
    assert!(matches!(his.context().await, Err(ViewError::Forbidden { .. })), "nor read by him");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent's private notes across the channels of a session (issue 233)
/// are read as what it saw: by its owner, not by another member.
#[tokio::test]
async fn an_agents_private_notes_are_its_owners_alone() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let notes = |user| {
        let world = &world;
        let session = session.id();
        async move { world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().private_notes_of(helper).await }
    };

    assert!(notes(world.alice).await.is_ok(), "read by its owner");
    assert!(matches!(notes(world.bob).await, Err(ViewError::Forbidden { .. })), "not by bob, who takes part");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What an agent can do now - its tools (issue 194) - is read by every
/// member of the channel it works in, not only by its owner; not by
/// whoever is no member.
#[tokio::test]
async fn which_tools_an_agent_has_is_read_by_the_members_of_its_channel() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    let (tools, asked) = world.asked(hers.tool_sets()).await;
    assert!(tools.is_ok(), "read from its running actor");
    assert_eq!(asked, checks(&["alice: ReadAgentTools(alice's) yes"]));
    let his = member(world.bob).await;
    assert!(his.may_read_tools().await, "offered to bob, who takes part in the channel");
    let (tools, asked) = world.asked(his.tool_sets()).await;
    assert!(tools.is_ok(), "and read by him");
    assert_eq!(asked, checks(&["bob: ReadAgentTools(alice's) yes"]));
    let theirs = world.manager.views(Viewer::new(World::as_user(world.carol))).session(session.id()).await;
    assert!(theirs.is_err(), "carol, who is no member, does not even reach the channel");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What an agent was built from - its components, the preconfigured ones
/// it includes resolved (issue 270) - is read by its owner alone: it is
/// configuration, and what it includes may have been shared with the owner
/// alone. Not by bob, who takes part in the channel and sees its tools.
#[tokio::test]
async fn which_components_an_agent_was_built_from_is_read_by_its_owner_alone() {
    let world = World::new().await;
    let tools = StoredComponents::new("bobs-tools", ComponentConfig { component_nodes: vec![ComponentDescriptor::new("calculator", "calculator_tool")] }, attributes(Some(world.bob), &[world.alice]));
    world.store.create_components(&World::as_user(world.bob), &tools).await.unwrap();
    let mut hers = world.personality("hers", Some(world.alice), &[]).await;
    hers.components = ComponentConfig { component_nodes: vec![
        ComponentDescriptor::new("stop", "stop_tool"),
        ComponentDescriptor { params: serde_json::json!({ "id": "bobs-tools" }), ..ComponentDescriptor::new("tools", PRECONFIGURED_REF_KIND) },
    ] };
    world.store.update_personality(&World::as_user(world.alice), &hers).await.unwrap();
    let helper = world.agent("helper", world.alice, &hers).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    assert!(hers.may_read_components().await, "offered to alice, its owner");
    let (components, asked) = world.asked(hers.components()).await;
    let ids: Vec<String> = components.expect("read from its running actor").component_nodes.into_iter().map(|node| node.id).collect();
    assert_eq!(ids, ["stop", "calculator"], "resolved, as it was built");
    assert_eq!(asked, checks(&["alice: ReadAgentComponents(alice's) yes"]));
    let his = member(world.bob).await;
    assert!(!his.may_read_components().await, "not offered to bob");
    let (components, asked) = world.asked(his.components()).await;
    assert!(matches!(components, Err(ViewError::Forbidden { .. })), "nor read by him, though he sees its tools");
    assert_eq!(asked, checks(&["bob: ReadAgentComponents(alice's) no"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Approvals ---------------------------------------------------------------

/// A model that answers its first call with `answer`, and no other.
#[derive(Debug)]
struct Once(Mutex<Option<caelumora_model::message::Message>>);

#[async_trait]
impl caelumora_runtime::llm::LLMClient for Once {
    async fn chat_completion(&self, _: &[caelumora_model::message::Message], _: &[caelumora_model::tool::Tool])
        -> futures::stream::BoxStream<'static, caelumora_runtime::llm::LLMResult<caelumora_model::message::MessageEvent>>
    {
        match self.0.lock().unwrap().take() {
            Some(answer) => futures::stream::iter([Ok(caelumora_model::message::MessageEvent::Complete(answer))]).boxed(),
            None => futures::stream::pending().boxed(),
        }
    }

    fn profile(&self) -> caelumora_model::context::ModelProfile {
        Default::default()
    }
}

/// Waits until the policy was asked `line`.
async fn asked_for(world: &World, line: &str) {
    let line = checks(&[line]).remove(0);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let names = world.policy.names.lock().unwrap().clone();
            let asked = world.policy.asked.lock().unwrap().iter()
                .map(|l| names.iter().fold(l.clone(), |l, (actor, name)| l.replace(&actor.to_string(), name)))
                .any(|l| l == line);
            if asked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("asked");
}

/// alice's agent `helper`, which calls `sleep` (a call that needs approval)
/// once it is asked, in a session alice shares with bob and he opened;
/// with the session's main channel.
async fn with_a_call_to_approve() -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>, AgentId) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({ "seconds": 0 }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with_kinds(|registry| registry.register(ComponentKind::new("once").provides::<dyn caelumora_runtime::llm::LLMClient>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn caelumora_runtime::llm::LLMClient>(Arc::<Once>::clone(&model));
        Ok(facets)
    })).await;
    let asking = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "once"),
            ComponentDescriptor {
                params: serde_json::json!({ "requires_approval": true }),
                ..ComponentDescriptor::new("sleep", caelumora_runtime::tool::sleep_tool::SLEEP_TOOL_KIND)
            },
        ] },
        ..world.personality("asking", None, &[]).await
    };
    world.store.update_personality(&World::system(), &asking).await.unwrap();
    let helper = world.agent("helper", world.alice, &asking).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (world, session, main, helper)
}

/// A tool that asks the agent's owner its name before it runs (issue 165),
/// the question `sensitive` or not.
#[derive(Debug)]
struct AskingOwner {
    sensitive: bool,
}

#[async_trait]
impl caelumora_runtime::tool::ToolProvider for AskingOwner {
    fn description(&self) -> String {
        "asks its owner".to_string()
    }
    fn tools(&self) -> Vec<caelumora_model::tool::Tool> {
        vec![caelumora_model::tool::Tool {
            name: "asking".to_string(),
            description: "asks its owner".to_string(),
            parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    async fn execute(&self, _: &str, _: &serde_json::Value, answers: Option<&caelumora_model::question::Answers>, _: &mut caelumora_runtime::agent::turn::Turn) -> caelumora_runtime::tool::ToolResult<caelumora_runtime::tool::ToolOutput> {
        Ok(serde_json::json!({ "answered": answers.is_some() }).into())
    }
    fn questions(&self, _: &str, _: &serde_json::Value) -> caelumora_runtime::tool::ToolResult<Option<caelumora_model::question::Questions>> {
        use caelumora_model::question::{Addressee, Question, Questions};
        Ok(Some(Questions {
            questions: vec![Question { header: "Name".to_string(), question: "Who are you?".to_string(), choices: Vec::new(), multi_select: false }],
            ask: vec![Addressee::Owner],
            sensitive: self.sensitive,
        }))
    }
}

/// alice's agent `helper`, which calls `asking` - a tool that asks alice,
/// its owner, before it runs - once `asker` asks it, in a session alice
/// shares with bob and he opened; with the session's main channel, the
/// questions as alice is shown them, and what bob was shown since before
/// it was asked.
async fn with_a_call_asking(sensitive: bool, asker: fn(&World) -> UserId) -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>, caelumora_model::question::QuestionRequest, caelumora_runtime::actor::user::ChannelWatch) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "asking".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with_kinds(|registry| {
        registry.register(ComponentKind::new("once").provides::<dyn caelumora_runtime::llm::LLMClient>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn caelumora_runtime::llm::LLMClient>(Arc::<Once>::clone(&model));
            Ok(facets)
        });
        registry.register(ComponentKind::new("asking_owner").provides::<dyn caelumora_runtime::tool::ToolProvider>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn caelumora_runtime::tool::ToolProvider>(Arc::new(AskingOwner { sensitive }));
            Ok(facets)
        });
    }).await;
    let asking = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "once"),
            ComponentDescriptor::new("asking", "asking_owner"),
        ] },
        ..world.personality("asking", None, &[]).await
    };
    world.store.update_personality(&World::system(), &asking).await.unwrap();
    let helper = world.agent("helper", world.alice, &asking).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut watch = main.user_actor(world.alice).expect("she takes part").watch();
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    main.user_actor(asker(&world)).unwrap().send_message("@helper ask".to_string(), None).await.expect("sent");
    let caelumora_model::actor::ActorEvent::QuestionRequested(request) = shown(&mut watch, "QuestionRequested").await else { unreachable!() };
    (world, session, main, request, his)
}

/// The questions of `helper`'s calls in `main`, as `user` is shown them in
/// a snapshot.
async fn questions_shown(world: &World, session: SessionId, main: ChannelId, user: UserId) -> Vec<caelumora_model::question::QuestionId> {
    use caelumora_model::snapshot::Detail;
    let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(main).await.unwrap();
    view.snapshot(Detail::MEMBERS | Detail::APPROVALS).await.members.into_iter().flat_map(|m| m.pending_questions).map(|q| q.id).collect()
}

/// Answering the questions of an agent's call (issue 165) is asked as
/// `AnswerQuestion` of the channel, of whoever answers - bob may, as a
/// participant - and counts only of those the questions were put to: bob,
/// shown alice's questions, is refused, alice answers.
#[tokio::test]
async fn questions_are_answered_by_whom_they_are_put_to() {
    use caelumora_model::question::{QuestionError, Response};
    let (world, session, main, request, _) = with_a_call_asking(false, |world| world.alice).await;
    assert_eq!(request.questions.asked(), [ActorId::User(world.alice)], "the agent's owner");
    assert_eq!(questions_shown(&world, session.id(), main.id(), world.bob).await, [request.id], "everyone taking part sees what the agent waits for");
    let name = || Some(vec![Response::Text { text: "Alice".to_string() }]);

    let (refused, asked) = world.asked(main.user_actor(world.bob).unwrap().answer_questions(request.id, name())).await;
    let refused = refused.expect_err("not put to him");
    assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: QuestionError::NotAsked, .. })), "{refused}");
    assert_eq!(asked, checks(&["bob: AnswerQuestion(alice's) yes"]), "his to answer as a participant, were he asked");

    let (answered, asked) = world.asked(main.user_actor(world.alice).unwrap().answer_questions(request.id, name())).await;
    answered.expect("put to her");
    assert_eq!(asked, checks(&["alice: AnswerQuestion(alice's) yes"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Sensitive questions (issue 165) are shown to those they are put to
/// alone: bob, taking part, neither is sent them nor sees them in a
/// snapshot, and cannot answer them.
#[tokio::test]
async fn sensitive_questions_reach_only_whom_they_are_put_to() {
    use caelumora_model::question::{QuestionError, Response};
    let (world, session, main, request, mut his) = with_a_call_asking(true, |world| world.alice).await;
    assert_eq!(questions_shown(&world, session.id(), main.id(), world.alice).await, [request.id]);
    assert!(questions_shown(&world, session.id(), main.id(), world.bob).await.is_empty(), "not in his snapshot");

    let refused = main.user_actor(world.bob).unwrap().answer_questions(request.id, Some(vec![Response::Text { text: "Bob".to_string() }])).await.expect_err("never shown to him");
    assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: QuestionError::NotOpen, .. })), "{refused}");
    main.user_actor(world.alice).unwrap().answer_questions(request.id, Some(vec![Response::Text { text: "Alice".to_string() }])).await.expect("put to her");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    while let Ok(envelope) = his.events.try_recv() {
        assert!(!matches!(envelope.event, caelumora_model::actor::ActorEvent::QuestionRequested(_) | caelumora_model::actor::ActorEvent::QuestionClosed { .. }), "nothing of them reaches him: {}", envelope.event.kind());
    }
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The owner a call's questions are put to (`Addressee::Owner`, issue 165)
/// is the agent's owner whoever asked the agent (issue 255): bob asks
/// alice's agent, and alice is asked - a tool that asks its owner for a
/// password never asks whoever happens to talk to the agent.
#[tokio::test]
async fn questions_for_the_owner_are_put_to_the_agents_owner_whoever_asked() {
    let (world, session, _, request, _) = with_a_call_asking(false, |world| world.bob).await;
    assert_eq!(request.questions.asked(), [ActorId::User(world.alice)], "the agent's owner, not bob who asked");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What came of asking `helper` in `main`, as `user` follows it: the
/// request the approver put up - `None` if none within a while, as for a
/// call refused before anyone is asked - the rule `user`'s actor answered it
/// by, if one did, and the approver's decision, if it decided within a
/// while.
struct Asked {
    request: Option<caelumora_model::approval::ApprovalRequest>,
    by_rule: Option<caelumora_model::approval::ApprovalRuleId>,
    approved: Option<bool>,
}

async fn ask_helper(world: &World, main: &caelumora_runtime::channel::ChannelLoop, user: UserId) -> Asked {
    use caelumora_model::actor::ActorEvent;
    let mut watch = main.user_actor(user).expect("they take part").watch();
    main.user_actor(world.alice).unwrap().send_message("@helper wait".to_string(), None).await.expect("sent");
    let mut asked = Asked { request: None, by_rule: None, approved: None };
    let _ = tokio::time::timeout(std::time::Duration::from_millis(1500), async {
        loop {
            let envelope = watch.events.recv().await.expect("the channel runs");
            let from_approver = matches!(envelope.context.sender_id, ActorId::Approver(_));
            match envelope.event {
                ActorEvent::ApprovalRequested(request) if from_approver => asked.request = Some(request),
                ActorEvent::ApprovalGranted(answer) | ActorEvent::ApprovalDenied(answer) if envelope.context.sender_id == ActorId::User(user) => asked.by_rule = answer.by_rule,
                ActorEvent::ApprovalGranted(_) if from_approver => { asked.approved = Some(true); break }
                ActorEvent::ApprovalDenied(_) if from_approver => { asked.approved = Some(false); break }
                _ => {}
            }
        }
    }).await;
    asked
}

fn sleeping(decision: RuleDecision, reach: Reach) -> ApprovalRule {
    ApprovalRule::new(ToolMatcher { tool: "sleep".to_string(), arguments: Default::default() }, decision, reach, None).unwrap()
}

/// A person's rule (issue 13) is their answer given in advance: it answers
/// a request for them, through their actor, as their click would, and the
/// approver decides on it as on any answer of theirs. Only they set it.
#[tokio::test]
async fn a_persons_rule_answers_for_them_as_they_would() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    let rule = sleeping(RuleDecision::Allow, Reach::Session(session.id()));
    assert!(denied(&anyhow::Error::new(hers.add_approval_rule(&World::as_user(world.bob), rule.clone()).await.expect_err("not his to set"))));
    hers.add_approval_rule(&World::as_user(world.alice), rule.clone()).await.expect("hers");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_some_and(|r| !r.explicit), "the call is put up for approval");
    assert_eq!(asked.by_rule, Some(rule.id), "her actor answers by her rule, and shows her so");
    assert_eq!(asked.approved, Some(true), "and the approver decides on it");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A rule answers only where its person's own answer would count: bob's
/// rule does not answer for alice's agent, whose calls he may not approve.
#[tokio::test]
async fn a_rule_answers_only_where_its_person_may_approve() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let his = world.runtime.controller_manager().user_controller(world.bob).await.unwrap();
    his.add_approval_rule(&World::as_user(world.bob), sleeping(RuleDecision::Allow, Reach::Everywhere)).await.expect("his");

    let asked = ask_helper(&world, &main, world.bob).await;
    assert!(asked.request.is_some(), "the call is put up for approval");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "his rule does not answer, and nothing is decided");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A rule answers only within its reach: one for another session or
/// another channel does not.
#[tokio::test]
async fn a_rule_out_of_its_reach_does_not_answer() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    let alice = World::as_user(world.alice);
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Session(SessionId::new()))).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Channel(ChannelId::new()))).await.unwrap();

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_some());
    assert_eq!((asked.by_rule, asked.approved), (None, None), "neither answers");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What a place restricts outweighs what a person allows: a channel's
/// `ask` leaves the call to her own decision, and a session's `deny`
/// refuses it before anyone is asked.
#[tokio::test]
async fn a_rule_of_a_place_outweighs_a_persons_rule() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Everywhere)).await.unwrap();
    main.update_approval_rules(&alice, vec![sleeping(RuleDecision::Ask, Reach::Everywhere)]).await.expect("her channel");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.as_ref().is_some_and(|r| r.explicit), "asked for explicitly");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "her rule does not answer");
    world.manager.stop_session(&alice, session.id()).await.unwrap();

    let (world, session, main, _) = with_a_call_to_approve().await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Everywhere)).await.unwrap();
    session.update_approval_rules(&alice, vec![sleeping(RuleDecision::Deny, Reach::Everywhere)]).await.expect("her session");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_none(), "nobody is asked");
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}

/// Approving a tool call (issue 14) is for who is responsible for the
/// agent - its owner and contributors: alice's answer counts, bob's, who
/// only takes part, is left out. The approver asks in the context each
/// answer comes with, and of everybody taking part to know whom to wait
/// for; the view offers it to alice alone.
#[tokio::test]
async fn a_tool_call_is_approved_by_who_is_responsible_for_the_agent() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::approval::ApprovalResponse;

    let (world, session, main, helper) = with_a_call_to_approve().await;
    let mut watch = main.user_actor(world.alice).expect("she takes part").watch();

    main.user_actor(world.alice).unwrap().send_message("@helper wait".to_string(), None).await.expect("sent");
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let ActorEvent::ApprovalRequested(request) = watch.events.recv().await.expect("the channel runs").event {
                break request;
            }
        }
    }).await.expect("the call is put up for approval");

    let view = |user| {
        let world = &world;
        let session = session.id();
        let channel = main.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };
    let (offered, asked) = world.asked(async { (view(world.alice).await.may_approve().await, view(world.bob).await.may_approve().await) }).await;
    assert_eq!(offered, (true, false), "offered to alice alone");
    let approving: Vec<&String> = asked.iter().filter(|line| line.contains("ApproveToolCall")).collect();
    assert_eq!(approving, ["alice: ApproveToolCall(alice's) yes", "bob: ApproveToolCall(alice's) no"]);

    let response = ApprovalResponse::new(request.id, helper);
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_approval(response.clone(), false).await.expect("sent");
        asked_for(&world, "bob: ApproveToolCall(alice's) no").await;
        main.user_actor(world.alice).unwrap().send_approval(response.clone(), true).await.expect("sent");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                // The approver's decision, not the answers shown as given.
                let envelope = watch.events.recv().await.expect("the channel runs");
                if !matches!(envelope.context.sender_id, ActorId::Approver(_)) {
                    continue;
                }
                match envelope.event {
                    ActorEvent::ApprovalGranted(decided) if decided.id == request.id => break,
                    ActorEvent::ApprovalDenied(decided) if decided.id == request.id => panic!("bob's answer counted"),
                    _ => {}
                }
            }
        }).await.expect("decided");
    }).await;
    let approving: Vec<&String> = asked.iter().filter(|line| line.contains("ApproveToolCall")).collect();
    assert_eq!(approving, [
        // bob answers, and is left out.
        "bob: ApproveToolCall(alice's) no",
        // alice answers; whom to wait for: her alone.
        "alice: ApproveToolCall(alice's) yes",
        "alice: ApproveToolCall(alice's) yes",
        "bob: ApproveToolCall(alice's) no",
    ]);
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A tool whose arguments go to a remote side cleared for public alone -
/// as an MCP server that names nothing (issue 180).
#[derive(Debug)]
struct Outward;

#[async_trait]
impl caelumora_runtime::tool::ToolProvider for Outward {
    fn description(&self) -> String {
        "sends out".to_string()
    }
    fn tools(&self) -> Vec<caelumora_model::tool::Tool> {
        vec![caelumora_model::tool::Tool {
            name: "send_out".to_string(),
            description: "sends out".to_string(),
            parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    fn destination(&self) -> Option<Classification> {
        Some(Classification::PUBLIC)
    }
    async fn execute(&self, _: &str, _: &serde_json::Value, _: Option<&caelumora_model::question::Answers>, _: &mut caelumora_runtime::agent::turn::Turn) -> caelumora_runtime::tool::ToolResult<caelumora_runtime::tool::ToolOutput> {
        panic!("a withheld tool is never run")
    }
}

/// The door (issue 180): a tool set whose remote side the channel may not
/// flow to is not offered there - asked as publishing what the channel
/// holds there, with its owner's rights in a turn, asked by whoever asked
/// it, and in its owner's name for who looks at its tools (issue 255) - and
/// its tools are not called by name.
#[tokio::test]
async fn a_tool_whose_remote_side_is_not_cleared_for_the_channel_is_not_offered_there() {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolResultStatus, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "send_out".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with_kinds(|registry| {
        registry.register(ComponentKind::new("once").provides::<dyn caelumora_runtime::llm::LLMClient>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn caelumora_runtime::llm::LLMClient>(Arc::<Once>::clone(&model));
            Ok(facets)
        });
        registry.register(ComponentKind::new("outward").provides::<dyn caelumora_runtime::tool::ToolProvider>(), |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn caelumora_runtime::tool::ToolProvider>(Arc::new(Outward));
            Ok(facets)
        });
    }).await;
    let sending = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "once"), ComponentDescriptor::new("outward", "outward")] },
        ..world.personality("sending", None, &[]).await
    };
    world.store.update_personality(&World::system(), &sending).await.unwrap();
    let helper = world.agent("helper", world.alice, &sending).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut watch = main.user_actor(world.bob).expect("he takes part").watch();

    let (answered, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper send it".to_string(), None).await.expect("sent");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let caelumora_model::actor::ActorEvent::CompleteMessageGenerated(message) = watch.events.recv().await.expect("the channel runs").event {
                    if let Some(result) = message.tool_results.first() {
                        break result.status;
                    }
                }
            }
        }).await.expect("the call is answered")
    }).await;
    assert_eq!(answered, ToolResultStatus::UnknownTool, "not called by its name");
    assert!(asked.contains(&"alice as helper, asked by bob: Publish no".to_string()), "judged as alice, its owner, asked by bob: {asked:?}");
    assert!(!asked.iter().any(|line| line.contains("CallTool")), "refused before the policy is asked of the call: {asked:?}");

    let (sets, asked) = world.asked(async {
        let view = world.manager.views(Viewer::new(World::as_user(world.alice))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        view.member(ActorId::Agent(helper)).await.unwrap().tool_sets().await.unwrap()
    }).await;
    assert!(sets[0].tools.is_empty() && sets[0].unavailable.is_some(), "shown as unavailable: {sets:?}");
    assert!(asked.contains(&"alice as helper: Publish no".to_string()), "as a turn would be offered them, in the agent's own name: {asked:?}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- In whose name an agent acts (issue 255) ---------------------------------

/// A model component of the kind `kind` that answers its first call with
/// `answer`, registered with `registry`.
fn register_once(registry: &mut ComponentRegistry, kind: &str, answer: caelumora_model::message::Message) {
    let model = Arc::new(Once(Mutex::new(Some(answer))));
    registry.register(ComponentKind::new(kind).provides::<dyn caelumora_runtime::llm::LLMClient>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn caelumora_runtime::llm::LLMClient>(Arc::<Once>::clone(&model));
        Ok(facets)
    });
}

/// An agent asked by another agent acts for its own owner, and is recorded
/// as asked by whoever began the conversation: carol asks bob's `relay`,
/// which asks alice's `doer`, whose call is judged as alice and recorded as
/// asked by carol. The agent between them lends nothing, nor does its owner:
/// one agent mentioning another does not delegate to it.
#[tokio::test]
async fn an_agent_asked_by_an_agent_acts_for_its_own_owner() {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let internal = Classification::general(Confidentiality::Internal);
    let relayed = Message { role: MessageRole::Assistant, content: Some("@doer echo it".to_string()), ..Message::new(internal.clone()) };
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "echo".to_string(), arguments: serde_json::json!({ "message": "it" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(internal)
    };
    let world = World::with_kinds(|registry| {
        register_once(registry, "relaying", relayed);
        register_once(registry, "calling", call);
    }).await;
    let with = |name: &'static str, components: Vec<ComponentDescriptor>| {
        let world = &world;
        async move {
            let personality = Personality { components: ComponentConfig { component_nodes: components }, ..world.personality(name, None, &[]).await };
            world.store.update_personality(&World::system(), &personality).await.unwrap();
            personality
        }
    };
    let relaying = with("relaying", vec![ComponentDescriptor::new("model", "relaying")]).await;
    let calling = with("calling", vec![
        ComponentDescriptor::new("model", "calling"),
        ComponentDescriptor::new("echo", caelumora_runtime::tool::echo_tool::ECHO_TOOL_KIND),
    ]).await;
    let relay = world.agent("relay", world.bob, &relaying).await;
    let doer = world.agent("doer", world.alice, &calling).await;
    let session = world.session_of(&[world.bob, world.carol], &[relay, doer], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.carol), session.id()).await.expect("she opens it");

    let (_, asked) = world.asked(async {
        main.user_actor(world.carol).expect("she takes part").send_message("@relay ask doer".to_string(), None).await.expect("sent");
        asked_for(&world, "alice as doer, asked by carol: CallTool yes").await;
    }).await;
    let calling: Vec<&String> = asked.iter().filter(|line| line.contains("CallTool")).collect();
    assert!(!calling.is_empty() && calling.iter().all(|line| *line == "alice as doer, asked by carol: CallTool yes"), "judged as its owner, asked by who began it: {asked:?}");
    assert!(asked.iter().filter(|line| line.contains("as doer")).all(|line| line.starts_with("alice as doer, asked by carol:")), "nothing of it in bob's name: {asked:?}");
    assert!(asked.iter().any(|line| line.starts_with("bob as relay, asked by carol:")), "the relay acted for its own owner: {asked:?}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The agent `reader` of `owner`, which reads the channel "side" with
/// `read_history` once it is asked, in a session alice shares with bob and
/// he opened: "side" is alice's, and `reader` takes part in it if
/// `in_side`; alice said something there, classified as the main channel
/// is. With the main channel.
async fn with_a_reader_of(owner: fn(&World) -> UserId, in_side: bool) -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let internal = Classification::general(Confidentiality::Internal);
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "read_history".to_string(), arguments: serde_json::json!({ "channel": "side" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(internal)
    };
    let world = World::with_kinds(|registry| register_once(registry, "reading", call)).await;
    let reading = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "reading"),
            ComponentDescriptor::new("history", caelumora_runtime::component_impl::channel_history::CHANNEL_HISTORY_KIND),
        ] },
        ..world.personality("reading", None, &[]).await
    };
    world.store.update_personality(&World::system(), &reading).await.unwrap();
    let reader = world.agent("reader", owner(&world), &reading).await;
    let session = world.shared_session(&[reader], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let participants = if in_side { vec![ActorId::Agent(reader)] } else { vec![] };
    let side = session.create_channel(&World::as_user(world.alice), ChannelSpec { name: "side".to_string(), components: quiet(), participants, classification: None, language: None, uses_workspace: None }).await.expect("hers");
    side.user_actor(world.alice).expect("she takes part").send_message("only for side".to_string(), None).await.expect("said");
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap();
    assert!(his.channel(side.id()).await.is_err(), "not his to read");
    (world, session, main)
}

/// What the policy is asked when bob asks `reader` in `main` to read
/// "side", once it was asked `line` and a little after: what side holds is
/// asked of entry by entry after the channel as a whole.
async fn asked_to_read_side(world: &World, main: &caelumora_runtime::channel::ChannelLoop, line: &str) -> Vec<String> {
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@reader what is on side?".to_string(), None).await.expect("sent");
        asked_for(world, line).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }).await;
    asked
}

/// An agent reads another channel with its owner's rights, whoever asked
/// it (issue 255): bob asks alice's `reader` to read her channel "side",
/// which he may not read himself - the agent reaches it as alice, asked by
/// bob (`GetChannel`, issue 264), and copies it as far as the flow rule
/// lets it into the channel he asked in (`CopyInformation`). What the
/// decision accepts: her agent, asked without approval, lets every member
/// read through it what she may.
#[tokio::test]
async fn an_agent_reads_another_channel_with_its_owners_rights_whoever_asked() {
    let (world, session, main) = with_a_reader_of(|world| world.alice, false).await;
    let asked = asked_to_read_side(&world, &main, "alice as reader, asked by bob: CopyInformation yes").await;
    let readers: Vec<&String> = asked.iter().filter(|line| line.contains("as reader")).collect();
    assert!(readers.iter().all(|line| line.starts_with("alice as reader, asked by bob:")), "with her rights, asked by him: {asked:?}");
    assert!(readers.contains(&&"alice as reader, asked by bob: GetChannel(alice's) yes".to_string()), "reached first: {asked:?}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent does not read a channel of its session that its owner may not
/// read and it takes no part in (issue 264): bob's `reader` does not reach
/// alice's "side" (`GetChannel`), and nothing of it is asked to be copied -
/// not as a whole, though the flow rule would let it, nor entry by entry:
/// a release adds no readers.
#[tokio::test]
async fn an_agent_does_not_read_a_channel_its_owner_may_not_read() {
    let (world, session, main) = with_a_reader_of(|world| world.bob, false).await;
    let asked = asked_to_read_side(&world, &main, "bob as reader: GetChannel(alice's) no").await;
    assert!(!asked.iter().any(|line| line.contains("CopyInformation")), "nothing of side to copy: {asked:?}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent reads a channel it takes part in from another, though its
/// owner may not read it (issue 264): bob's `reader`, added to alice's
/// "side" by her, reads it from the main channel when bob asks.
#[tokio::test]
async fn an_agent_reads_a_channel_it_takes_part_in_whose_owner_may_not() {
    let (world, session, main) = with_a_reader_of(|world| world.bob, true).await;
    let asked = asked_to_read_side(&world, &main, "bob as reader: CopyInformation yes").await;
    assert!(asked.contains(&"bob as reader: GetChannel(alice's) yes".to_string()), "reached as taking part: {asked:?}");
    assert!(asked.iter().filter(|line| line.contains("CopyInformation")).all(|line| line.ends_with(" yes")), "{asked:?}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- External agents ---------------------------------------------------------

/// A remote agent that takes every request on as a task and works on it
/// for ever - or, given a `gate`, until the gate lets it answer, one permit
/// a request. Every call reads its credential, as an A2A agent does - in
/// the name it is called in - and is recorded. What it brings in itself is
/// classified as it `declared`.
#[derive(Debug)]
struct RemoteAtWork {
    secrets: Arc<SecretManager>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    declared: Option<Classification>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
}

impl RemoteAtWork {
    async fn called(&self, call: &'static str, security: &SecurityContext) {
        // Whether there is such a secret does not matter: who is asked does.
        let _ = self.secrets.resolve(security, "REMOTE_TOKEN").await;
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl ExternalAgent for RemoteAtWork {
    async fn send(&self, security: &SecurityContext, _: Option<&ConversationId>, _: OutboundMessage) -> ExternalAgentResult<ExternalStream> {
        self.called("send", security).await;
        let started = futures::stream::iter([Ok(ExternalEvent::Started(TaskId("t-1".to_string())))]);
        let Some(gate) = self.gate.clone() else { return Ok(Box::pin(started.chain(futures::stream::pending()))) };
        let opened = futures::stream::once(async move {
            if let Ok(permit) = gate.acquire_owned().await {
                permit.forget();
            }
        });
        let answered = futures::stream::iter([Ok(ExternalEvent::Message("Found.".to_string())), Ok(ExternalEvent::Completed)]);
        Ok(Box::pin(started.chain(opened.filter_map(|()| async { None })).chain(answered)))
    }

    async fn resume(&self, security: &SecurityContext, _: &TaskId) -> ExternalAgentResult<ExternalStream> {
        self.called("resume", security).await;
        Ok(Box::pin(futures::stream::pending()))
    }

    async fn cancel(&self, security: &SecurityContext, _: &TaskId) -> ExternalAgentResult<()> {
        self.called("cancel", security).await;
        Ok(())
    }

    fn classification(&self) -> Option<Classification> {
        self.declared.clone()
    }

    fn profile(&self) -> caelumora_model::context::ModelProfile {
        caelumora_model::context::ModelProfile::default()
    }
}

/// The world, with the kind `remote_at_work`: a `RemoteAtWork` declaring
/// `declared`, recording its calls in `calls`, working for ever.
async fn with_remote_at_work(calls: &Arc<Mutex<Vec<&'static str>>>, declared: Option<Classification>) -> World {
    with_remote(calls, declared, None).await
}

/// `with_remote_at_work`, answering each request once `gate` lets it, if
/// there is one.
async fn with_remote(calls: &Arc<Mutex<Vec<&'static str>>>, declared: Option<Classification>, gate: Option<Arc<tokio::sync::Semaphore>>) -> World {
    let calls = Arc::clone(calls);
    World::with_kinds(|registry| registry.register(ComponentKind::new("remote_at_work").provides::<dyn ExternalAgent>(), move |_, context| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn ExternalAgent>(Arc::new(RemoteAtWork { secrets: Arc::clone(&context.secret_manager), calls: Arc::clone(&calls), declared: declared.clone(), gate: gate.clone() }));
        Ok(facets)
    })).await
}

/// An external personality whose agent is reached as `remote_at_work`.
async fn remote_personality(world: &World) -> Personality {
    let remote = Personality {
        kind: PersonalityKind::External,
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("remote", "remote_at_work")] },
        ..world.personality("remote", None, &[]).await
    };
    world.store.update_personality(&World::system(), &remote).await.unwrap();
    remote
}

/// Waits until `calls` holds `count` calls.
async fn calls_made(calls: &Mutex<Vec<&'static str>>, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while calls.lock().unwrap().len() < count {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the remote agent is called");
}

/// An external agent's task is done as the request that began it, as an
/// agent's turn is (issue 211), with its owner's rights (issue 255): asked
/// by bob, the remote agent is sent his request with alice's credential,
/// asked by him - and followed so after a restart, whoever opened the
/// session.
#[tokio::test]
async fn an_external_agents_task_is_followed_for_its_owner_as_whoever_asked_began_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        calls_made(&calls, 1).await;
    }).await;
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "with alice's credential, asked by bob");

    // The task is stored as begun; the session stops with it still open.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while world.store.get_agent_channel_state(hermes, main.id()).await.unwrap().remote.open_tasks.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the task is stored");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();

    let (reopened, asked) = world.asked(async {
        let reopened = world.manager.open_session(&World::as_user(world.alice), session.id()).await.expect("alice opens it again");
        calls_made(&calls, 2).await;
        reopened
    }).await;
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "resume"]);
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "followed so too, whoever opened the session");
    world.manager.stop_session(&World::as_user(world.alice), reopened.id()).await.unwrap();
}

/// Asked several times while it answers, an agent answers once afterwards
/// (issue 222) - as asked by the last who asked, not by the first nor by
/// all of them, and with its owner's rights (issue 255): alice's request is
/// dropped, and the remote agent is sent the conversation as asked by bob
/// alone.
#[tokio::test]
async fn an_agent_asked_by_several_while_it_answers_answers_once_as_asked_by_the_last() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let world = with_remote(&calls, None, Some(Arc::clone(&gate))).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch().events;
    main.user_actor(world.alice).expect("she takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;

    let (_, asked) = world.asked(async {
        // Both ask while it works; both messages are said before it is free.
        main.user_actor(world.alice).expect("she takes part").send_message("@hermes and this".to_string(), None).await.expect("she asks");
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes and that".to_string(), None).await.expect("he asks");
        let said_by = |envelope: &caelumora_model::actor::Envelope, sender: ActorId| envelope.context.sender_id == sender
            && matches!(envelope.event, caelumora_model::actor::ActorEvent::CompleteMessageGenerated(_));
        let finished_by = |envelope: &caelumora_model::actor::Envelope, sender: ActorId| envelope.context.sender_id == sender
            && matches!(envelope.event, caelumora_model::actor::ActorEvent::TurnFinished { .. });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !said_by(&events.recv().await.expect("the channel runs"), ActorId::User(world.bob)) {}
        }).await.expect("his message is said");
        gate.add_permits(3);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut answers = 0;
            while answers < 2 {
                answers += usize::from(finished_by(&events.recv().await.expect("the channel runs"), ActorId::Agent(hermes)));
            }
        }).await.expect("it answers the first request and then the rest");
        // A further answer would call the remote agent right away.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }).await;
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "send"], "one call for all that was asked meanwhile");
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "as asked by the last who asked");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An external agent whose answers, as its component declares them, may no
/// longer enter its channel once the channel was lowered is refused with
/// every request (issue 212): asked with its owner's rights, as asked by
/// whoever asks, the remote agent is not called, and the users are told.
#[tokio::test]
async fn an_external_agent_whose_answers_no_longer_enter_its_lowered_channel_is_refused() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, Some(Classification::general(Confidentiality::Internal))).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch().events;

    let (failed, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let caelumora_model::actor::ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                    break match reason {
                        caelumora_model::turn::GivenUpReason::Failed { message } => message,
                        other => other.cause().to_string(),
                    };
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("may not be published"), "{failed}");
    let publish: Vec<&String> = asked.iter().filter(|line| line.starts_with("alice as hermes, asked by bob: Publish")).collect();
    assert_eq!(publish, ["alice as hermes, asked by bob: Publish no"], "as its owner, asked by bob");
    assert!(calls.lock().unwrap().is_empty(), "the remote agent is not called");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An external agent's task begun before its channel was lowered is given up
/// as a turn is (issue 212): the remote agent is asked to cancel it as
/// asked by who asked for it, with its owner's rights, and so is whether
/// what came of it may still be published.
#[tokio::test]
async fn lowering_a_channel_gives_up_an_external_agents_task_as_whoever_asked_began_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch().events;
    main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;

    let (failed, asked) = world.asked(async {
        main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let caelumora_model::actor::ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                    break match reason {
                        caelumora_model::turn::GivenUpReason::Failed { message } => message,
                        other => other.cause().to_string(),
                    };
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("lowered"), "{failed}");
    let his: Vec<&String> = asked.iter().filter(|line| line.starts_with("alice as hermes, asked by bob")).collect();
    // The lowering cancels the answer, which cancels its remote task - as
    // every cancel does - and only then is asked whether it may go on.
    assert_eq!(his, ["alice as hermes, asked by bob: ReadSecret yes", "alice as hermes, asked by bob: Publish no"], "cancelled, and asked whether it goes on, as he asked");
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "cancel"]);
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Users and their profiles -------------------------------------------------

/// U1: a user's record is the system's, their profile their own; what they
/// say of themselves reaches everyone in their session.
#[tokio::test]
async fn a_profile_is_written_by_its_user_alone_and_read_by_who_shares_a_session() {
    let world = World::new().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let profile = UserProfile::new(world.alice, Some(Introduction::new("I run the team.").unwrap()));
    world.store.create_user_profile(&alice, &profile).await.expect("she writes her own");
    let forged = UserProfile::new(world.alice, Some(Introduction::new("I quit.").unwrap()));
    assert!(world.store.update_user_profile(&bob, &forged).await.is_err(), "bob does not write hers");
    let carols = UserProfile::new(world.carol, Some(Introduction::new("hello").unwrap()));
    assert!(world.store.create_user_profile(&bob, &carols).await.is_err(), "nor anyone else's");

    let mut promoted = world.store.get_user(&alice, world.alice).await.expect("everyone reads a user");
    promoted.roles.push(RoleId::new());
    assert!(world.store.update_user(&alice, &promoted).await.is_err(), "nobody grants themselves roles");

    let session = world.shared_session(&[], &[]).await;
    let channel = session.channels()[0].id();
    let view = world.manager.views(Viewer::new(bob)).session(session.id()).await.unwrap().channel(channel).await.unwrap();
    let members = view.members().await;
    let hers = members.iter().find(|m| m.id() == ActorId::User(world.alice)).unwrap();
    assert_eq!(hers.introduction().await.unwrap().as_deref(), Some("I run the team."), "bob reads how she introduces herself");
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}


// --- The permissions each action needs --------------------------------------
//
// What each action asks of the policy, in whose name, of whose entity, and
// what it is answered - for its owner, for someone taking part, for a
// stranger: the record of which permissions an action needs (issue 198).
// The runtime's own reads in its own name are left out (`World::asked`).

/// `lines`, as `World::asked` records them.
fn checks(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

/// Making a session from a template: reading the template and its owner,
/// finding or making the agents - reading their personalities -, the
/// classification and everyone's clearance for the channels, each agent's
/// personality in its own name, and creating the session and its channels.
#[tokio::test]
async fn making_a_session_needs_the_template_the_personalities_and_creating_what_is_made() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let bobs = world.personality("bobs", Some(world.bob), &[]).await;
    let template = world.template(None, &[], "helper", shared.id).await;
    let (alice, carol) = (World::as_user(world.alice), World::as_user(world.carol));

    let (made, asked) = world.asked(world.manager.create_from_template(&alice, template, None, None)).await;
    let session = made.expect("alice makes it");
    assert_eq!(asked, checks(&[
        "alice: GetSessionTemplate(system's) yes",
        "alice: GetUser(system's) yes",
        "alice: ListAgents yes",
        "alice: GetPersonality(bob's) yes",
        "alice: CreateAgent(alice's) yes",
        "alice: Classify yes",
        "alice: JoinChannel(alice) yes",
        "alice as helper: GetPersonality(bob's) yes",
        "alice: JoinChannel(helper) yes",
        "alice: CreateSession(alice's) yes",
        "alice: CreateChannel(alice's) yes",
        "alice as helper: GetPersonality(bob's) yes",
    ]), "the agent's personality is asked for in its name twice: whether it is cleared, and as its channel starts");
    world.manager.stop_session(&alice, session.id()).await.unwrap();

    // Again: the agent is found, not made - its personality is not read
    // for making it, only in its own name, to run.
    let (again, asked) = world.asked(world.manager.create_from_template(&alice, template, None, None)).await;
    assert!(again.is_ok());
    assert_eq!(asked.iter().filter(|line| line.contains("CreateAgent")).count(), 0, "{asked:?}");
    assert!(asked.contains(&"alice as helper: GetPersonality(bob's) yes".to_string()), "{asked:?}");
    world.manager.stop_session(&alice, again.unwrap().id()).await.unwrap();

    let private = world.template(Some(world.bob), &[], "private", shared.id).await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&carol, private, None, None)).await;
    assert!(not_found(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["carol: GetSessionTemplate(bob's) no"]), "a template one may not read: nothing more is asked");

    let unusable = world.template(Some(world.alice), &[], "unusable", bobs.id).await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&alice, unusable, None, None)).await;
    assert!(not_found(&refused.unwrap_err()));
    assert_eq!(asked, checks(&[
        "alice: GetSessionTemplate(alice's) yes",
        "alice: GetUser(system's) yes",
        "alice: ListAgents yes",
        "alice: GetAgent(alice's) yes",
        "alice: GetPersonality(bob's) no",
    ]), "a personality one may not read: no agent, nothing created - the list is filtered by what she may read, her agent from before");
}

/// Opening (starting) a session: reading it.
#[tokio::test]
async fn opening_a_session_needs_reading_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    for (user, answer) in [(world.alice, "yes"), (world.bob, "yes"), (world.carol, "no")] {
        let (_, asked) = world.asked(world.manager.open_session(&World::as_user(user), session.id())).await;
        let name = world.policy.name(ActorId::User(user));
        assert_eq!(asked, checks(&[&format!("{name}: GetSession(alice's) {answer}")]));
    }
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Stopping a session for everyone in it: reading it, and stopping it -
/// its owner's alone.
#[tokio::test]
async fn stopping_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let (_, asked) = world.asked(world.manager.stop_session(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: GetSession(alice's) no"]), "a stranger: not even found");
    let (_, asked) = world.asked(world.manager.stop_session(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: GetSession(alice's) yes", "bob: StopSession(alice's) no"]));
    let (stopped, asked) = world.asked(world.manager.stop_session(&World::as_user(world.alice), session.id())).await;
    assert!(stopped.unwrap());
    assert_eq!(asked, checks(&["alice: GetSession(alice's) yes", "alice: StopSession(alice's) yes"]));
}

/// Deleting a session: reading it and deleting it - its owner's alone. It
/// is marked and kept, with its channels (issue 164), and what lies in its
/// scope goes with it in the runtime's name - also an agent of somebody
/// else's, whom nothing more is asked (issue 178).
#[tokio::test]
async fn deleting_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let personality = world.personality("plain", None, &[]).await;
    let bobs = Agent::new(ScopedName::new("helper", Scope::SessionScope { session_id: session.id() }).unwrap(), &personality, ActorId::User(world.bob));
    world.store.create_agent(&World::system(), &bobs).await.unwrap();
    let (_, asked) = world.asked(world.manager.delete_session(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: GetSession(alice's) no"]));
    let (_, asked) = world.asked(world.manager.delete_session(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: GetSession(alice's) yes", "bob: DeleteSession(alice's) no"]));
    let (deleted, asked) = world.asked(world.manager.delete_session(&World::as_user(world.alice), session.id())).await;
    assert!(deleted.unwrap());
    assert_eq!(asked, checks(&[
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
    ]), "asked by the session manager before it stops anything, and again by the store");
    assert!(world.store.get_agent(&World::system(), bobs.id).await.is_err(), "bob's agent of its scope went with it");

    world.manager.restore_session(&World::as_user(world.alice), session.id()).await.unwrap();
    assert!(world.store.get_agent(&World::system(), bobs.id).await.is_ok(), "and comes back with it");
}

/// Creating a channel: the classification it is given, everyone's
/// clearance for it, creating it - as its session's owner - and changing
/// the session that lists it.
#[tokio::test]
async fn creating_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let spec = |participants| ChannelSpec { name: "side".to_string(), components: quiet(), participants, classification: None, language: None, uses_workspace: None };
    let (created, asked) = world.asked(session.create_channel(&World::as_user(world.alice), spec(vec![ActorId::User(world.bob)]))).await;
    assert!(created.is_ok());
    assert_eq!(asked, checks(&[
        "alice: UpdateSession(alice's) yes",
        "alice: Classify yes",
        "alice: JoinChannel(alice) yes",
        "alice: JoinChannel(bob) yes",
        "alice: CreateChannel(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
    ]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Who may not create a channel is refused before anything is asked of
/// the others in it (issue 198, F2).
#[tokio::test]
async fn who_may_not_create_a_channel_is_refused_before_the_others_are_looked_at() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let spec = ChannelSpec { name: "his".to_string(), components: quiet(), participants: vec![ActorId::User(world.alice)], classification: None, language: None, uses_workspace: None };
    let (refused, asked) = world.asked(session.create_channel(&World::as_user(world.bob), spec)).await;
    assert!(refused.is_err());
    assert_eq!(asked, checks(&["bob: UpdateSession(alice's) no"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Deleting a channel: changing its session - its owner's alone - and
/// deleting the channel.
#[tokio::test]
async fn deleting_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    let (_, asked) = world.asked(session.delete_channel(&World::as_user(world.bob), main)).await;
    assert_eq!(asked, checks(&["bob: GetSession(alice's) yes", "bob: UpdateSession(alice's) no"]));
    let (deleted, asked) = world.asked(session.delete_channel(&World::as_user(world.alice), main)).await;
    assert!(deleted.is_ok());
    assert_eq!(asked, checks(&[
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
        "alice: GetChannel(alice's) yes",
        "alice: DeleteChannel(alice's) yes",
    ]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Restoring a deleted session: reading it as deleted and restoring it -
/// its owner's alone, not a contributor's; for whoever may not read the
/// trash, there is nothing to restore (issue 164).
#[tokio::test]
async fn restoring_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    world.manager.delete_session(&World::as_user(world.alice), session.id()).await.unwrap();
    let (_, asked) = world.asked(world.manager.restore_session(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: ReadDeleted(alice's) no"]));
    let (_, asked) = world.asked(world.manager.restore_session(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: ReadDeleted(alice's) no"]), "a contributor does not see the trash");
    let (restored, asked) = world.asked(world.manager.restore_session(&World::as_user(world.alice), session.id())).await;
    assert!(restored.is_ok());
    assert_eq!(asked, checks(&[
        "alice: ReadDeleted(alice's) yes",
        "alice: Restore(alice's) yes",
        "alice: GetSession(alice's) yes",
    ]), "and reads it back, restored");
}

/// Purging a deleted session: the system's alone until there is an
/// administrator (issue 125) - its owner may read it as deleted, but not
/// remove it for good (issue 164).
#[tokio::test]
async fn purging_a_session_is_not_its_owners() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    world.manager.delete_session(&World::as_user(world.alice), session.id()).await.unwrap();
    let (_, asked) = world.asked(world.manager.purge_session(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: ReadDeleted(alice's) no"]));
    let (refused, asked) = world.asked(world.manager.purge_session(&World::as_user(world.alice), session.id())).await;
    assert!(refused.is_err());
    assert_eq!(asked, checks(&[
        "alice: ReadDeleted(alice's) yes",
        "alice: ReadDeleted(alice's) yes",
        "alice: Purge(alice's) no",
    ]), "only what was deleted, asked by the session manager and again by the store");
}

/// Restoring a deleted channel: changing its session - its owner's alone
/// -, everyone's clearance and restoring the channel (issue 164).
#[tokio::test]
async fn restoring_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    session.delete_channel(&World::as_user(world.alice), main).await.unwrap();
    let (_, asked) = world.asked(session.restore_channel(&World::as_user(world.bob), main)).await;
    assert_eq!(asked, checks(&["bob: UpdateSession(alice's) no"]));
    let (restored, asked) = world.asked(session.restore_channel(&World::as_user(world.alice), main)).await;
    assert!(restored.is_ok());
    assert_eq!(asked, checks(&[
        "alice: UpdateSession(alice's) yes",
        "alice: ReadDeleted(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: JoinChannel(bob) yes",
        "alice: ReadDeleted(alice's) yes",
        "alice: Restore(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
    ]), "everyone's clearance again, as when it was created, and listing it in the session");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Deleting an agent: reading it and deleting it - its owner's alone, not
/// the owner's of its personality; for whoever may not read it, there is
/// nothing to delete.
#[tokio::test]
async fn deleting_an_agent_needs_owning_it() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let (deleted, asked) = world.asked(world.store.delete_agent(&World::as_user(world.bob), helper)).await;
    assert!(!deleted.unwrap(), "not there for bob, though the personality is his");
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) no"]));
    let (deleted, asked) = world.asked(world.store.delete_agent(&World::as_user(world.alice), helper)).await;
    assert!(deleted.unwrap());
    assert_eq!(asked, checks(&["alice: GetAgent(alice's) yes", "alice: DeleteAgent(alice's) yes"]));
}

/// Deleting an agent through the session manager (issue 164): reading it
/// and deleting it - its owner's alone - before anything happens; it leaves
/// its channels in the runtime's name. Restoring it is its owner's too,
/// purging it not.
#[tokio::test]
async fn deleting_restoring_and_purging_an_agent_is_its_owners_but_purging() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let (_, asked) = world.asked(world.manager.delete_agent(&World::as_user(world.bob), helper)).await;
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) no"]));
    let (deleted, asked) = world.asked(world.manager.delete_agent(&World::as_user(world.alice), helper)).await;
    assert!(deleted.is_ok());
    assert_eq!(asked, checks(&[
        "alice: GetAgent(alice's) yes",
        "alice: DeleteAgent(alice's) yes",
        "alice: GetAgent(alice's) yes",
        "alice: DeleteAgent(alice's) yes",
    ]), "asked by the session manager before it changes any channel, and again by the store");
    let (refused, asked) = world.asked(world.manager.purge_agent(&World::as_user(world.alice), helper)).await;
    assert!(refused.is_err());
    assert_eq!(asked, checks(&["alice: ReadDeleted(alice's) yes", "alice: Purge(alice's) no"]));
    let (restored, asked) = world.asked(world.manager.restore_agent(&World::as_user(world.alice), helper)).await;
    assert!(restored.is_ok());
    assert_eq!(asked, checks(&[
        "alice: ReadDeleted(alice's) yes",
        "alice: ReadDeleted(alice's) yes",
        "alice: Restore(alice's) yes",
    ]), "only what was deleted, asked by the session manager and again by the store");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Adding an agent to a channel: its personality in its own name - may it
/// run -, its clearance, and changing the channel - its owner's alone.
#[tokio::test]
async fn adding_an_agent_needs_owning_the_channel_and_the_agent_its_personality() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let (helper, other) = (world.agent("helper", world.alice, &shared).await, world.agent("other", world.alice, &shared).await);
    let session = world.shared_session(&[], &[helper, other]).await;
    let main = Arc::clone(&session.channels()[0]);
    let alice = World::as_user(world.alice);

    let (added, asked) = world.asked(main.add_participant(&alice, ActorId::Agent(helper), HistoryAccess::All)).await;
    assert!(added.is_ok());
    assert_eq!(asked, checks(&[
        "alice: UpdateChannel(alice's) yes",
        "alice as helper: GetPersonality(bob's) yes",
        "alice: JoinChannel(helper) yes",
        "alice: GetChannel(alice's) yes",
        "alice: UpdateChannel(alice's) yes",
    ]));

    let withdrawn = Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world.store.update_personality(&World::as_user(world.bob), &withdrawn).await.unwrap();
    let (refused, asked) = world.asked(main.add_participant(&alice, ActorId::Agent(other), HistoryAccess::All)).await;
    assert!(denied(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["alice: UpdateChannel(alice's) yes", "alice as other: GetPersonality(bob's) no"]), "an agent that may not run is not added");
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}

/// Who may not change a channel is refused before anything is asked of
/// the agent they would add (issue 198, F2).
#[tokio::test]
async fn who_may_not_add_to_a_channel_is_refused_before_the_agent_is_looked_at() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (refused, asked) = world.asked(main.add_participant(&World::as_user(world.bob), ActorId::Agent(helper), HistoryAccess::All)).await;
    assert!(denied(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["bob: UpdateChannel(alice's) no"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Removing an agent from a channel: changing the channel - its owner's
/// alone; nothing is asked of the agent.
#[tokio::test]
async fn removing_an_agent_needs_owning_the_channel() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (_, asked) = world.asked(main.remove_participant(&World::as_user(world.bob), ActorId::Agent(helper))).await;
    assert_eq!(asked, checks(&["bob: GetChannel(alice's) yes", "bob: UpdateChannel(alice's) no"]));
    let (removed, asked) = world.asked(main.remove_participant(&World::as_user(world.alice), ActorId::Agent(helper))).await;
    assert!(removed.is_ok());
    assert_eq!(asked, checks(&["alice: GetChannel(alice's) yes", "alice: UpdateChannel(alice's) yes"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Workspaces (issues 251, 253) ---

/// The content of a file to write.
fn file(text: &'static str) -> ByteStream {
    Box::pin(futures::stream::iter([Ok(bytes::Bytes::from_static(text.as_bytes()))]))
}

async fn read(terminal: &dyn Terminal, path: &str) -> String {
    let (_, content) = terminal.read(&WorkspacePath::parse(path).unwrap()).await.unwrap();
    let chunks: Vec<_> = content.collect().await;
    String::from_utf8(chunks.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect()).unwrap()
}

/// A workspace is its owner's alone (issue 251, decision 12): made by a
/// user cleared for its classification, read, changed, deleted and
/// restored by them, by nobody else - and purged by the system, its files
/// with it.
#[tokio::test]
async fn a_workspace_is_its_owners_alone() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);

    let (made, asked) = world.asked(workspaces.create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", Some("Drafts"), internal.clone())).await;
    let docs = made.expect("alice makes one");
    assert_eq!(asked, checks(&["alice: Classify yes", "alice: JoinChannel(alice) yes", "alice: CreateWorkspace(alice's) yes"]));
    assert!(world.files.path().join(docs.id.to_string()).is_dir(), "with its place");
    let above = workspaces.create(&alice, "secrets", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Confidential)).await;
    assert!(matches!(above, Err(WorkspaceError::NotCleared)), "not above what she is cleared for: {above:?}");
    let elsewhere = workspaces.create(&alice, "other", Scope::UserScope { user_id: world.alice }, "nowhere", None, internal.clone()).await;
    assert!(matches!(elsewhere, Err(WorkspaceError::UnknownStorage(_))), "nor in a storage that is not configured: {elsewhere:?}");

    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));
    assert_eq!(views(&alice).workspaces().await.unwrap().len(), 1, "hers");
    assert!(views(&bob).workspaces().await.unwrap().is_empty(), "not his");
    let (hidden, asked) = world.asked(views(&bob).workspace(docs.id)).await;
    assert!(matches!(hidden, Err(ViewError::WorkspaceNotFound(_))));
    assert_eq!(asked, checks(&["bob: GetWorkspace(alice's) no"]));

    let renamed = workspaces.rename(&bob, docs.id, "mine").await;
    assert!(matches!(renamed, Err(WorkspaceError::NotFound(_))), "bob does not rename it: {renamed:?}");
    let (renamed, asked) = world.asked(workspaces.rename(&alice, docs.id, "notes")).await;
    assert_eq!(renamed.expect("alice does").name.name, "notes");
    assert_eq!(asked, checks(&["alice: GetWorkspace(alice's) yes", "alice: GetWorkspace(alice's) yes", "alice: UpdateWorkspace(alice's) yes"]));

    assert!(!workspaces.delete(&bob, docs.id).await.unwrap(), "nothing bob could see to delete");
    assert!(workspaces.delete(&alice, docs.id).await.unwrap());
    assert!(matches!(workspaces.restore(&bob, docs.id).await, Err(WorkspaceError::NotFound(_))), "nor restore");
    workspaces.restore(&alice, docs.id).await.expect("alice restores it");
    assert!(workspaces.delete(&alice, docs.id).await.unwrap());
    let (purged, asked) = world.asked(workspaces.purge(&alice, docs.id)).await;
    assert!(matches!(purged, Err(WorkspaceError::Store(StoreError::PermissionDenied))), "the owner does not purge: {purged:?}");
    assert_eq!(asked, checks(&["alice: ReadDeleted(alice's) yes", "alice: Purge(alice's) no"]));
    assert!(world.files.path().join(docs.id.to_string()).is_dir(), "and the files stay");
    assert!(workspaces.purge(&World::system(), docs.id).await.unwrap(), "the system does");
    assert!(!world.files.path().join(docs.id.to_string()).exists(), "with the files");
}

/// A storage takes what may flow to where it puts it (issue 251, part B2):
/// a workspace classified above its destination is not made there, nor
/// reclassified above it - asked as publishing there, before the
/// clearance is.
#[tokio::test]
async fn a_storage_holds_no_workspace_above_its_destination() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let alice = World::as_user(world.alice);
    let mine = Scope::UserScope { user_id: world.alice };

    let (above, asked) = world.asked(workspaces.create(&alice, "secrets", mine.clone(), "shell", None, Classification::general(Confidentiality::Confidential))).await;
    assert!(matches!(&above, Err(WorkspaceError::StorageNotCleared(storage)) if storage == "shell"), "{above:?}");
    assert_eq!(asked, checks(&["alice: Classify yes", "alice: Publish no"]));

    let (made, asked) = world.asked(workspaces.create(&alice, "docs", mine, "shell", None, Classification::general(Confidentiality::Internal))).await;
    let docs = made.expect("up to it");
    assert_eq!(asked, checks(&["alice: Classify yes", "alice: Publish yes", "alice: JoinChannel(alice) yes", "alice: CreateWorkspace(alice's) yes"]));
    let raised = workspaces.reclassify(&alice, docs.id, Classification::general(Confidentiality::Confidential)).await;
    assert!(matches!(raised, Err(WorkspaceError::StorageNotCleared(_))), "{raised:?}");
}

/// Attaching a workspace is its owner's act, into a session they may change
/// (issue 251, decision 12); it shares the workspace with the session's
/// members only where asked to, by an entry of its access list (issue 23).
/// Taking it away needs changing the session alone, and leaves the list.
#[tokio::test]
async fn only_its_owner_attaches_a_workspace_to_a_session_they_may_change() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);
    let hers = workspaces.create(&alice, "hers", Scope::UserScope { user_id: world.alice }, "main", None, internal.clone()).await.unwrap();
    let his = workspaces.create(&bob, "his", Scope::UserScope { user_id: world.bob }, "main", None, internal).await.unwrap();
    let session = world.shared_session(&[], &[]).await;

    let (refused, asked) = world.asked(session.set_workspace(&bob, Some(his.id), None)).await;
    assert!(denied(&refused.unwrap_err()), "bob's own, into alice's session");
    assert_eq!(asked, checks(&["bob: UpdateSession(alice's) no"]), "refused before his workspace is asked about");
    let (refused, asked) = world.asked(session.set_workspace(&alice, Some(his.id), None)).await;
    assert!(not_found(&refused.unwrap_err()), "bob's, by alice");
    assert_eq!(asked, checks(&["alice: UpdateSession(alice's) yes", "alice: GetWorkspace(bob's) no"]));

    let (attached, asked) = world.asked(session.set_workspace(&alice, Some(hers.id), None)).await;
    attached.expect("hers, into her session");
    assert_eq!(asked, checks(&["alice: UpdateSession(alice's) yes", "alice: GetWorkspace(alice's) yes", "alice: UpdateWorkspace(alice's) yes", "alice: GetSession(alice's) yes", "alice: UpdateSession(alice's) yes"]));
    assert_eq!(session.session().workspace, Some(hers.id));
    let members = Subject::MemberOf(Membership::Session(session.id()));
    assert!(world.store.get_workspace(&alice, hers.id).await.unwrap().security_attributes.acl.is_empty(), "attaching alone shares nothing");

    session.set_workspace(&alice, Some(hers.id), Some(Grant::Read)).await.expect("shared with its members to read");
    let acl = world.store.get_workspace(&alice, hers.id).await.unwrap().security_attributes.acl;
    assert_eq!(acl, [caelumora_model::security::AclEntry { subject: members, grant: Grant::Read }]);
    session.set_workspace(&alice, Some(hers.id), Some(Grant::Write)).await.expect("and widened to write");
    let acl = world.store.get_workspace(&alice, hers.id).await.unwrap().security_attributes.acl;
    assert_eq!(acl, [caelumora_model::security::AclEntry { subject: members, grant: Grant::Write }], "one entry, widened");

    assert!(denied(&session.set_workspace(&bob, None, None).await.unwrap_err()), "bob does not take it away");
    session.set_workspace(&alice, None, None).await.expect("alice does");
    assert_eq!(session.session().workspace, None);
    assert_eq!(world.store.get_workspace(&alice, hers.id).await.unwrap().security_attributes.acl.len(), 1, "the list is the workspace's, and stays");
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}

/// In a session, a workspace is used through the channels (issue 251,
/// decisions 10, 11): by a current member - not its owner alone - whom its
/// access list lets (issue 23), where the channel uses it and it and the
/// channel flow both ways; nobody else, and nothing says why. What the list
/// grants is reading, or changing the files too.
#[tokio::test]
async fn a_workspace_is_used_in_a_channel_by_its_members_as_its_list_lets_them_where_both_flow() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = workspaces.create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), None).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let in_main = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.session(session_id).await.unwrap().channel(main).await.unwrap() }
    };

    assert!(in_main(&bob).await.workspace().await.unwrap().is_none(), "not shared: not his to use, though attached");
    workspaces.share(&alice, docs.id, Subject::MemberOf(Membership::Session(session_id)), Grant::Read).await.unwrap();
    let (usable, asked) = world.asked(async { in_main(&bob).await.workspace().await.unwrap() }).await;
    let bobs = usable.expect("bob, a member of the channel, uses alice's workspace there");
    assert!(asked.ends_with(&checks(&["bob: GetWorkspace(alice's) yes", "bob: Publish yes", "bob: Publish yes"])), "its list, then both ways: {asked:?}");
    assert!(matches!(bobs.terminal(Grant::Write).await, Err(WorkspaceError::ReadOnly(_))), "reading, not changing, where it grants reading");
    workspaces.share(&alice, docs.id, Subject::MemberOf(Membership::Session(session_id)), Grant::Write).await.unwrap();
    let terminal = bobs.terminal(Grant::Write).await.expect("and reaches its files");
    terminal.write(&WorkspacePath::parse("notes/a.md").unwrap(), file("from bob"), 100).await.unwrap();
    let owned = world.manager.views(Viewer::new(alice.clone())).workspace(docs.id).await.unwrap();
    assert_eq!(read(owned.terminal(Grant::Read).await.unwrap().as_ref(), "notes/a.md").await, "from bob", "the same files as its owner's");

    let channel = Arc::clone(&session.channels()[0]);
    assert!(denied(&channel.update_uses_workspace(&bob, false).await.unwrap_err()), "a member does not switch it off");
    channel.update_uses_workspace(&alice, false).await.expect("the channel's owner does");
    assert!(in_main(&bob).await.workspace().await.unwrap().is_none(), "not where the channel does not use it");
    assert!(matches!(bobs.terminal(Grant::Read).await, Err(WorkspaceError::NotFound(_))), "asked again when used");
    channel.update_uses_workspace(&alice, true).await.unwrap();

    workspaces.reclassify(&alice, docs.id, Classification::PUBLIC).await.unwrap();
    assert!(in_main(&bob).await.workspace().await.unwrap().is_none(), "not where what is seen may not flow into it");
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}

/// Issue 23: a workspace is reached by its access list, wherever one acts -
/// not through a channel: who it is shared with reads it, or changes its
/// files as far as the list grants; only its owner shares it, never with an
/// agent; who it is not shared with does not learn it exists.
#[tokio::test]
async fn a_workspace_is_reached_by_its_list_wherever_one_acts() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob, carol) = (World::as_user(world.alice), World::as_user(world.bob), World::as_user(world.carol));
    let docs = workspaces.create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));
    assert!(matches!(views(&bob).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))), "not shared: not there for him");

    let bobs = SecurityAttributes::owned_by(ActorId::User(world.alice)).granting(Subject::Actor(ActorId::User(world.bob)), Grant::Read).acl;
    let (refused, asked) = world.asked(workspaces.set_access(&bob, docs.id, bobs.clone())).await;
    assert!(matches!(refused, Err(WorkspaceError::NotFound(_))), "nobody shares what they cannot read: {refused:?}");
    assert_eq!(asked, checks(&["bob: GetWorkspace(alice's) no"]));
    let (shared, asked) = world.asked(workspaces.set_access(&alice, docs.id, bobs.clone())).await;
    shared.expect("its owner shares it");
    assert_eq!(asked, checks(&["alice: GetWorkspace(alice's) yes", "alice: GetWorkspace(alice's) yes", "alice: UpdateWorkspace(alice's) yes"]), "read, then changed as stored");

    let elsewhere = SecurityContext::new(ActorId::User(world.bob), SessionId::new(), ChannelId::new());
    let his = views(&elsewhere).workspace(docs.id).await.expect("he reads it, wherever he acts");
    his.terminal(Grant::Read).await.expect("its files");
    assert!(matches!(his.terminal(Grant::Write).await, Err(WorkspaceError::ReadOnly(_))), "not to change them");
    let seen = his.snapshot().await;
    assert!(!seen.can_write && !seen.can_change && seen.access.is_empty(), "nor whom else it is shared with");
    let listed: Vec<WorkspaceId> = views(&bob).workspaces().await.unwrap().iter().map(|w| w.id()).collect();
    assert_eq!(listed, [docs.id], "listed among his");
    let owners = views(&alice).workspace(docs.id).await.unwrap().snapshot().await;
    assert_eq!(owners.access.iter().map(|e| e.name.as_deref()).collect::<Vec<_>>(), [Some("bob")], "its owner sees whom, by name");
    assert!(matches!(workspaces.set_access(&bob, docs.id, Vec::new()).await, Err(WorkspaceError::Store(StoreError::PermissionDenied))), "a reader does not change the list");
    assert!(matches!(views(&carol).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))), "nor is it there for carol");

    let writers = SecurityAttributes::owned_by(ActorId::User(world.alice)).granting(Subject::Actor(ActorId::User(world.bob)), Grant::Write).acl;
    workspaces.set_access(&alice, docs.id, writers).await.unwrap();
    views(&bob).workspace(docs.id).await.unwrap().terminal(Grant::Write).await.expect("where it grants writing, he writes");
    let agent = SecurityAttributes::owned_by(ActorId::User(world.alice)).granting(Subject::Actor(ActorId::Agent(AgentId::new())), Grant::Read).acl;
    assert!(matches!(workspaces.set_access(&alice, docs.id, agent).await, Err(WorkspaceError::Store(StoreError::Validation(_)))), "never with an agent (issue 255)");
}

/// Who takes part in the session but not in the channel does not use the
/// channel's workspace there - an agent of alice's waiting in the session,
/// say.
#[tokio::test]
async fn a_workspace_is_not_used_in_a_channel_by_who_is_not_in_it() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let waiting = world.agent("waiting", world.alice, &global).await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[waiting]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let as_agent = alice.next(ActorId::Agent(waiting), ActorId::User(world.alice));
    let main = world.manager.views(Viewer::new(as_agent)).session(session.id()).await.unwrap().channel(session.channels()[0].id()).await.unwrap();
    assert!(main.workspace().await.unwrap().is_none());
    world.manager.stop_session(&alice, session.id()).await.unwrap();
}

/// The content of an artifact's version, as text.
async fn kept(content: caelumora_runtime::terminal::ByteStream) -> String {
    let chunks: Vec<_> = content.collect().await;
    String::from_utf8(chunks.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect()).unwrap()
}

/// How reading version `number` of artifact `id` in `ctx`'s name went, as a
/// frontend reads it - through the views (issue 292): its size and text, the
/// artifact not being there for them, or the version not.
#[derive(Debug, PartialEq)]
enum Read {
    Content(u64, String),
    NoArtifact,
    NoVersion,
}

async fn read_version(world: &World, ctx: &SecurityContext, id: caelumora_model::artifact::ArtifactId, number: u32) -> Read {
    let artifact = match world.manager.views(Viewer::new(ctx.clone())).artifact(id).await {
        Ok(artifact) => artifact,
        Err(ViewError::ArtifactNotFound(_)) => return Read::NoArtifact,
        Err(e) => panic!("{e}"),
    };
    match artifact.content(number).await {
        Ok((size, content)) => Read::Content(size, kept(content).await),
        Err(ArtifactError::VersionNotFound { .. }) => Read::NoVersion,
        Err(e) => panic!("{e}"),
    }
}

/// An artifact (issue 271) is published from the workspace of a channel,
/// in the name of whoever publishes - its owner, who makes it
/// (`CreateArtifact`), or whom its list lets add a version
/// (`WriteArtifact`, issue 23) -, into the session's scope. Its owner reads
/// every version; a version is read by the members of the channel it was
/// published in, by the artifact, wherever they act; only its owner shares
/// and keeps it beyond the session, and what is not kept goes with the
/// session, its content too.
#[tokio::test]
async fn an_artifact_is_published_in_a_channel_for_its_publisher_and_read_by_who_reads_the_channel() {
    let world = World::new().await;
    let artifacts = world.runtime.artifact_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let in_main = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.session(session_id).await.unwrap().channel(main).await.unwrap() }
    };
    let workspace = in_main(&alice).await.workspace().await.unwrap().expect("hers to use");
    let terminal = workspace.terminal(Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();

    let (published, asked) = world.asked(workspace.publish_artifact(&at, "report.csv", Some("The figures"))).await;
    let (artifact, first) = published.expect("published");
    assert_eq!((first, &artifact.name.scope, artifact.owner()), (1, &Scope::SessionScope { session_id }, Some(world.alice)));
    assert!(asked.contains(&"alice: CreateArtifact(alice's) yes".to_string()), "made in her name: {asked:?}");
    terminal.write(&at, file("a,b\n1,2"), 100).await.unwrap();
    let (published, asked) = world.asked(workspace.publish_artifact(&at, "report.csv", None)).await;
    assert_eq!(published.expect("a second version").1, 2);
    assert!(asked.contains(&"alice: WriteArtifact(alice's) yes".to_string()), "added to as hers: {asked:?}");

    assert_eq!(read_version(&world, &alice, artifact.id, 1).await, Read::Content(3, "a,b".to_string()), "its owner reads every version, as it was then");
    let elsewhere = SecurityContext::new(ActorId::User(world.bob), SessionId::new(), ChannelId::new());
    assert_eq!(read_version(&world, &elsewhere, artifact.id, 2).await, Read::Content(7, "a,b\n1,2".to_string()), "a member of the channel reads what was published there, wherever he acts");
    assert_eq!(read_version(&world, &bob, artifact.id, 3).await, Read::NoVersion);
    assert_eq!(read_version(&world, &World::as_user(world.carol), artifact.id, 1).await, Read::NoArtifact, "nobody else, who learns nothing");

    let bobs = in_main(&bob).await.workspace().await.unwrap().expect("bob uses it too");
    let refused = bobs.publish_artifact(&at, "report.csv", None).await;
    assert!(matches!(refused, Err(ArtifactError::Store(StoreError::PermissionDenied))), "nor adds to alice's artifact of that name: {refused:?}");
    let writers = SecurityAttributes::owned_by(ActorId::User(world.alice)).granting(Subject::Actor(ActorId::User(world.bob)), Grant::Write).acl;
    assert!(matches!(artifacts.set_access(&bob, artifact.id, writers.clone()).await, Err(ArtifactError::NotFound(_))), "nor shares it");
    artifacts.set_access(&alice, artifact.id, writers).await.expect("its owner shares it");
    let (added, third) = bobs.publish_artifact(&at, "report.csv", None).await.expect("whom it is shared with to write adds a version");
    assert_eq!((third, added.owner()), (3, Some(world.alice)), "hers still");
    assert!(matches!(artifacts.keep(&bob, artifact.id, Scope::UserScope { user_id: world.bob }).await, Err(ArtifactError::Store(StoreError::PermissionDenied))), "nor keeps it");
    let kept_one = artifacts.keep(&alice, artifact.id, Scope::UserScope { user_id: world.alice }).await.expect("its owner keeps it");
    assert_eq!(kept_one.name.scope, Scope::UserScope { user_id: world.alice });
    let (left, _) = workspace.publish_artifact(&at, "draft.csv", None).await.unwrap();

    world.manager.stop_session(&alice, session_id).await.unwrap();
    world.manager.delete_session(&alice, session_id).await.unwrap();
    world.manager.purge_session(&World::system(), session_id).await.unwrap();
    assert!(world.store.get_artifact(&alice, artifact.id).await.is_ok(), "what was kept stays");
    assert!(world.files.path().join("artifacts").join(artifact.id.to_string()).is_dir(), "with its content");
    assert!(world.store.get_deleted_artifact(&World::system(), left.id).await.is_err(), "what was not kept went with the session");
    assert!(!world.files.path().join("artifacts").join(left.id.to_string()).exists(), "its content too");
}

/// Issue 23: a version is read by the members of the channel it was
/// published in, besides whom the artifact is shared with - so drafts
/// published where alice works alone reach nobody who reads the version
/// she published to the team, nor does what she said of them.
#[tokio::test]
async fn drafts_of_an_artifact_stay_with_the_channel_they_were_published_in() {
    let world = World::new().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let drafts = session.create_channel(&alice, ChannelSpec { name: "drafts".to_string(), components: quiet(), participants: vec![], classification: None, language: None, uses_workspace: None }).await.expect("hers alone").id();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let workspace_in = |channel| {
        let views = world.manager.views(Viewer::new(alice.clone()));
        async move { views.session(session_id).await.unwrap().channel(channel).await.unwrap().workspace().await.unwrap().expect("hers to use") }
    };
    let at = WorkspacePath::parse("report.md").unwrap();
    let (in_drafts, in_main) = (workspace_in(drafts).await, workspace_in(main).await);
    in_drafts.terminal(Grant::Write).await.unwrap().write(&at, file("rough"), 100).await.unwrap();
    let (artifact, _) = in_drafts.publish_artifact(&at, "report.md", Some("Still wrong about Q3")).await.expect("a draft");
    in_main.terminal(Grant::Write).await.unwrap().write(&at, file("final"), 100).await.unwrap();
    let (_, final_one) = in_main.publish_artifact(&at, "report.md", Some("The report")).await.expect("the final one");
    assert_eq!(final_one, 2);

    let his = world.manager.views(Viewer::new(bob.clone())).artifact(artifact.id).await.expect("bob reads what reached the team").snapshot().await;
    assert_eq!(his.artifact.versions.iter().map(|v| v.number).collect::<Vec<_>>(), [2], "the final one alone");
    assert_eq!(his.artifact.versions[0].description.as_ref().map(|d| d.to_string()), Some("The report".to_string()));
    assert!(his.artifact.description.is_none() && his.access.is_empty() && his.version_access.is_empty(), "nothing said of the draft, nor whom it reaches");
    assert_eq!(read_version(&world, &bob, artifact.id, 1).await, Read::NoVersion, "not the draft");
    assert_eq!(read_version(&world, &bob, artifact.id, 2).await, Read::Content(5, "final".to_string()), "the final one");

    let hers = world.manager.views(Viewer::new(alice.clone())).artifact(artifact.id).await.unwrap().snapshot().await;
    assert_eq!(hers.artifact.versions.len(), 2, "its owner reads both");
    let names: Vec<Option<String>> = hers.version_access.values().map(|access| access[0].name.clone()).collect();
    assert_eq!(names, [Some("drafts".to_string()), Some("main".to_string())], "and sees where each reaches");
    world.manager.stop_session(&alice, session_id).await.unwrap();
}

/// What is asked when the agent of `owner`, asked by bob, makes `call` in the
/// session's workspace, where the files `present` lie already - one that
/// leaves `made`, a file of the workspace.
async fn asked_for_a_call_on_the_workspace(function: &str, arguments: serde_json::Value, present: &[&str], made: &str, owner: fn(&World) -> UserId) -> Vec<String> {
    let kind = caelumora_runtime::tool::terminal_tool::TERMINAL_TOOL_KIND;
    asked_for_a_call(kind, function, arguments, present, |_, place| place.join(made).is_file(), owner).await
}

/// What is asked when the agent of `owner`, with a component of `kind`,
/// asked by bob, makes `call` in the session's workspace, where the files
/// `present` lie already - until `done` says the call arrived, given where
/// the storages keep their files and where the workspace's lie.
async fn asked_for_a_call(kind: &str, function: &str, arguments: serde_json::Value, present: &[&str], done: impl Fn(&std::path::Path, &std::path::Path) -> bool, owner: fn(&World) -> UserId) -> Vec<String> {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: function.to_string(), arguments, visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with_kinds(|registry| registry.register(ComponentKind::new("once").provides::<dyn caelumora_runtime::llm::LLMClient>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn caelumora_runtime::llm::LLMClient>(Arc::<Once>::clone(&model));
        Ok(facets)
    })).await;
    let working = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "once"),
            ComponentDescriptor {
                params: serde_json::json!({ "requires_approval": false }),
                ..ComponentDescriptor::new("workspace", kind)
            },
        ] },
        ..world.personality("working", None, &[]).await
    };
    world.store.update_personality(&World::system(), &working).await.unwrap();
    let helper = world.agent("helper", owner(&world), &working).await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "shell", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[helper], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let main = Arc::clone(&session.channels()[0]);

    let place = world.files.path().join("shell").join(docs.id.to_string());
    for file in present {
        std::fs::create_dir_all(place.join(file).parent().unwrap()).unwrap();
        std::fs::write(place.join(file), "present").unwrap();
    }
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper do it".to_string(), None).await.expect("sent");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !done(world.files.path(), &place) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.expect("the call reached the workspace");
    }).await;
    world.manager.stop_session(&alice, session.id()).await.unwrap();
    asked
}

/// That `asked` asked for an agent's call on the workspace: all of it in
/// one context (issue 255) - the call and the workspace with alice's
/// rights, its owner's, asked by bob - and the workspace through the
/// channel as the agent reads every other place, with its owner's
/// clearance (issue 142): its terminal reached through the session, its
/// access list asked (`GetWorkspace`, issue 23), its flow both ways, before
/// anything is sent to the storage.
fn asked_as_a_call_on_the_workspace(asked: &[String]) {
    let calling: Vec<&String> = asked.iter().filter(|line| line.contains("CallTool")).collect();
    assert!(!calling.is_empty() && calling.iter().all(|line| *line == "alice as helper, asked by bob: CallTool yes"), "the call as its owner, asked by bob: {asked:?}");
    let agents: Vec<&String> = asked.iter().filter(|line| line.contains("as helper")).collect();
    assert!(agents.iter().all(|line| line.starts_with("alice as helper, asked by bob:") && line.ends_with(" yes")), "one context for all the turn does: {asked:?}");
    assert!(agents.contains(&&"alice as helper, asked by bob: GetSession(alice's) yes".to_string()), "its terminal through the session: {asked:?}");
    let listed: Vec<&String> = agents.iter().copied().filter(|line| line.contains("Workspace")).collect();
    assert!(listed.iter().any(|line| line.contains("GetWorkspace")), "its list asked: {asked:?}");
    assert!(listed.iter().all(|line| line.ends_with("Workspace(alice's) yes")), "and only of the workspace attached: {asked:?}");
    assert!(agents.iter().filter(|line| line.contains("Publish")).count() >= 4, "the flow both ways, for the workspace and its terminal: {asked:?}");
}

/// An agent's command (issue 251).
#[tokio::test]
async fn an_agents_command_is_called_for_its_owner_and_reaches_the_workspace_through_its_channel() {
    let asked = asked_for_a_call_on_the_workspace("run_command", serde_json::json!({ "command": "echo ran > ran.txt" }), &[], "ran.txt", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: WriteWorkspace(alice's) yes".to_string()), "a command may change anything: {asked:?}");
}

/// An agent's change of a file (issue 251, step 9) asks what a command asks.
#[tokio::test]
async fn an_agents_file_tool_asks_what_a_command_asks() {
    let asked = asked_for_a_call_on_the_workspace("write_file", serde_json::json!({ "path": "notes/a.md", "content": "written" }), &[], "notes/a.md", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: WriteWorkspace(alice's) yes".to_string()), "a change of its files: {asked:?}");
}

/// An agent's move of a file (issue 267) asks what any change of a file
/// asks.
#[tokio::test]
async fn an_agents_move_of_a_file_asks_what_a_change_asks() {
    let asked = asked_for_a_call_on_the_workspace("move_file", serde_json::json!({ "from": "notes/a.md", "to": "archive/a.md" }), &["notes/a.md"], "archive/a.md", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
}

/// An agent's publication of an artifact (issue 271) asks what a call on
/// the workspace asks, and makes the artifact its owner's - asked in the
/// same context, `CreateArtifact` for alice.
#[tokio::test]
async fn an_agents_artifact_is_published_for_its_owner() {
    let kind = caelumora_runtime::tool::artifact_tool::ARTIFACT_TOOL_KIND;
    let kept = |files: &std::path::Path, _: &std::path::Path| std::fs::read_dir(files.join("artifacts")).is_ok_and(|mut entries| entries.next().is_some());
    let asked = asked_for_a_call(kind, "publish_artifact", serde_json::json!({ "path": "notes/a.md" }), &["notes/a.md"], kept, |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: CreateArtifact(alice's) yes".to_string()), "its owner's: {asked:?}");
}

/// An agent reaches the workspace of a session its owner does not take
/// part in (issue 264): carol's agent, in alice's session with bob, reaches
/// the terminal through the session it takes part in itself - read as
/// carol, by the agent's own taking part.
#[tokio::test]
async fn an_agent_reaches_the_workspace_of_a_session_its_owner_does_not_take_part_in() {
    let asked = asked_for_a_call_on_the_workspace("run_command", serde_json::json!({ "command": "echo ran > ran.txt" }), &[], "ran.txt", |world| world.carol).await;
    assert!(asked.contains(&"carol as helper, asked by bob: GetSession(alice's) yes".to_string()), "the session it takes part in: {asked:?}");
}

/// A template of the system whose sessions have a workspace "files" in
/// `scope`, in `storage`.
async fn template_with_workspace(world: &World, scope: ScopeTemplate, storage: &str) -> SessionTemplateId {
    let global = world.personality(&format!("global-{storage}"), None, &[]).await;
    let id = world.template(None, &[], &format!("helper-{storage}"), global.id).await;
    let mut template = world.store.get_session_template(&World::system(), id).await.unwrap();
    template.workspace = Some(WorkspaceTemplate { name: Name::new("files").unwrap(), scope, storage: storage.to_string(), classification: Classification::general(Confidentiality::Internal), description: None });
    world.store.update_session_template(&World::system(), &template).await.unwrap();
    id
}

/// A session made from a template that names a workspace in the session's
/// scope gets one of its own, made with it and attached (issue 251,
/// decision 13) - its owner's - which is used in that session alone and
/// goes with it: deleted, restored and purged with it, its files too.
#[tokio::test]
async fn a_workspace_of_a_session_s_scope_is_made_with_it_and_goes_with_it() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let template = template_with_workspace(&world, ScopeTemplate::SessionScope, "main").await;
    let first = world.manager.create_from_template(&alice, template, None, None).await.expect("a session with its workspace");
    let second = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    let (mine, others) = (first.session().workspace.expect("attached"), second.session().workspace.expect("attached"));
    assert_ne!(mine, others, "one for every session");
    let made = world.store.get_workspace(&alice, mine).await.expect("hers");
    assert_eq!(made.name.scope, Scope::SessionScope { session_id: first.id() });

    assert_eq!(made.security_attributes.acl, [caelumora_model::security::AclEntry { subject: Subject::MemberOf(Membership::Session(first.id())), grant: Grant::Write }], "its members' to work on (issue 23)");
    let owned = world.manager.views(Viewer::new(alice.clone())).workspace(mine).await.unwrap();
    assert!(owned.terminal(Grant::Write).await.is_ok(), "used directly, wherever who may use it acts (issue 23)");
    let refused = second.set_workspace(&alice, Some(mine), None).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::WorkspaceOutOfScope(_))), "nor attached to another session: {refused}");

    world.manager.delete_session(&alice, first.id()).await.unwrap();
    assert!(world.store.get_deleted_workspace(&alice, mine).await.is_ok(), "deleted with it");
    world.manager.restore_session(&alice, first.id()).await.unwrap();
    assert!(world.store.get_workspace(&alice, mine).await.is_ok(), "restored with it");
    world.manager.delete_session(&alice, first.id()).await.unwrap();
    world.manager.purge_session(&World::system(), first.id()).await.unwrap();
    assert!(!world.files.path().join(mine.to_string()).exists(), "purged with it, its files too");
    assert!(world.files.path().join(others.to_string()).is_dir(), "the other session's stays");
    world.manager.stop_session(&alice, second.id()).await.unwrap();
}

/// A template that names a workspace in its owner's scope finds it there:
/// every session of the user made from it shares the one (issue 251,
/// decision 13); one of that name in another storage is neither reused
/// nor replaced.
#[tokio::test]
async fn a_workspace_of_the_user_s_scope_is_found_by_every_session_of_theirs() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let template = template_with_workspace(&world, ScopeTemplate::UserScope, "main").await;
    let first = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    let second = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    assert_eq!(first.session().workspace, second.session().workspace, "the same one");
    let bobs = world.manager.create_from_template(&World::as_user(world.bob), template, None, None).await.unwrap();
    assert_ne!(bobs.session().workspace, first.session().workspace, "bob's own");

    let elsewhere = template_with_workspace(&world, ScopeTemplate::UserScope, "other").await;
    let refused = world.manager.create_from_template(&alice, elsewhere, None, None).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<TemplateError>(), Some(TemplateError::WorkspaceNameTaken(_))), "{refused}");
    for session in [first, second] {
        world.manager.stop_session(&alice, session.id()).await.unwrap();
    }
    world.manager.stop_session(&World::as_user(world.bob), bobs.id()).await.unwrap();
}

/// A workspace chosen as a session is made takes the place of its
/// template's, which is then neither found nor made (issue 266): none; one
/// of its maker's own - asked about as attaching it to a running session,
/// before anything is made, and never another's nor one of another
/// session's scope; or one made for the session alone, in its scope and
/// named as it, which a classification its maker is not cleared for
/// refuses, leaving no session behind.
#[tokio::test]
async fn a_workspace_chosen_as_a_session_is_made_takes_the_place_of_its_template_s() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);
    let template = template_with_workspace(&world, ScopeTemplate::UserScope, "main").await;
    let hers = workspaces.create(&alice, "hers", Scope::UserScope { user_id: world.alice }, "main", None, internal.clone()).await.unwrap();
    let his = workspaces.create(&bob, "his", Scope::UserScope { user_id: world.bob }, "main", None, internal.clone()).await.unwrap();
    let views = || world.manager.views(Viewer::new(alice.clone()));

    let without = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::None)).await.expect("without one");
    assert_eq!(without.session().workspace, None);

    let (attached, asked) = world.asked(world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: hers.id, share: None }))).await;
    let attached = attached.expect("hers");
    assert_eq!(attached.session().workspace, Some(hers.id));
    assert_eq!(asked[..3], checks(&["alice: GetSessionTemplate(system's) yes", "alice: GetWorkspace(alice's) yes", "alice: UpdateWorkspace(alice's) yes"]), "{asked:?}");
    assert!(!asked.iter().any(|line| line.contains("UpdateSession")), "she is making the session: {asked:?}");

    let (refused, asked) = world.asked(world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: his.id, share: None }))).await;
    assert!(not_found(&refused.unwrap_err()), "not bob's");
    assert_eq!(asked, checks(&["alice: GetSessionTemplate(system's) yes", "alice: GetWorkspace(bob's) no"]), "refused before anything is made");

    let made = world.manager.create_from_template(&alice, template, Some("Report"), Some(WorkspaceChoice::New { storage: "main".to_string(), classification: internal.clone() })).await.expect("one of its own");
    let own = world.store.get_workspace(&alice, made.session().workspace.expect("attached")).await.unwrap();
    assert_eq!((own.name.name.as_str(), &own.name.scope, own.storage.as_str()), ("Report", &Scope::SessionScope { session_id: made.id() }, "main"));
    assert_eq!(own.owner(), Some(world.alice));
    let refused = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: own.id, share: None })).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::WorkspaceOutOfScope(_))), "not of another session's scope: {refused}");

    let sessions = views().sessions().await.unwrap().len();
    let confidential = Classification::general(Confidentiality::Confidential);
    let refused = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::New { storage: "main".to_string(), classification: confidential })).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<WorkspaceError>(), Some(WorkspaceError::NotCleared)), "{refused}");
    assert_eq!(views().sessions().await.unwrap().len(), sessions, "no session left behind");

    let names: Vec<String> = views().workspaces().await.unwrap().iter().map(|w| w.name().to_string()).collect();
    assert!(!names.contains(&"files".to_string()), "the template's was never made: {names:?}");
    for session in [without, attached, made] {
        world.manager.stop_session(&alice, session.id()).await.unwrap();
    }
}

// --- Commands an agent carries out itself (issue 269) ------------------------

/// A component whose agent carries out `/mark`, for whoever may write in the
/// channel, and `/seal`, for its owners alone; it keeps the context of every
/// turn it carries one out in.
#[derive(Debug)]
struct Commanded(Arc<Mutex<Vec<SecurityContext>>>);

impl caelumora_runtime::command::ProvidesCommands for Commanded {
    fn commands(&self) -> Vec<caelumora_model::command::Command> {
        let command = |name: &str, owners_only| caelumora_model::command::Command { name: name.to_string(), description: String::new(), usage: None, owners_only };
        vec![command("mark", false), command("seal", true)]
    }
}

#[async_trait]
impl caelumora_runtime::command::AgentCommandProvider for Commanded {
    async fn execute(&self, name: &str, _args: &str, turn: &mut caelumora_runtime::agent::turn::Turn<'_>) -> caelumora_runtime::command::CommandResult<caelumora_model::command::CommandOutput> {
        self.0.lock().unwrap().push(turn.security_context().clone());
        Ok(caelumora_model::command::CommandOutput::private(format!("{name}: done")))
    }
}

/// A session of alice and bob with alice's `helper`, which carries out the
/// commands of `Commanded`; what its turns ran in, and how bob's stream
/// shows the answers to his commands.
async fn with_a_commanded_agent() -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>, Arc<Mutex<Vec<SecurityContext>>>, caelumora_runtime::actor::user::ChannelWatch) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let world = World::with_kinds(|registry| {
        register_once(registry, "silent", caelumora_model::message::Message::new(Classification::PUBLIC));
        let seen = Arc::clone(&seen);
        registry.register(ComponentKind::new("commanded").provides::<dyn caelumora_runtime::command::AgentCommandProvider>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn caelumora_runtime::command::AgentCommandProvider>(Arc::new(Commanded(Arc::clone(&seen))));
            Ok(facets)
        });
    }).await;
    let commanded = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("commands", "commanded"),
        ] },
        ..world.personality("commanded", None, &[]).await
    };
    world.store.update_personality(&World::system(), &commanded).await.unwrap();
    let helper = world.agent("helper", world.alice, &commanded).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    (world, session, main, seen, his)
}

/// What was asked about the commands of agents, of `asked`.
fn about_agent_commands(asked: &[String]) -> Vec<&str> {
    asked.iter().filter(|line| line.contains("AgentCommand")).map(String::as_str).collect()
}

/// A command of an agent (issue 269) is seen and asked for by whoever may
/// write in the channel - bob, taking part -, unless it is for the agent's
/// owners alone: `/seal` bob neither sees nor asks for, alice, the agent's
/// owner, does. Bob's actor asks before it sends, and the agent asks again,
/// in the context the request came with, before it carries it out.
#[tokio::test]
async fn a_bystander_asks_an_agent_for_its_command_but_not_for_one_of_its_owners() {
    let (world, session, main, _, mut his) = with_a_commanded_agent().await;
    let bob = main.user_actor(world.bob).unwrap();

    let ((sent, answer), asked) = world.asked(async {
        let sent = bob.execute_command("mark", "@helper").await;
        (sent, shown(&mut his, "CommandExecuted").await)
    }).await;
    sent.expect("his to ask for");
    assert!(matches!(answer, caelumora_model::actor::ActorEvent::CommandExecuted { ref output, .. } if output == "mark: done"), "{answer:?}");
    assert_eq!(about_agent_commands(&asked), [
        "bob: ShowAgentCommand(alice's) yes",
        "bob: ShowAgentCommand(alice's) no",
        "bob: ShowAgentCommand(alice's) yes",
        "bob: ExecuteAgentCommand(alice's) yes",
        "bob: ShowAgentCommand(alice's) yes",
        "bob: ExecuteAgentCommand(alice's) yes",
    ], "what he sees, what he may ask for, and the agent asking again");

    let refused = bob.execute_command("seal", "@helper").await.expect_err("not his");
    assert!(matches!(refused.downcast_ref::<caelumora_runtime::command::CommandError>(), Some(caelumora_runtime::command::CommandError::Unknown(name)) if name == "seal"), "unknown to him: {refused}");
    let mut hers = main.user_actor(world.alice).unwrap().watch();
    main.user_actor(world.alice).unwrap().execute_command("seal", "@helper").await.expect("hers, as its owner");
    assert!(matches!(shown(&mut hers, "CommandExecuted").await, caelumora_model::actor::ActorEvent::CommandExecuted { ref output, .. } if output == "seal: done"));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent carries a command out for its owner, on the word of whoever
/// asked (issue 255): bob's `/mark` runs in a turn whose caller is the agent,
/// whose principal is alice, and whose initiator is bob.
#[tokio::test]
async fn an_agents_command_runs_for_its_owner_on_the_word_of_whoever_asked() {
    let (world, session, main, seen, mut his) = with_a_commanded_agent().await;
    let helper = main.members().into_iter().find(|m| m.name == "helper").expect("helper takes part").id;

    main.user_actor(world.bob).unwrap().execute_command("mark", "@helper").await.expect("his to ask for");
    shown(&mut his, "CommandExecuted").await;
    let turn = seen.lock().unwrap().pop().expect("carried out");
    assert_eq!((turn.caller_id, turn.principal_id, turn.initiator_id), (helper, ActorId::User(world.alice), ActorId::User(world.bob)));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Compaction (issue 16) --------------------------------------------------------

/// Compacting a shared agent's context is its owners' (issue 16, `owners_only`
/// as for approving its calls): bob, who may write in the channel, neither
/// sees nor asks for `/compact` of alice's `helper`; alice does, and the
/// agent carries it out for her - here refusing, since it heard nothing yet.
#[tokio::test]
async fn only_an_agents_owner_compacts_its_context() {
    let world = World::with_kinds(|registry| register_once(registry, "silent", caelumora_model::message::Message::new(Classification::PUBLIC))).await;
    let compacting = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("compaction", caelumora_runtime::compaction::command::COMPACTION_KIND),
        ] },
        ..world.personality("compacting", None, &[]).await
    };
    world.store.update_personality(&World::system(), &compacting).await.unwrap();
    let helper = world.agent("helper", world.alice, &compacting).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let (refused, asked) = world.asked(main.user_actor(world.bob).unwrap().execute_command("compact", "@helper")).await;
    let refused = refused.expect_err("not his");
    assert!(matches!(refused.downcast_ref::<caelumora_runtime::command::CommandError>(), Some(caelumora_runtime::command::CommandError::Unknown(name)) if name == "compact"), "unknown to him: {refused}");
    assert!(about_agent_commands(&asked).iter().all(|line| line.ends_with(" no")), "never granted to him: {asked:?}");

    let mut hers = main.user_actor(world.alice).unwrap().watch();
    main.user_actor(world.alice).unwrap().execute_command("compact", "@helper").await.expect("hers, as its owner");
    assert!(matches!(shown(&mut hers, "CommandFailed").await, caelumora_model::actor::ActorEvent::CommandFailed { ref reason, .. } if reason.contains("nothing to compact")),
        "carried out for her by the agent, which has nothing to compact yet");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Skills (issue 22) --------------------------------------------------------

/// A session of alice and bob with alice's `helper`, which has the `skills`
/// component, and a skill of alice's, `release-notes`, classified as the
/// channel; and how bob's stream shows him what comes of his commands.
async fn with_a_skilled_agent() -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>, AgentId, caelumora_runtime::actor::user::ChannelWatch) {
    let world = World::with_kinds(|registry| register_once(registry, "silent", caelumora_model::message::Message::new(Classification::PUBLIC))).await;
    let skilled = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("skills", caelumora_runtime::component_impl::skills::SKILLS_KIND),
        ] },
        ..world.personality("skilled", None, &[]).await
    };
    world.store.update_personality(&World::system(), &skilled).await.unwrap();
    let helper = world.agent("helper", world.alice, &skilled).await;
    let internal = Classification::general(Confidentiality::Internal);
    world.runtime.skill_manager()
        .create(&World::as_user(world.alice), "release-notes", Scope::UserScope { user_id: world.alice }, "How we write a release note", "1. Collect.\n2. Group.", internal).await
        .expect("hers to make");
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    (world, session, main, helper, his)
}

/// A skill loaded by a command (issue 22) is loaded by the agent for its
/// owner, on the word of whoever asked: bob's `/skill` loads alice's skill
/// into her agent - `UseSkill` asked as the agent for alice, bob the
/// initiator. Its text goes into the agent's transcript in bob's name, and
/// the channel's history records the command, not the text.
#[tokio::test]
async fn an_agent_loads_a_skill_for_its_owner_whoever_asked() {
    let (world, session, main, helper, mut his) = with_a_skilled_agent().await;

    let (answer, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().execute_command("skill", "@helper release-notes").await.expect("his to give");
        shown(&mut his, "CommandExecuted").await
    }).await;
    assert!(matches!(answer, caelumora_model::actor::ActorEvent::CommandExecuted { ref output, .. } if output.contains("release-notes")), "{answer:?}");
    let using: Vec<&String> = asked.iter().filter(|line| line.contains("UseSkill")).collect();
    assert_eq!(using, ["alice as helper, asked by bob: UseSkill(alice's) yes"], "for its owner, asked by him");

    let state = world.store.get_agent_channel_state(helper, main.id()).await.unwrap();
    let given = state.transcript.messages().into_iter().last().expect("given");
    assert!(given.content.as_deref().is_some_and(|c| c.starts_with("[Skill: release-notes]\n1. Collect.")), "{given:?}");
    assert_eq!((given.sender_id, given.role), (ActorId::User(world.bob), caelumora_model::message::MessageRole::User), "in his name");
    let history = world.store.get_channel_state(main.id()).await.unwrap().history;
    assert!(history.messages().iter().all(|m| !m.content.as_deref().unwrap_or_default().contains("Collect.")), "nothing of the text in the channel's history");
    assert!(history.items().iter().any(|item| matches!(item, caelumora_model::message_history::HistoryEntry::CommandExecuted { command, issuer, .. } if command == "skill" && *issuer == ActorId::User(world.bob))), "but the command");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A skill classified above the channel (issue 22) may not flow into it:
/// the agent neither lists it nor loads it, and answers for it as for one
/// that does not exist.
#[tokio::test]
async fn a_skill_classified_above_the_channel_is_neither_listed_nor_loaded() {
    let (world, session, main, _, mut his) = with_a_skilled_agent().await;
    world.runtime.skill_manager()
        .create(&World::system(), "secret-plan", Scope::GlobalScope, "What is not said", "Nothing.", Classification::general(Confidentiality::StrictlyConfidential)).await
        .expect("the system's");
    let bob = main.user_actor(world.bob).unwrap();

    bob.execute_command("skills", "@helper").await.expect("his to ask");
    let caelumora_model::actor::ActorEvent::CommandExecuted { output, .. } = shown(&mut his, "CommandExecuted").await else { unreachable!() };
    assert!(output.contains("release-notes") && !output.contains("secret-plan"), "{output}");

    bob.execute_command("skill", "@helper secret-plan").await.expect("sent");
    let caelumora_model::actor::ActorEvent::CommandFailed { reason, .. } = shown(&mut his, "CommandFailed").await else { unreachable!() };
    assert!(reason.contains("no skill named \"secret-plan\""), "as absent: {reason}");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A session of alice and bob with alice's `helper`, whose model calls the
/// tool `function` with `arguments` once it is asked, and which writes
/// skills without approval; and a skill of bob's, `review`, shared with
/// alice.
async fn with_a_writing_agent(function: &str, arguments: serde_json::Value) -> (World, Arc<SessionLoop>, Arc<caelumora_runtime::channel::ChannelLoop>) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: function.to_string(), arguments, visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let world = World::with_kinds(|registry| register_once(registry, "writing", call)).await;
    let writing = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "writing"),
            ComponentDescriptor { params: serde_json::json!({ "requires_approval": false }), ..ComponentDescriptor::new("skills", caelumora_runtime::component_impl::skills::SKILLS_KIND) },
        ] },
        ..world.personality("writing", None, &[]).await
    };
    world.store.update_personality(&World::system(), &writing).await.unwrap();
    let helper = world.agent("helper", world.alice, &writing).await;
    let review = world.runtime.skill_manager()
        .create(&World::as_user(world.bob), "review", Scope::GlobalScope, "How we review", "Read it twice.", Classification::general(Confidentiality::Internal)).await
        .expect("his to make");
    let shared = caelumora_model::skill::Skill { security_attributes: SecurityAttributes::owned_by(ActorId::User(world.bob)).granting(caelumora_model::security::Subject::Actor(ActorId::User(world.alice)), caelumora_model::security::Grant::Read), ..review };
    world.store.update_skill(&World::as_user(world.bob), &shared).await.expect("shared with alice");
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (world, session, main)
}

/// A skill an agent writes (issue 22) is its owner's - asked as the agent
/// for its owner, whoever asked - and lies in the session's scope it asked
/// for, classified as what its turn had seen.
#[tokio::test]
async fn an_agents_skill_belongs_to_its_owner_and_lies_in_the_session() {
    let arguments = serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.", "scope": "session" });
    let (world, session, main) = with_a_writing_agent("create_skill", arguments).await;

    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper write it down".to_string(), None).await.expect("sent");
        asked_for(&world, "alice as helper, asked by bob: CreateSkill(alice's) yes").await;
    }).await;
    assert!(asked.iter().any(|line| line == "alice as helper, asked by bob: CreateSkill(alice's) yes"), "{asked:?}");
    let skills = world.store.list_skills(&World::system()).await.unwrap();
    let deploy = skills.iter().find(|s| s.name.name.as_str() == "deploy").expect("written");
    assert_eq!(deploy.security_attributes.owner_id, ActorId::User(world.alice), "the agent's owner's");
    assert_eq!(deploy.name.scope, Scope::SessionScope { session_id: session.id() }, "in the session's scope");
    assert!(deploy.classification.surely_flows_to(&main.classification()), "classified as what the turn saw, never above the channel");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A skill shared with the agent's owner is loaded, not changed: changing a
/// skill (`UpdateSkill`) is its owner's alone, also through an agent.
#[tokio::test]
async fn an_agent_changes_only_skills_of_its_owners() {
    let arguments = serde_json::json!({ "name": "review", "body": "Skip it." });
    let (world, session, main) = with_a_writing_agent("update_skill", arguments).await;

    main.user_actor(world.bob).unwrap().send_message("@helper change it".to_string(), None).await.expect("sent");
    asked_for(&world, "alice as helper, asked by bob: UpdateSkill(bob's) no").await;
    let review = world.store.find_skill_by_name(&World::system(), &ScopedName::new("review", Scope::GlobalScope).unwrap()).await.unwrap();
    assert_eq!(review.body.as_str(), "Read it twice.", "unchanged");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}
