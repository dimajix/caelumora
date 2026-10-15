//! What tests of the runtime need to build it as a binary does, and the fixtures the tests of
//! sessions, channels and the views share: one of each, for building a runtime, a user, a session
//! and its main channel, and the views an actor of a running session navigates (issue 315).

use crate::testing::{Script, ScriptedAgents};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityRules};
use caelumora_model::security::{OwnershipPolicy, AccessPolicy};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::memory::MemoryStore;
use std::sync::Arc;
use crate::session::{ChannelSpec, SessionLoop, SessionManager};
use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::runtime::{ActorRuntime, ChannelRuntime, RuntimeManager};
use caelumora_model::actor::ActorId;
use caelumora_model::group::GroupId;
use caelumora_model::session::Participant;
use caelumora_model::user::{User, UserId};
use caelumora_model::security::{Grant, Subject};
use anyhow::Result;
use caelumora_model::channel::Channel;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::AllowAllPolicy;
use caelumora_model::actor::{ActorEvent, Envelope, Rejection};
use caelumora_model::message::MessageId;
use tokio::sync::broadcast;
use caelumora_model::config::AppConfig;
use crate::testing::test_registry;

/// A store under the ownership policy, which learns who belongs to a
/// group from that store (issue 23), and `rules` as the confidentiality
/// policy beside it (issue 304).
pub fn owned_store(rules: ConfidentialityRules) -> (Arc<dyn AccessPolicy>, Arc<dyn ConfidentialityPolicy>, Arc<MemoryStore>) {
    let memberships = Arc::new(StoreMemberships::new());
    let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let confidentiality: Arc<dyn ConfidentialityPolicy> = Arc::new(rules);
    let store = Arc::new(MemoryStore::with_policies(Arc::clone(&access_policy), Arc::clone(&confidentiality)));
    memberships.bind(&store.entity_stores().groups);
    (access_policy, confidentiality, store)
}

/// A user named `name`, not stored.
pub fn user(name: &str) -> User {
    User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None }
}

/// `users` as participants, recorded in the store the session reads
/// them from (issue 120).
pub async fn record_users(manager: &SessionManager, users: &[&User]) -> Vec<Participant> {
    let store = &manager.runtime_manager.entity_stores().users;
    let mut participants = Vec::new();
    for user in users {
        store.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    participants
}

/// A runtime under the ownership policy with `rules` (`owned_store`), and
/// a manager on it.
pub fn owned_runtime(rules: ConfidentialityRules) -> (Arc<MemoryStore>, Arc<SessionManager>) {
    let (policy, confidentiality, store) = owned_store(rules);
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, confidentiality, &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&store)), &[&ScriptedAgents(Script::Answers("ok".to_string()))]).unwrap());
    (store, Arc::new(SessionManager::new(runtime)))
}

/// A runtime under no policy: everything is allowed.
pub fn test_runtime() -> Arc<RuntimeManager> {
    let store = Arc::new(MemoryStore::new());
    let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(AllowAllPolicy::new())));
    Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secret_manager, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()), &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&store)), &[&ScriptedAgents(Script::Answers("ok".to_string()))]).unwrap())
}

/// The runtime of an actor in a channel of no running session, never
/// attached - for turns and what an actor is built with in tests that reach
/// no channel: its views read nothing elsewhere, its questions have nobody
/// to be put to, and it has no workspace.
pub fn detached_actor_runtime() -> ActorRuntime {
    let runtime = test_runtime();
    let commands = Arc::new(CommandExecutor::empty(Arc::clone(runtime.access_policy())));
    let session_id = SessionId::new();
    ChannelRuntime::outside_a_session(runtime, commands, session_id).new_actor_runtime(ActorId::None, session_id, ChannelId::new())
}

/// A session that holds nothing but its id `session_id`: what a channel
/// outside a running session belongs to (`ChannelRuntime::outside_a_session`).
pub fn bare_session(session_id: SessionId) -> Session {
    let now = chrono::Utc::now();
    Session {
        summary: None, briefing: None,
        id: session_id,
        name: Name::new("s").unwrap(),
        participants: Vec::new(),
        channels: Vec::new(),
        commands: Default::default(),
        privacy: Default::default(),
        approval_rules: Vec::new(),
        workspace: None,
        created_at: now,
        updated_at: now,
        security_attributes: Default::default(),
        deleted: None,
    }
}

