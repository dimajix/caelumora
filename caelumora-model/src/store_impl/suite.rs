//! The behaviour every store implementation owes (issue 93): one suite that
//! the `MemoryStore` and the `JsonFileStore` both run, so that the two can't
//! drift apart again. What is specific to one store - files on disk,
//! concurrent writers - stays in that store's own tests.

use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId, TurnState};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::confidentiality::Confidentiality;
use crate::meta::{Metadata, Scope};
use crate::security::SecurityContext;
use crate::session::{Session, SessionId};
use crate::store::{EntityStore, StateStore, StoreError};
use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId};
use chrono::Utc;
use uuid::Uuid;

/// Runs one kind through its whole life: create, create again, update, get,
/// list, delete, delete again, and get and update once it is gone.
/// `$field` is a text field of the entity that the update changes.
macro_rules! lifecycle {
    ($store:expr, $value:expr, $key:expr, $($field:ident).+, $entity:literal,
     $create:ident, $get:ident, $update:ident, $delete:ident, $list:ident) => {{
        let store = $store;
        let ctx = SecurityContext::default();
        let mut value = $value;

        store.$create(&ctx, &value).await.expect(concat!($entity, ": create"));
        let conflict = store.$create(&ctx, &value).await;
        assert!(matches!(conflict, Err(StoreError::Conflict(_))), concat!($entity, ": creating twice is a conflict, got {:?}"), conflict);

        value.$($field).+ = "renamed".to_string();
        store.$update(&ctx, &value).await.expect(concat!($entity, ": update"));
        let found = store.$get(&ctx, ($key)(&value)).await.expect(concat!($entity, ": get"));
        assert_eq!(found.$($field).+, "renamed", concat!($entity, ": get sees the update"));

        let listed = store.$list(&ctx).await.expect(concat!($entity, ": list"));
        assert_eq!(listed.len(), 1, concat!($entity, ": list holds exactly what was created"));
        assert!(listed[0].id == value.id, concat!($entity, ": list holds the entity"));

        assert!(store.$delete(&ctx, ($key)(&value)).await.expect(concat!($entity, ": delete")), concat!($entity, ": delete reports the removal"));
        assert!(!store.$delete(&ctx, ($key)(&value)).await.expect(concat!($entity, ": delete again")), concat!($entity, ": deleting again removes nothing"));

        let gone = store.$get(&ctx, ($key)(&value)).await;
        assert!(matches!(gone, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": get after delete is NotFound, got {:?}"), gone);
        let gone = store.$update(&ctx, &value).await;
        assert!(matches!(gone, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": update after delete is NotFound, got {:?}"), gone);
        assert!(store.$list(&ctx).await.expect(concat!($entity, ": list")).is_empty(), concat!($entity, ": list is empty after delete"));
    }};
}

fn metadata(name: &str) -> Metadata {
    Metadata {
        name: name.to_string(),
        scope: Scope::GlobalScope,
        ..Metadata::default()
    }
}

fn personality() -> Personality {
    Personality {
        reasoning_confidentiality: Confidentiality::Public,
        security_attributes: Default::default(),
        id: PersonalityId(Uuid::new_v4()),
        metadata: metadata("assistant"),
        components: Default::default(),
    }
}

fn session_template() -> SessionTemplate {
    SessionTemplate {
        security_attributes: Default::default(),
        commands: Default::default(),
        id: SessionTemplateId(Uuid::new_v4()),
        metadata: metadata("pairing"),
        scope: ScopeTemplate::GlobalScope,
        participants: vec![ParticipantTemplate::User { name: "user".to_string() }],
        channels: vec![ChannelTemplate {
            name: "general".to_string(),
            moderator: "broadcast".to_string(),
            confidentiality: Confidentiality::CHANNEL_DEFAULT,
            participants: vec!["user".to_string()],
        }],
    }
}

fn session() -> Session {
    Session {
        commands: Default::default(),
        id: SessionId(Uuid::new_v4()),
        metadata: metadata("session"),
        participants: Vec::new(),
        channels: Vec::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        security_attributes: Default::default(),
    }
}

fn channel() -> Channel {
    Channel {
        id: ChannelId(Uuid::new_v4()),
        session_id: SessionId(Uuid::new_v4()),
        name: "general".to_string(),
        moderator: "broadcast".to_string(),
        confidentiality: Confidentiality::CHANNEL_DEFAULT,
        history_limits: Vec::new(),
        participants: Vec::new(),
        security_attributes: Default::default(),
    }
}

fn user(username: &str) -> User {
    User {
        id: UserId::new(),
        username: username.to_string(),
        email: String::new(),
        roles: vec!["user".to_string()],
    }
}

