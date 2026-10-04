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
use caelumora_model::channel::{moderation_components, Channel, HistoryAccess, Participant};
use caelumora_model::command::default_commands;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::confidentiality::{Classification, Clearance, Compartments, ConfidentialityRules, Confidentiality, Roles};
use caelumora_model::name::{Name, ScopedName};
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{ActionType, OwnershipSecurityManager, SecurityAttributes, SecurityContext, SecurityManager};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::store::{EntityStore, StateStore, StoreError};
use caelumora_model::store_impl::memory::MemoryStore;
use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use caelumora_model::user::{User, UserId, UserProfile};
use caelumora_runtime::channel::ChannelError;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use caelumora_runtime::external::{ExternalAgent, ExternalAgentResult, ExternalEvent, ExternalStream, OutboundMessage};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::{ChannelSpec, SessionLoop, SessionManager};
use caelumora_runtime::view::{ViewError, Viewer};

/// The ownership policy, recording every question it is asked - who asked,
/// what, of whose entity, and what it answered - so that a test says which
/// permissions an action needs (issue 198).
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
        let who = match (context.initiator_id, context.caller_id) {
            (initiator, caller) if caller == initiator || caller == ActorId::None => self.name(initiator),
            (initiator, caller) => format!("{} as {}", self.name(initiator), self.name(caller)),
        };
        let answer = if allowed { "yes" } else { "no" };
        self.asked.lock().unwrap().push(format!("{who}: {} {answer}", self.label(&action)));
        allowed
    }
}

/// The attributes of the entity `action` is about, if it is about one.
fn attributes_of<'a>(action: &ActionType<'a>) -> Option<&'a SecurityAttributes> {
    use ActionType::*;
    match *action {
        GetPersonality(a) | CreatePersonality(a) | UpdatePersonality(a) | DeletePersonality(a)
        | GetSessionTemplate(a) | CreateSessionTemplate(a) | UpdateSessionTemplate(a) | DeleteSessionTemplate(a)
        | CreateAgent(a) | GetAgent(a) | ReadAgentContext(a) | UpdateAgent(a) | DeleteAgent(a)
        | CreateSession(a) | GetSession(a) | UpdateSession(a) | DeleteSession(a) | StopSession(a)
        | GetChannel(a) | CreateChannel(a) | UpdateChannel(a) | DeleteChannel(a)
        | GetUser(a) | CreateUser(a) | UpdateUser(a) | DeleteUser(a)
        | GetUserProfile(a) | CreateUserProfile(a) | UpdateUserProfile(a) | DeleteUserProfile(a)
        | CancelChannel(a) | WakeChannel(a)
        | GetComponent(a) | CreateComponent(a) | UpdateComponent(a) | DeleteComponent(a) => Some(a),
        _ => None,
    }
}

/// The runtime with the ownership policy, and three users: alice and bob
/// on the staff, carol a stranger to everything of theirs.
struct World {
    policy: Arc<Recorder>,
    store: Arc<MemoryStore>,
    manager: Arc<SessionManager>,
    alice: UserId,
    bob: UserId,
    carol: UserId,
}

impl World {
    async fn new() -> Self {
        Self::with_kinds(|_| {}).await
    }

