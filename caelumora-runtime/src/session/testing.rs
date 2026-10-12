//! What tests of the runtime need to build it as a binary does, and the fixtures the tests of
//! sessions, channels and the views share: one of each, for building a runtime, a user, a session
//! and its main channel, and the views an actor of a running session navigates (issue 315).

use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::memory::MemoryStore;
use std::sync::Arc;
use crate::session::{ChannelSpec, SessionLoop, SessionManager};
use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::view::ViewsFactory;
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
use caelumora_model::security::DummySecurityManager;
use caelumora_model::actor::{ActorEvent, Envelope, Rejection};
use caelumora_model::message::MessageId;
use tokio::sync::broadcast;
use caelumora_model::config::AppConfig;
use crate::component::ComponentRegistry;

/// A store under the ownership policy, which learns who belongs to a
/// group from that store (issue 23), and `rules` as the confidentiality
/// policy beside it (issue 304).
pub(crate) fn owned_store(rules: ConfidentialityRules) -> (Arc<dyn SecurityManager>, Arc<dyn ConfidentialityPolicy>, Arc<MemoryStore>) {
    let memberships = Arc::new(StoreMemberships::new());
    let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let confidentiality: Arc<dyn ConfidentialityPolicy> = Arc::new(rules);
    let store = Arc::new(MemoryStore::with_policies(Arc::clone(&policy), Arc::clone(&confidentiality)));
    memberships.bind(&store.entity_stores().groups);
    (policy, confidentiality, store)
}

/// A user named `name`, not stored.
pub(crate) fn user(name: &str) -> User {
    User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None }
}

/// `users` as participants, recorded in the store the session reads
/// them from (issue 120).
pub(crate) async fn record_users(manager: &SessionManager, users: &[&User]) -> Vec<Participant> {
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
pub(crate) fn owned_runtime(rules: ConfidentialityRules) -> (Arc<MemoryStore>, Arc<SessionManager>) {
    let (policy, confidentiality, store) = owned_store(rules);
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store.entity_stores(), secrets, policy, confidentiality, &AppConfig::default(), ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
    (store, Arc::new(SessionManager::new(runtime)))
}

/// A runtime under no policy: everything is allowed.
pub(crate) fn test_runtime() -> Arc<RuntimeManager> {
    let store = Arc::new(MemoryStore::new());
    let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
    Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store.entity_stores(), secret_manager, Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default()), &AppConfig::default(), ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap())
}

/// A session that holds nothing but its id `session_id`: what a channel
/// outside a running session belongs to (`ChannelRuntime::outside_a_session`).
pub(crate) fn bare_session(session_id: SessionId) -> Session {
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
pub(crate) fn make_session(name: &str, owner: ActorId) -> Session {
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
pub(crate) fn main_channel(session: &Session) -> Channel {
    let id = session.channels[0];
    Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id,
        session_id: session.id,
        name: Name::new("main").unwrap(),
        components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
        classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        participants: session.participants.clone(),
        security_attributes: SecurityAttributes::owned_by(session.security_attributes.owner_id).granting(Subject::Group(GroupId::of_channel(id)), Grant::Read),
        deleted: None,
        former: Vec::new(),
    }
}

/// Creates and starts `session` with its main channel.
pub(crate) async fn create(manager: &SessionManager, ctx: &SecurityContext, session: Session) -> Result<Arc<SessionLoop>> {
    let channels = vec![main_channel(&session)];
    manager.create_session(ctx, session, channels).await
}

/// Stores `session` and its main channel without starting it.
pub(crate) async fn persist(runtime: &Arc<RuntimeManager>, ctx: &SecurityContext, session: &Session) {
    runtime.entity_store().create_session(ctx, session).await.unwrap();
    runtime.entity_stores().channels.create(ctx, &main_channel(session)).await.unwrap();
}

/// Drain every event currently buffered on the receiver.
pub(crate) fn drain(rx: &mut crate::event::EventSubscription) -> Vec<GlobalEvent> {
    let mut out = Vec::new();
    while let Some(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

/// A running session owned by a user who takes part in it, with that
/// user's context and a second user of the session.
pub(crate) async fn running_session_of_a_user(manager: &SessionManager) -> (Arc<SessionLoop>, SecurityContext, caelumora_model::actor::ActorId) {
    let (owner, other) = (user("owner"), user("other"));
    let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
    let mut session = make_session("with-users", owner_id);
    session.participants = record_users(manager, &[&owner, &other]).await;
    let ctx = SecurityContext::new(owner_id, session.id, Default::default());
    let session_loop = create(manager, &ctx, session).await.expect("the session starts");
    (session_loop, ctx, other_id)
}

/// What `actor` navigates from `channel` of `session`, as its turns do
/// (`ActorRuntime::views`).
pub(crate) fn views_in(session: &SessionLoop, channel: &Arc<ChannelLoop>, actor: ActorId) -> ViewsFactory {
    let channel_runtime = ChannelRuntime::new(Arc::clone(&session.runtime_manager), Arc::clone(&session.commands), Arc::clone(&session.session_controller));
    channel_runtime.attach(channel);
    channel_runtime.new_actor_runtime(actor, session.id(), channel.id()).views()
}

pub(crate) fn spec(name: &str, participants: Vec<caelumora_model::actor::ActorId>) -> ChannelSpec {
    ChannelSpec { name: name.to_string(), components: Default::default(), participants, classification: None, language: None, uses_workspace: None }
}

/// The message and the reason of the next `MessageRejected` `events`
/// show: why the channel did not let in what their user sent (issue 304).
pub(crate) async fn rejection(events: &mut broadcast::Receiver<Envelope>) -> (Option<MessageId>, Rejection) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let ActorEvent::MessageRejected { message, reason, .. } = events.recv().await.expect("the channel is running").event {
                return (message, reason);
            }
        }
    }).await.expect("the channel tells why it did not let it in")
}

pub(crate) async fn history_of(channel: &ChannelLoop, count: usize) -> Vec<caelumora_model::message::Message> {
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
pub(crate) fn runtime_of(manager: &SessionManager) -> &Arc<RuntimeManager> {
    &manager.runtime_manager
}