fn component() -> ComponentDescriptor {
    ComponentDescriptor {
        id: "search".to_string(),
        kind: "web_search".to_string(),
        params: Default::default(),
        tags: Vec::new(),
        fail_policy: Default::default(),
        security_attributes: Default::default(),
    }
}

// A named function rather than a closure: a closure returning a borrow of
// its argument doesn't get the lifetime inferred.
fn component_id(value: &ComponentDescriptor) -> &str {
    &value.id
}

/// Every kind of entity goes through the same life, and users are also
/// found by name.
pub(crate) async fn entity_store_behaves(store: &dyn EntityStore) {
    lifecycle!(store, personality(), |v: &Personality| v.id, metadata.name, "personality",
        create_personality, get_personality, update_personality, delete_personality, list_personalities);
    lifecycle!(store, session_template(), |v: &SessionTemplate| v.id, metadata.name, "session_template",
        create_session_template, get_session_template, update_session_template, delete_session_template, list_session_templates);
    lifecycle!(store, Agent::new("assistant-1".to_string(), Scope::GlobalScope, &personality(), crate::actor::ActorId::None), |v: &Agent| v.id, metadata.name, "agent",
        create_agent, get_agent, update_agent, delete_agent, list_agents);
    lifecycle!(store, session(), |v: &Session| v.id, metadata.name, "session",
        create_session, get_session, update_session, delete_session, list_sessions);
    lifecycle!(store, channel(), |v: &Channel| v.id, name, "channel",
        create_channel, get_channel, update_channel, delete_channel, list_channels);
    lifecycle!(store, user("alice"), |v: &User| v.id, username, "user",
        create_user, get_user, update_user, delete_user, list_users);
    lifecycle!(store, component(), component_id, kind, "component",
        create_component, get_component, update_component, delete_component, list_components);

    let ctx = SecurityContext::default();
    let alice = user("alice");
    store.create_user(&ctx, &alice).await.expect("create user");
    store.create_user(&ctx, &user("bob")).await.expect("create user");
    let found = store.find_user_by_name(&ctx, "alice").await.expect("alice is found by name");
    assert_eq!(found.id, alice.id);
    let missing = store.find_user_by_name(&ctx, "carol").await;
    assert!(matches!(missing, Err(StoreError::NotFound { entity: "user", .. })), "an unknown name is NotFound, got {missing:?}");
}

/// Agent states that were never stored read as their default, a channel
/// state as `NotFound`; deleting a channel's state takes its agents' states
/// in the channel along, and nothing else.
pub(crate) async fn state_store_behaves(store: &dyn StateStore) {
    let (agent, other_agent) = (AgentId::new(), AgentId::new());
    let (channel, other_channel) = (ChannelId::new(), ChannelId::new());

    store.get_agent_state(agent).await.expect("a missing agent state is no error");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("a missing agent channel state is no error").turn.is_none());
    let missing = store.get_channel_state(channel).await;
    assert!(matches!(missing, Err(StoreError::NotFound { entity: "channel_state", .. })), "a missing channel state is NotFound, got {missing:?}");

    let in_turn = AgentChannelState { turn: Some(TurnState::default()), ..AgentChannelState::default() };
    store.put_agent_channel_state(agent, channel, &in_turn).await.expect("put agent channel state");
    store.put_agent_channel_state(other_agent, channel, &in_turn).await.expect("put agent channel state");
    store.put_agent_channel_state(agent, other_channel, &in_turn).await.expect("put agent channel state");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("get agent channel state").turn.is_some(), "the stored state comes back");

    store.put_agent_state(agent, &AgentState::default()).await.expect("put agent state");
    store.get_agent_state(agent).await.expect("get agent state");

    store.put_channel_state(channel, &ChannelState::new(channel)).await.expect("put channel state");
    store.put_channel_state(other_channel, &ChannelState::new(other_channel)).await.expect("put channel state");
    assert_eq!(store.get_channel_state(channel).await.expect("get channel state").channel_id, channel);

    store.delete_channel_state(channel).await.expect("delete channel state");
    assert!(matches!(store.get_channel_state(channel).await, Err(StoreError::NotFound { .. })), "the channel state is gone");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("get").turn.is_none(), "the agents' states in the channel are gone");
    assert!(store.get_agent_channel_state(other_agent, channel).await.expect("get").turn.is_none(), "the agents' states in the channel are gone");
    assert!(store.get_agent_channel_state(agent, other_channel).await.expect("get").turn.is_some(), "other channels keep theirs");
    store.get_channel_state(other_channel).await.expect("other channels keep theirs");
    store.delete_channel_state(channel).await.expect("nothing left to delete is no error");
}