    /// The world, with the component kinds `register` adds besides the
    /// built-in ones.
    async fn with_kinds(register: impl FnOnce(&mut ComponentRegistry)) -> Self {
        let roles = Roles::new([("staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }])]).unwrap();
        let rules = ConfidentialityRules::new(Compartments::new([]).unwrap()).with_roles(roles);
        let recorder = Arc::new(Recorder { policy: OwnershipSecurityManager::with_rules(rules), asked: Mutex::new(Vec::new()), names: Mutex::new(HashMap::new()) });
        let policy: Arc<dyn SecurityManager> = recorder.clone();
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let mut registry = ComponentRegistry::built_in(store.clone());
        register(&mut registry);
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), registry).unwrap());
        let manager = Arc::new(SessionManager::new(runtime));
        let mut world = Self { policy: recorder, store, manager, alice: UserId::new(), bob: UserId::new(), carol: UserId::new() };
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
        let user = User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: vec!["staff".to_string()] };
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
            description: format!("{name}, privately described"),
            introduction: Some(format!("I am {name}.")),
            components: Default::default(),
            security_attributes: attributes(owner, contributors),
            reasoning_visibility: Default::default(),
        };
        self.store.create_personality(&Self::system(), &personality).await.unwrap();
        personality
    }

    /// A template of `owner` - the system's if `None` - shared with
    /// `contributors`: its owner and an agent `name` of `personality`, of
    /// the owner's scope, in one channel.
    async fn template(&self, owner: Option<UserId>, contributors: &[UserId], agent: &str, personality: PersonalityId) -> SessionTemplateId {
        let template = SessionTemplate {
            id: SessionTemplateId::new(),
            name: ScopedName::new(format!("with-{agent}"), Scope::GlobalScope).unwrap(),
            description: String::new(),
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Agent { name: Name::new(agent).unwrap(), scope: ScopeTemplate::UserScope, personality_id: personality },
            ],
            channels: vec![ChannelTemplate {
                name: Name::new("main").unwrap(),
                components: quiet(),
                classification: Classification::general(Confidentiality::Internal),
                participants: vec!["user".to_string(), agent.to_string()],
            }],
            security_attributes: attributes(owner, contributors),
            commands: Default::default(),
            privacy: Default::default(),
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
        let alice = ActorId::User(self.alice);
        let users = [Participant::User(self.alice), Participant::User(self.bob)];
        let in_channel: Vec<Participant> = users.iter().cloned().chain(agents.iter().map(|a| Participant::Agent(*a))).collect();
        let in_session: Vec<Participant> = in_channel.iter().cloned().chain(waiting.iter().map(|a| Participant::Agent(*a))).collect();
        let channel = Channel::build(SessionId::new(), alice, Name::new("main").unwrap(), quiet(), Classification::general(Confidentiality::Internal), in_channel);
        let session = Session {
            id: channel.session_id,
            name: "shared".to_string(),
            commands: Default::default(),
            privacy: Default::default(),
            participants: in_session.clone(),
            channels: vec![channel.id],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: caelumora_model::channel::participant_attributes(alice, &in_session),
        };
        self.store.create_session(&Self::system(), &session).await.unwrap();
        self.store.create_channel(&Self::system(), &Channel { security_attributes: caelumora_model::channel::participant_attributes(alice, &channel.participants), ..channel }).await.unwrap();
        self.manager.open_session(&Self::as_user(self.alice), session.id).await.expect("alice opens her session")
    }

    /// The ids of the members of `channel` as `user` sees them.
    async fn members(&self, user: UserId, session: SessionId, channel: ChannelId) -> Vec<ActorId> {
        let view = self.manager.views(Viewer::new(Self::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
        view.members().await.iter().map(|m| m.id()).collect()
    }
}

/// Owned by `owner` - the system if `None` - with `contributors`.
fn attributes(owner: Option<UserId>, contributors: &[UserId]) -> SecurityAttributes {
    SecurityAttributes {
        owner_id: owner.map(ActorId::User).unwrap_or(ActorId::System),
        contributors: contributors.iter().map(|c| ActorId::User(*c)).collect(),
    }
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

    let refused = world.manager.create_from_template(&World::as_user(world.alice), template, None).await.expect_err("not hers to use");
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
        let session = world.manager.create_from_template(&World::as_user(world.alice), template, None).await.expect("hers to use");
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
    let main = session.channels()[0].clone();

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
    let changed = Personality { description: "hers now".to_string(), ..shared.clone() };
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

    let refused = world.manager.create_from_template(&alice, bobs, None).await.expect_err("not hers");
    assert!(not_found(&refused), "{refused:#}");
    for usable in [shared, systems] {
        let session = world.manager.create_from_template(&alice, usable, None).await.expect("hers to use");
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

/// S2: a participant reads and takes part, but changes nothing of the
/// session or its channels.
#[tokio::test]
async fn a_participant_takes_part_but_changes_nothing() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].clone();
    let bob = World::as_user(world.bob);

    world.manager.open_session(&bob, session.id()).await.expect("he opens it");
    assert!(world.members(world.bob, session.id(), main.id()).await.contains(&ActorId::User(world.alice)), "and sees who is in it");
    main.user_actor(world.bob).expect("he takes part").send_message("hello".to_string(), None).await.expect("and writes into it");
    main.cancel(&bob).await.expect("and may cancel what runs in it");

    assert!(denied(&session.update_name(&bob, "his".to_string()).await.expect_err("not his to rename")));
    assert!(session.update_privacy(&bob, PrivacyConfig::default()).await.is_err(), "nor to change its privacy");
    assert!(session.create_channel(&bob, ChannelSpec { name: Name::new("side").unwrap(), components: quiet(), participants: vec![], classification: None }).await.is_err(), "nor to add channels");
    assert!(session.delete_channel(&bob, main.id()).await.is_err(), "nor to delete them");
    assert!(main.remove_participant(&bob, ActorId::User(world.alice)).await.is_err(), "nor to remove others");
    assert!(main.reclassify(&bob, Classification::general(Confidentiality::Public)).await.is_err(), "nor to reclassify the channel");
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

    session.update_name(&alice, "renamed".to_string()).await.expect("hers to rename");
    let side = session.create_channel(&alice, ChannelSpec { name: Name::new("side").unwrap(), components: quiet(), participants: vec![], classification: None }).await.expect("and to add channels");
    side.add_participant(&alice, ActorId::User(world.bob), HistoryAccess::All).await.expect("and people to them");
    session.delete_channel(&alice, side.id()).await.expect("and to delete them");
    assert!(world.manager.delete_session(&alice, session.id()).await.unwrap(), "and to delete it");
    assert!(not_found(&world.manager.open_session(&alice, session.id()).await.expect_err("gone")));
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

// --- External agents ---------------------------------------------------------

/// A remote agent that takes every request on as a task and works on it
/// for ever. Every call reads its credential, as an A2A agent does - in the
/// name it is called in - and is recorded. What it brings in itself is
/// classified as it `declared`.
#[derive(Debug)]
struct RemoteAtWork {
    secrets: Arc<SecretManager>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    declared: Option<Classification>,
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
        Ok(Box::pin(started.chain(futures::stream::pending())))
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
/// `declared`, recording its calls in `calls`.
async fn with_remote_at_work(calls: &Arc<Mutex<Vec<&'static str>>>, declared: Option<Classification>) -> World {
    let calls = calls.clone();
    World::with_kinds(|registry| registry.register(ComponentKind::new("remote_at_work").provides::<dyn ExternalAgent>(), move |_, context| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn ExternalAgent>(Arc::new(RemoteAtWork { secrets: context.secret_manager.clone(), calls: calls.clone(), declared: declared.clone() }));
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

/// An external agent's task acts with the authority of the request that
/// began it, as an agent's turn does (issue 211): asked by bob, the remote
/// agent is sent his request in his name - and followed in his name after
/// a restart, not in the name of the agent's owner.
#[tokio::test]
async fn an_external_agents_task_is_followed_in_the_name_of_whoever_asked() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = session.channels()[0].clone();
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        calls_made(&calls, 1).await;
    }).await;
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["bob as hermes: ReadSecret yes"], "sent in the name of who asked");

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
    assert_eq!(read_secret, ["bob as hermes: ReadSecret yes"], "followed in his name too, whoever opened the session");
    world.manager.stop_session(&World::as_user(world.alice), reopened.id()).await.unwrap();
}

/// An external agent whose answers, as its component declares them, may no
/// longer enter its channel once the channel was lowered is refused with
/// every request (issue 212): asked in the name of who asks, the remote
/// agent is not called, and the users are told.
#[tokio::test]
async fn an_external_agent_whose_answers_no_longer_enter_its_lowered_channel_is_refused() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, Some(Classification::general(Confidentiality::Internal))).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = session.channels()[0].clone();
    main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch().events;

    let (failed, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let caelumora_model::actor::ActorEvent::TurnFailed { message } = envelope.event {
                    break message;
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("may not be published"), "{failed}");
    let publish: Vec<&String> = asked.iter().filter(|line| line.starts_with("bob as hermes: Publish")).collect();
    assert_eq!(publish, ["bob as hermes: Publish no"], "asked in the name of who asked");
    assert!(calls.lock().unwrap().is_empty(), "the remote agent is not called");
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An external agent's task begun before its channel was lowered is given up
/// as a turn is (issue 212): the remote agent is asked to cancel it in the
/// name of who asked for it, and whether what came of it may still be
/// published is asked in his name too.
#[tokio::test]
async fn lowering_a_channel_gives_up_an_external_agents_task_in_the_name_of_whoever_asked() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = session.channels()[0].clone();
    world.manager.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch().events;
    main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;

    let (failed, asked) = world.asked(async {
        main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let caelumora_model::actor::ActorEvent::TurnFailed { message } = envelope.event {
                    break message;
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("lowered"), "{failed}");
    let bobs: Vec<&String> = asked.iter().filter(|line| line.starts_with("bob as hermes")).collect();
    // The lowering cancels the answer, which cancels its remote task - as
    // every cancel does - and only then is asked whether it may go on.
    assert_eq!(bobs, ["bob as hermes: ReadSecret yes", "bob as hermes: Publish no"], "cancelled, and asked whether it goes on, in his name");
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
    let profile = UserProfile::introducing(world.alice, Some("I run the team.")).unwrap();
    world.store.create_user_profile(&alice, &profile).await.expect("she writes her own");
    let forged = UserProfile::introducing(world.alice, Some("I quit.")).unwrap();
    assert!(world.store.update_user_profile(&bob, &forged).await.is_err(), "bob does not write hers");
    let carols = UserProfile::introducing(world.carol, Some("hello")).unwrap();
    assert!(world.store.create_user_profile(&bob, &carols).await.is_err(), "nor anyone else's");

    let mut promoted = world.store.get_user(&alice, world.alice).await.expect("everyone reads a user");
    promoted.roles.push("admin".to_string());
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

    let (made, asked) = world.asked(world.manager.create_from_template(&alice, template, None)).await;
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
    let (again, asked) = world.asked(world.manager.create_from_template(&alice, template, None)).await;
    assert!(again.is_ok());
    assert_eq!(asked.iter().filter(|line| line.contains("CreateAgent")).count(), 0, "{asked:?}");
    assert!(asked.contains(&"alice as helper: GetPersonality(bob's) yes".to_string()), "{asked:?}");
    world.manager.stop_session(&alice, again.unwrap().id()).await.unwrap();

    let private = world.template(Some(world.bob), &[], "private", shared.id).await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&carol, private, None)).await;
    assert!(not_found(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["carol: GetSessionTemplate(bob's) no"]), "a template one may not read: nothing more is asked");

    let unusable = world.template(Some(world.alice), &[], "unusable", bobs.id).await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&alice, unusable, None)).await;
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

