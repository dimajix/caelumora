//! What tests of the runtime need to build it as a binary does, and the fixtures the tests of
//! sessions and channels share.

use caelumora_model::confidentiality::{ConfidentialityPolicy, ConfidentialityRules};
use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
use caelumora_model::store::EntityStore;
use caelumora_model::store::memberships::StoreMemberships;
use caelumora_model::store::memory::MemoryStore;
use std::sync::Arc;
use crate::session::{ChannelSpec, SessionLoop, SessionManager};
use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use caelumora_model::channel::Channel;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::DummySecurityManager;
use uuid::Uuid;
use caelumora_model::actor::{ActorEvent, Envelope, Rejection};
use caelumora_model::message::MessageId;
use tokio::sync::broadcast;

/// A store under the ownership policy, which learns who belongs to a
/// group from that store (issue 23), and `rules` as the confidentiality
/// policy beside it (issue 304).
pub(crate) fn owned_store(rules: ConfidentialityRules) -> (Arc<dyn SecurityManager>, Arc<dyn ConfidentialityPolicy>, Arc<MemoryStore>) {
    let memberships = Arc::new(StoreMemberships::new());
    let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    let store = Arc::new(MemoryStore::with_security_manager(Arc::clone(&policy)));
    let entities: Arc<dyn EntityStore> = Arc::<MemoryStore>::clone(&store);
    memberships.bind(&entities);
    (policy, Arc::new(rules), store)
}

/// `users` as participants, recorded in the store the session reads
/// them from (issue 120).
pub(crate) async fn record_users(manager: &SessionManager, users: &[&caelumora_model::user::User]) -> Vec<caelumora_model::session::Participant> {
    let store = manager.runtime_manager.entity_store();
    let mut participants = Vec::new();
    for user in users {
        store.create_user(&SecurityContext::system(), user).await.unwrap();
        participants.push(caelumora_model::session::Participant::User(user.id));
    }
    participants
}

pub(crate) fn test_runtime() -> Arc<RuntimeManager> {
    let store = Arc::new(MemoryStore::new());
    let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
    Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secret_manager, Arc::new(DummySecurityManager::new()), Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap())
}

pub(crate) fn make_session(name: &str) -> Session {
    Session {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: None,
        commands: Default::default(),
        privacy: Default::default(),
        id: SessionId(Uuid::new_v4()),
        name: Name::new(name).unwrap(),
        participants: Vec::new(),
        channels: vec![ChannelId(Uuid::new_v4())],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        security_attributes: SecurityAttributes::default(),
        deleted: None,
    }
}

/// The channel `session` lists, as a document of its own (issue 70):
/// "main", belonging to whom the session belongs to.
pub(crate) fn main_channel(session: &Session) -> Channel {
    Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: session.channels[0],
        session_id: session.id,
        name: Name::new("main").unwrap(),
        components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
        classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        participants: Vec::new(),
        security_attributes: session.security_attributes.clone(),
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
    runtime.entity_store().create_channel(ctx, &main_channel(session)).await.unwrap();
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
    use caelumora_model::actor::ActorId;
    use caelumora_model::user::{User, UserId};
    let user = |name: &str| User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
    let (owner, other) = (user("owner"), user("other"));
    let (owner_id, other_id) = (ActorId::User(owner.id), ActorId::User(other.id));
    let mut session = make_session("with-users");
    session.participants = record_users(manager, &[&owner, &other]).await;
    session.security_attributes = SecurityAttributes::owned_by(owner_id);
    let ctx = SecurityContext::new(owner_id, session.id, Default::default());
    let session_loop = create(manager, &ctx, session).await.expect("the session starts");
    (session_loop, ctx, other_id)
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