/// A session of `owner` named `name`, as a template builds one
/// (`Template::build`): read by its members, the group of its participants
/// (issue 23), whom the test sets. It lists one channel, `main_channel`.
pub fn make_session(name: &str, owner: ActorId) -> Session {
    let id = SessionId::new();
    let now = chrono::Utc::now();
    Session {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: None,
        commands: Default::default(),
        privacy: Default::default(),
        id,
        name: Name::new(name).unwrap(),
        participants: Vec::new(),
        channels: vec![ChannelId::new()],
        created_at: now,
        updated_at: now,
        security_attributes: SecurityAttributes::owned_by(owner).granting(Subject::Group(GroupId::of_session(id)), Grant::Read),
        deleted: None,
    }
}

/// The channel `session` lists, as a document of its own (issue 70) and as
/// `Channel::build` makes one: "main", belonging to whom the session belongs
/// to, read by its members - the session's participants.
pub fn main_channel(session: &Session) -> Channel {
    let id = session.channels[0];
    Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id,
        session_id: session.id,
        name: Name::new("main").unwrap(),
        components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        participants: session.participants.clone(),
        security_attributes: SecurityAttributes::owned_by(session.security_attributes.owner_id).granting(Subject::Group(GroupId::of_channel(id)), Grant::Read),
        deleted: None,
        former: Vec::new(),
    }
}

/// The channel `id` of the running `session`, classified as
/// `classification`: the one it runs, or else one it is made to run as its
/// main channel is, with that id and classification - for tests whose
/// agents work in channels of their own.
pub async fn running_channel(session: &Arc<SessionLoop>, id: ChannelId, classification: Classification) -> Arc<ChannelLoop> {
    if let Some(running) = session.channel(id) {
        return running;
    }
    let channel = Channel { id, classification, ..main_channel(&session.session()) };
    session.add_channel_loop(&channel).await.unwrap()
}

/// Creates and starts `session` with its main channel.
pub async fn create(manager: &SessionManager, ctx: &SecurityContext, session: Session) -> Result<Arc<SessionLoop>> {
    let channels = vec![main_channel(&session)];
    manager.create(ctx, session, channels).await
}

/// Stores `session` and its main channel without starting it.
pub async fn persist(runtime: &Arc<RuntimeManager>, ctx: &SecurityContext, session: &Session) {
    runtime.entity_stores().sessions.create(ctx, session).await.unwrap();
    runtime.entity_stores().channels.create(ctx, &main_channel(session)).await.unwrap();
}

/// Drain every event currently buffered on the receiver.
pub fn drain(rx: &mut crate::event::EventSubscription) -> Vec<GlobalEvent> {
    let mut out = Vec::new();
    while let Some(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// A running session owned by a user who takes part in it, with that
/// user's context and a second user of the session.
pub async fn running_session_of_a_user(manager: &SessionManager) -> (Arc<SessionLoop>, SecurityContext, caelumora_model::actor::ActorId) {
    let (owner, other) = (user("owner"), user("other"));
    let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
    let mut session = make_session("with-users", owner_id);
    session.participants = record_users(manager, &[&owner, &other]).await;
    let ctx = SecurityContext::new(owner_id, session.id, Default::default());
    let session_loop = create(manager, &ctx, session).await.expect("the session starts");
    (session_loop, ctx, other_id)
}

pub use crate::testing::actor_runtime_in;

pub fn spec(name: &str, participants: Vec<caelumora_model::actor::ActorId>) -> ChannelSpec {
    ChannelSpec { name: name.to_string(), components: Default::default(), participants, classification: None, language: None, uses_workspace: None }
}

/// The message and the reason of the next `MessageRejected` `events`
/// show: why the channel did not let in what their user sent (issue 304).
pub async fn rejection(events: &mut broadcast::Receiver<Envelope>) -> (Option<MessageId>, Rejection) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let ActorEvent::MessageRejected { message, reason, .. } = events.recv().await.expect("the channel is running").event {
                return (message, reason);
            }
        }
    }).await.expect("the channel tells why it did not let it in")
}

pub async fn history_of(channel: &ChannelLoop, count: usize) -> Vec<caelumora_model::message::Message> {
    for _ in 0..100 {
        let history = channel.history().await;
        if history.len() >= count {
            return history;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the channel's history never reached {count} messages");
}

/// The runtime `manager` works with, for tests that ask it directly.
pub fn runtime_of(manager: &SessionManager) -> &Arc<RuntimeManager> {
    &manager.runtime_manager
}