/// Deleting a session: reading it and deleting it - its owner's alone;
/// its channels go with it, in the runtime's name.
#[tokio::test]
async fn deleting_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
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
}

/// Creating a channel: the classification it is given, everyone's
/// clearance for it, creating it - as its session's owner - and changing
/// the session that lists it.
#[tokio::test]
async fn creating_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let spec = |participants| ChannelSpec { name: Name::new("side").unwrap(), components: quiet(), participants, classification: None };
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
    let spec = ChannelSpec { name: Name::new("his").unwrap(), components: quiet(), participants: vec![ActorId::User(world.alice)], classification: None };
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

/// Adding an agent to a channel: its personality in its own name - may it
/// run -, its clearance, and changing the channel - its owner's alone.
#[tokio::test]
async fn adding_an_agent_needs_owning_the_channel_and_the_agent_its_personality() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let (helper, other) = (world.agent("helper", world.alice, &shared).await, world.agent("other", world.alice, &shared).await);
    let session = world.shared_session(&[], &[helper, other]).await;
    let main = session.channels()[0].clone();
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
    let main = session.channels()[0].clone();
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
    let main = session.channels()[0].clone();
    let (_, asked) = world.asked(main.remove_participant(&World::as_user(world.bob), ActorId::Agent(helper))).await;
    assert_eq!(asked, checks(&["bob: GetChannel(alice's) yes", "bob: UpdateChannel(alice's) no"]));
    let (removed, asked) = world.asked(main.remove_participant(&World::as_user(world.alice), ActorId::Agent(helper))).await;
    assert!(removed.is_ok());
    assert_eq!(asked, checks(&["alice: GetChannel(alice's) yes", "alice: UpdateChannel(alice's) yes"]));
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}
