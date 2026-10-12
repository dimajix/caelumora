//! The behaviour every store implementation owes (issue 93): one suite that
//! the `MemoryStore` and the `JsonFileStore` both run, so that the two can't
//! drift apart again. What is specific to one store - files on disk,
//! concurrent writers - stays in that store's own tests.

use crate::store::query::{Filter, Query};
use crate::actor::ActorId;
use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::turn::TurnState;
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::{ComponentConfig, ComponentDescriptor, StoredComponents};
use crate::confidentiality::{Classification, Visibility};
use crate::external_agent::ConversationId;
use crate::value::name::{Name, ScopedName};
use crate::value::text::Introduction;
use crate::scope::{Scoped, Scope};
use crate::security::SecurityContext;
use crate::session::{Session, SessionId};
use crate::store::{ComponentStore, EntityStores, Journaled, StateStore, StoreError};
use crate::template::{ChannelTemplate, ParticipantTemplate, SessionTemplate, SessionTemplateId};
use crate::removal::{Deletable, Removal};
use crate::user::{User, UserId, UserProfile};
use crate::workspace::{Workspace, WorkspaceId};
use crate::skill::{Skill, SkillId};
use crate::group::{Group, GroupId};
use crate::artifact::{Artifact, ArtifactField, ArtifactId};
use chrono::Utc;
use uuid::Uuid;

/// Runs one kind through its whole life: create, create again, update, get,
/// list, delete, delete again, and get and update once it is gone.
/// `$field` is a text field of the entity that the update changes - to
/// "renamed", or to `$new` where given - `$key` what the entity is stored
/// under.
macro_rules! lifecycle {
    ($store:expr, $value:expr, $key:expr, $($field:ident).+, $entity:literal,
     $create:ident, $get:ident, $update:ident, $delete:ident, $list:ident) => {
        lifecycle!($store, $value, $key, $($field).+ = "renamed".to_string().try_into().expect(concat!($entity, ": a valid value")), $entity,
            $create, $get, $update, $delete, $list)
    };
    ($store:expr, $value:expr, $key:expr, $($field:ident).+ = $new:expr, $entity:literal,
     $create:ident, $get:ident, $update:ident, $delete:ident, $list:ident) => {{
        let store = $store;
        let ctx = SecurityContext::default();
        let mut value = $value;

        store.$create(&ctx, &value).await.expect(concat!($entity, ": create"));
        let conflict = store.$create(&ctx, &value).await;
        assert!(matches!(conflict, Err(StoreError::Conflict(_))), concat!($entity, ": creating twice is a conflict, got {:?}"), conflict);

        value.$($field).+ = $new;
        store.$update(&ctx, &value).await.expect(concat!($entity, ": update"));
        let found = store.$get(&ctx, ($key)(&value)).await.expect(concat!($entity, ": get"));
        assert_eq!(found.$($field).+, value.$($field).+, concat!($entity, ": get sees the update"));

        let listed = store.$list(&ctx, &Query::all()).await.map(|page| page.items).expect(concat!($entity, ": list"));
        assert_eq!(listed.len(), 1, concat!($entity, ": list holds exactly what was created"));
        assert!(($key)(&listed[0]) == ($key)(&value), concat!($entity, ": list holds the entity"));

        assert!(store.$delete(&ctx, ($key)(&value)).await.expect(concat!($entity, ": delete")), concat!($entity, ": delete reports the removal"));
        assert!(!store.$delete(&ctx, ($key)(&value)).await.expect(concat!($entity, ": delete again")), concat!($entity, ": deleting again removes nothing"));

        let gone = store.$get(&ctx, ($key)(&value)).await;
        assert!(matches!(gone, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": get after delete is NotFound, got {:?}"), gone);
        let gone = store.$update(&ctx, &value).await;
        assert!(matches!(gone, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": update after delete is NotFound, got {:?}"), gone);
        assert!(store.$list(&ctx, &Query::all()).await.map(|page| page.items).expect(concat!($entity, ": list")).is_empty(), concat!($entity, ": list is empty after delete"));
    }};
}

fn scoped(name: &str) -> ScopedName {
    ScopedName::new(name, Scope::GlobalScope).unwrap()
}

fn personality() -> Personality {
    Personality {
        introduction: None,
        reasoning_visibility: Visibility::Channel,
        security_attributes: Default::default(),
        id: PersonalityId(Uuid::new_v4()),
        name: scoped("assistant"),
        kind: Default::default(),
        roles: None,
        description: None,
        components: Default::default(),
        deleted: None,
    }
}

fn session_template() -> SessionTemplate {
    SessionTemplate {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: None,
        security_attributes: Default::default(),
        commands: Default::default(),
        privacy: Default::default(),
        id: SessionTemplateId(Uuid::new_v4()),
        name: scoped("pairing"),
        description: None,
        participants: vec![ParticipantTemplate::User { name: "user".to_string() }],
        channels: vec![ChannelTemplate {
            summary: None, briefing: None, language: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            name: Name::new("general").unwrap(),
            components: crate::channel::moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            participants: vec!["user".to_string()],
        }],
        deleted: None,
    }
}

fn session() -> Session {
    Session {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: None,
        commands: Default::default(),
        privacy: Default::default(),
        id: SessionId(Uuid::new_v4()),
        name: Name::new("session").unwrap(),
        participants: Vec::new(),
        channels: Vec::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        security_attributes: Default::default(),
        deleted: None,
    }
}

fn channel() -> Channel {
    Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId(Uuid::new_v4()),
        session_id: SessionId(Uuid::new_v4()),
        name: Name::new("general").unwrap(),
        components: crate::channel::moderation_components(None, Some("broadcast_policy")),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        former: Vec::new(),
        participants: Vec::new(),
        security_attributes: Default::default(),
        deleted: None,
    }
}

fn user(username: &str) -> User {
    User {
        id: UserId::new(),
        username: Name::new(username).unwrap(),
        email: String::new(),
        roles: vec![crate::role::RoleId::new()],
        deleted: None,
    }
}

fn workspace(owner: UserId) -> Workspace {
    workspace_named(ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap())
}

fn skill(owner: UserId) -> Skill {
    skill_named(ScopedName::new("release-notes", Scope::UserScope { user_id: owner }).unwrap())
}

/// A skill of `name`, owned by the user whose scope it lies in, else by
/// another.
fn skill_named(name: ScopedName) -> Skill {
    let owner = match name.scope {
        Scope::UserScope { user_id } => user_id,
        _ => UserId::new(),
    };
    Skill::new(name, crate::value::text::Description::new("How we write a release note").unwrap(), crate::value::text::SkillBody::new("Collect, group, write.").unwrap(), Classification::CHANNEL_DEFAULT, crate::actor::ActorId::User(owner))
}

fn group(owner: UserId) -> Group {
    group_named(ScopedName::new("project", Scope::UserScope { user_id: owner }).unwrap())
}

/// A team of `name`, owned by the user whose scope it lies in, else by
/// another.
fn group_named(name: ScopedName) -> Group {
    let owner = match name.scope {
        Scope::UserScope { user_id } => user_id,
        _ => UserId::new(),
    };
    Group::team(name, owner)
}

/// A workspace of `name`, owned by the user whose scope it lies in, else by
/// another.
fn workspace_named(name: ScopedName) -> Workspace {
    let owner = match name.scope {
        Scope::UserScope { user_id } => user_id,
        _ => UserId::new(),
    };
    Workspace::new(name, "main", Classification::CHANNEL_DEFAULT, owner)
}

fn artifact(owner: UserId) -> Artifact {
    artifact_named(ScopedName::new("report.csv", Scope::UserScope { user_id: owner }).unwrap())
}

/// An artifact of `name`, owned by the user whose scope it lies in, else by
/// another.
fn artifact_named(name: ScopedName) -> Artifact {
    let owner = match name.scope {
        Scope::UserScope { user_id } => user_id,
        _ => UserId::new(),
    };
    Artifact::new(ArtifactId::new(), name, "files", Classification::CHANNEL_DEFAULT, owner, crate::artifact::tests::version(b"a,b", SessionId::new()))
}

fn component() -> StoredComponents {
    StoredComponents::new("search", ComponentConfig { component_nodes: vec![ComponentDescriptor::new("search", "web_search")] }, Default::default())
}

/// A store answers a query of a list (issue 293): those that meet its
/// filter - fields of the entity, of an element of a collection in it, of
/// an element of a collection in that -, in the order of their ids, in pages
/// of its limit, each going on after the cursor the one before handed out.
pub(crate) async fn queries_behave(stores: &EntityStores) {
    let store = stores.entities.as_ref();
    use crate::security::{AclEntry, Grant, Subject};
    let ctx = SecurityContext::system();
    let (alice, bob) = (UserId::new(), UserId::new());
    let (session, channel) = (SessionId::new(), ChannelId::new());
    let bobs = Subject::Actor(ActorId::User(bob));
    let mut made = Vec::new();
    for (index, owner) in [alice, alice, bob, alice, bob].into_iter().enumerate() {
        let mut artifact = artifact_named(ScopedName::new(format!("a{index}.csv"), Scope::UserScope { user_id: owner }).unwrap());
        match index {
            // Published in the channel asked about.
            1 => (artifact.versions[0].session_id, artifact.versions[0].channel_id) = (session, channel),
            // Shared with bob as a whole.
            3 => artifact.security_attributes.acl.push(AclEntry { subject: bobs, grant: Grant::Read }),
            // A version of it shared with bob, published elsewhere in the session.
            4 => {
                artifact.versions[0].session_id = session;
                artifact.versions[0].acl.push(AclEntry { subject: bobs, grant: Grant::Read });
            }
            _ => {}
        }
        stores.artifacts.create(&ctx, &artifact).await.expect("an artifact");
        made.push(artifact.id);
    }
    let ids = |page: &[Artifact]| page.iter().map(|a| a.id).collect::<Vec<_>>();
    let mut sorted = made.clone();
    sorted.sort_by_key(|id| id.to_string());
    let listed = stores.artifacts.list(&ctx, &Query::all()).await.expect("all of them");
    assert_eq!(ids(&listed.items), sorted, "in the order of their ids");
    assert!(listed.next.is_none());

    let owned = Filter::eq(ArtifactField::Owner, ActorId::User(bob));
    let found = stores.artifacts.list(&ctx, &Query::filtered(owned)).await.unwrap();
    let mut bobs_own = vec![made[2], made[4]];
    bobs_own.sort_by_key(|id| id.to_string());
    assert_eq!(ids(&found.items), bobs_own, "a field of the entity");

    let place = Filter::And(vec![Filter::eq(ArtifactField::VersionSession, session), Filter::eq(ArtifactField::VersionChannel, channel)]);
    let published = stores.artifacts.list(&ctx, &Query::filtered(Filter::any(ArtifactField::Versions, place))).await.unwrap();
    assert_eq!(ids(&published.items), [made[1]], "a version published in the channel, not one elsewhere in the session");

    let named = Filter::eq(ArtifactField::Subject, bobs);
    let shared = Filter::Or(vec![
        Filter::any(ArtifactField::Acl, named.clone()),
        Filter::any(ArtifactField::Versions, Filter::any(ArtifactField::VersionAcl, named)),
    ]);
    let mut with_bob = vec![made[3], made[4]];
    with_bob.sort_by_key(|id| id.to_string());
    assert_eq!(ids(&stores.artifacts.list(&ctx, &Query::filtered(shared)).await.unwrap().items), with_bob, "an entry of its list or of a version's");

    let mut paged = Vec::new();
    let mut query = Query::all().limited(2);
    loop {
        let page = stores.artifacts.list(&ctx, &query).await.unwrap();
        assert!(page.items.len() <= 2, "at most its limit");
        paged.extend(ids(&page.items));
        match page.next {
            Some(cursor) => query = Query::all().limited(2).after(cursor),
            None => break,
        }
    }
    assert_eq!(paged, sorted, "the pages, one after the other, are the whole list");

    // By scope, by the session of a channel, by a participant.
    use crate::channel::{ChannelField, Participant};
    use crate::scope::ScopeField;
    use crate::session::SessionField;
    let session_id = SessionId(Uuid::new_v4());
    let in_session = Personality { name: ScopedName::new("in-session", Scope::SessionScope { session_id }).unwrap(), ..personality() };
    let in_channel = Personality { name: ScopedName::new("in-channel", Scope::ChannelScope { session_id, channel_id: ChannelId(Uuid::new_v4()) }).unwrap(), ..personality() };
    let elsewhere = Personality { name: scoped("elsewhere"), ..personality() };
    for personality in [&in_session, &in_channel, &elsewhere] {
        store.create_personality(&ctx, personality).await.unwrap();
    }
    let holder = Scope::SessionScope { session_id };
    let lying = store.list_personalities(&ctx, &Query { filter: holder.within::<ScopeField>(), ..Query::all() }).await.unwrap();
    let mut expected = vec![in_session.id, in_channel.id];
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(lying.items.iter().map(|p| p.id).collect::<Vec<_>>(), expected, "what lies in the session or a channel of it");

    let agent = AgentId(Uuid::new_v4());
    let mut with_agent = self::session();
    with_agent.participants.push(Participant::Agent(agent));
    let without = self::session();
    store.create_session(&ctx, &with_agent).await.unwrap();
    store.create_session(&ctx, &without).await.unwrap();
    let taking_part = Filter::any(SessionField::Participants, Filter::eq(SessionField::Participant, ActorId::Agent(agent)));
    let found = store.list_sessions(&ctx, &Query::filtered(taking_part)).await.unwrap();
    assert_eq!(found.items.iter().map(|s| s.id).collect::<Vec<_>>(), [with_agent.id], "a participant, the whole element");

    let of_it = Channel { session_id: with_agent.id, ..self::channel() };
    store.create_channel(&ctx, &of_it).await.unwrap();
    store.create_channel(&ctx, &Channel { session_id: without.id, ..self::channel() }).await.unwrap();
    let found = store.list_channels(&ctx, &Query::filtered(Filter::eq(ChannelField::Session, with_agent.id))).await.unwrap();
    assert_eq!(found.items.iter().map(|c| c.id).collect::<Vec<_>>(), [of_it.id], "the channels of a session");

    // By a member (issue 308).
    use crate::group::GroupField;
    let mut his = group(alice);
    his.members.push(ActorId::User(bob));
    stores.groups.create(&ctx, &his).await.unwrap();
    stores.groups.create(&ctx, &group(UserId::new())).await.unwrap();
    let belonging = Filter::any(GroupField::Members, Filter::eq(GroupField::Member, ActorId::User(bob)));
    let found = stores.groups.list(&ctx, &Query::filtered(belonging)).await.unwrap();
    assert_eq!(found.items.iter().map(|g| g.id).collect::<Vec<_>>(), [his.id], "the groups of a member, not those of their owner");
}

/// Preconfigured components go through the same life as the entities.
pub(crate) async fn component_store_behaves(store: &dyn ComponentStore) {
    lifecycle!(store, component(), component_id,
        components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("search", "renamed")] }, "components",
        create_components, get_components, update_components, delete_components, list_components);
}

/// A definition of a kind is found by its name in exactly its scope (issue
/// 178): compared as names are, not in another scope, not once deleted.
/// `$make` makes one of a `ScopedName`.
macro_rules! found_by_name {
    ($store:expr, $make:expr, $entity:literal, $create:ident, $delete:ident, $find:ident) => {{
        let store = $store;
        let ctx = SecurityContext::default();
        let user_id = UserId::new();
        let global = ($make)(ScopedName::new("Helper", Scope::GlobalScope).unwrap());
        let users = ($make)(ScopedName::new("helper", Scope::UserScope { user_id }).unwrap());
        store.$create(&ctx, &global).await.expect(concat!($entity, ": create"));
        store.$create(&ctx, &users).await.expect(concat!($entity, ": create"));

        let found = store.$find(&ctx, &ScopedName::new("helper", Scope::GlobalScope).unwrap()).await.expect(concat!($entity, ": found by name"));
        assert_eq!(found.id, global.id, concat!($entity, ": the one in that scope, compared as names are"));
        let found = store.$find(&ctx, &ScopedName::new("HELPER", Scope::UserScope { user_id }).unwrap()).await.expect(concat!($entity, ": found by name"));
        assert_eq!(found.id, users.id, concat!($entity, ": the one in the user's scope"));
        let elsewhere = store.$find(&ctx, &ScopedName::new("helper", Scope::SessionScope { session_id: SessionId::new() }).unwrap()).await;
        assert!(matches!(elsewhere, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": not in a scope it does not lie in, got {:?}"), elsewhere);

        store.$delete(&ctx, global.id).await.expect(concat!($entity, ": delete"));
        let deleted = store.$find(&ctx, &global.scoped_name()).await;
        assert!(matches!(deleted, Err(StoreError::NotFound { entity: $entity, .. })), concat!($entity, ": not once deleted, got {:?}"), deleted);
    }};
}

// A named function rather than a closure: a closure returning a borrow of
// its argument doesn't get the lifetime inferred.
fn component_id(value: &StoredComponents) -> &str {
    value.id()
}

/// Every kind of entity goes through the same life; users are also found
/// by name, and so are definitions in their scope.
pub(crate) async fn entity_store_behaves(stores: &EntityStores) {
    let store = stores.entities.as_ref();
    lifecycle!(store, personality(), |v: &Personality| v.id, name.name, "personality",
        create_personality, get_personality, update_personality, delete_personality, list_personalities);
    lifecycle!(store, session_template(), |v: &SessionTemplate| v.id, name.name, "session_template",
        create_session_template, get_session_template, update_session_template, delete_session_template, list_session_templates);
    lifecycle!(store, Agent::new(scoped("assistant-1"), &personality(), crate::actor::ActorId::None), |v: &Agent| v.id, name.name, "agent",
        create_agent, get_agent, update_agent, delete_agent, list_agents);
    lifecycle!(store, session(), |v: &Session| v.id, name, "session",
        create_session, get_session, update_session, delete_session, list_sessions);
    lifecycle!(store, channel(), |v: &Channel| v.id, name, "channel",
        create_channel, get_channel, update_channel, delete_channel, list_channels);
    lifecycle!(store, user("alice"), |v: &User| v.id, username, "user",
        create_user, get_user, update_user, delete_user, list_users);
    lifecycle!(store, UserProfile::new(UserId::new(), None), |v: &UserProfile| v.user_id(), introduction = Some(Introduction::new("renamed").unwrap()), "user_profile",
        create_user_profile, get_user_profile, update_user_profile, delete_user_profile, list_user_profiles);
    lifecycle!(stores.workspaces.as_ref(), workspace(UserId::new()), |v: &Workspace| v.id, name.name, "workspace",
        create, get, update, delete, list);
    lifecycle!(stores.skills.as_ref(), skill(UserId::new()), |v: &Skill| v.id, name.name, "skill",
        create, get, update, delete, list);
    lifecycle!(stores.artifacts.as_ref(), artifact(UserId::new()), |v: &Artifact| v.id, name.name, "artifact",
        create, get, update, delete, list);
    lifecycle!(stores.groups.as_ref(), group(UserId::new()), |v: &Group| v.id, name.name, "group",
        create, get, update, delete, list);

    let ctx = SecurityContext::default();
    let alice = user("alice");
    store.create_user(&ctx, &alice).await.expect("create user");
    store.create_user(&ctx, &user("bob")).await.expect("create user");
    let found = store.find_user_by_name(&ctx, "alice").await.expect("alice is found by name");
    assert_eq!(found.id, alice.id);
    let missing = store.find_user_by_name(&ctx, "carol").await;
    assert!(matches!(missing, Err(StoreError::NotFound { entity: "user", .. })), "an unknown name is NotFound, got {missing:?}");

    // A template that would make sessions it can never lie in (issue 178).
    let ctx = SecurityContext::default();
    let in_session = SessionTemplate { name: ScopedName::new("nested", Scope::SessionScope { session_id: SessionId::new() }).unwrap(), ..session_template() };
    let refused = store.create_session_template(&ctx, &in_session).await;
    assert!(matches!(refused, Err(StoreError::Validation(_))), "a template of a session's scope is not stored, got {refused:?}");
    let template = session_template();
    store.create_session_template(&ctx, &template).await.expect("a global one is");
    let moved = SessionTemplate { name: ScopedName::new("nested", Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }).unwrap(), ..template };
    let refused = store.update_session_template(&ctx, &moved).await;
    assert!(matches!(refused, Err(StoreError::Validation(_))), "nor moved into a channel's, got {refused:?}");

    found_by_name!(store, |name| Personality { name, id: PersonalityId::new(), ..personality() }, "personality",
        create_personality, delete_personality, find_personality_by_name);
    found_by_name!(store, |name| SessionTemplate { name, id: SessionTemplateId::new(), ..session_template() }, "session_template",
        create_session_template, delete_session_template, find_session_template_by_name);
    found_by_name!(store, |name| Agent::new(name, &personality(), crate::actor::ActorId::None), "agent",
        create_agent, delete_agent, find_agent_by_name);

    found_by_name!(stores.workspaces.as_ref(), workspace_named, "workspace",
        create, delete, find_by_name);
    let nobodys = stores.workspaces.create(&ctx, &Workspace { security_attributes: Default::default(), ..workspace(UserId::new()) }).await;
    assert!(matches!(nobodys, Err(StoreError::Validation(_))), "workspace: one that belongs to no user is not stored, got {nobodys:?}");

    found_by_name!(stores.skills.as_ref(), skill_named, "skill",
        create, delete, find_by_name);
    let nobodys = stores.skills.create(&ctx, &Skill { security_attributes: Default::default(), ..skill(UserId::new()) }).await;
    assert!(matches!(nobodys, Err(StoreError::Validation(_))), "skill: one that belongs to nobody is not stored, got {nobodys:?}");

    found_by_name!(stores.groups.as_ref(), group_named, "group",
        create, delete, find_by_name);
    let stray = stores.groups.create(&ctx, &Group { name: ScopedName::new("participants", Scope::SessionScope { session_id: SessionId::new() }).unwrap(), ..group(UserId::new()) }).await;
    assert!(matches!(stray, Err(StoreError::Validation(_))), "group: one in a session's scope that is not its participants is not stored, got {stray:?}");

    found_by_name!(stores.artifacts.as_ref(), artifact_named, "artifact",
        create, delete, find_by_name);
    let versionless = stores.artifacts.create(&ctx, &Artifact { versions: vec![], ..artifact(UserId::new()) }).await;
    assert!(matches!(versionless, Err(StoreError::Validation(_))), "artifact: one without a version is not stored, got {versionless:?}");
}

/// Of one kind, a name is unique in its name space (issue 250): creating,
/// renaming and restoring under a name another has there is refused,
/// compared as names are; in another name space, or once the other is
/// deleted, it is free. `$make` makes one of a name in one name space,
/// `$elsewhere` one in another, `$rename` renames one where it is.
macro_rules! unique_by_name {
    ($store:expr, $make:expr, $(elsewhere: $elsewhere:expr,)? rename: $rename:expr, $entity:literal, $create:ident, $update:ident, $delete:ident, $restore:ident) => {{
        let store = $store;
        let ctx = SecurityContext::default();
        let taken = |result| matches!(result, Err(StoreError::NameTaken { entity: $entity }));

        let first = ($make)("Helper");
        store.$create(&ctx, &first).await.expect(concat!($entity, ": create"));
        let second = ($make)("helper");
        assert!(taken(store.$create(&ctx, &second).await), concat!($entity, ": a name taken in its name space, compared as names are"));
        $(store.$create(&ctx, &($elsewhere)("helper")).await.expect(concat!($entity, ": the name is free in another name space"));)?

        let other = ($make)("Other");
        store.$create(&ctx, &other).await.expect(concat!($entity, ": create"));
        assert!(taken(store.$update(&ctx, &($rename)(other.clone(), "HELPER")).await), concat!($entity, ": nor renamed to it"));
        store.$update(&ctx, &($rename)(other.clone(), "other")).await.expect(concat!($entity, ": renamed to what is still its own name"));
        store.$update(&ctx, &($rename)(other.clone(), "Another")).await.expect(concat!($entity, ": renamed to a free one"));

        store.$delete(&ctx, first.id).await.expect(concat!($entity, ": delete"));
        store.$create(&ctx, &second).await.expect(concat!($entity, ": what is deleted holds no name"));
        assert!(taken(store.$restore(&ctx, first.id).await.map(drop)), concat!($entity, ": and is not restored under one taken meanwhile"));
        store.$update(&ctx, &($rename)(second.clone(), "Helper 2")).await.expect(concat!($entity, ": rename"));
        store.$restore(&ctx, first.id).await.expect(concat!($entity, ": once it is free again, it is"));
    }};
}

/// Names are unique per kind in their name space (issue 250), for every
/// kind that is `Scoped`.
pub(crate) async fn names_are_unique(stores: &EntityStores) {
    let store = stores.entities.as_ref();
    let user_id = UserId::new();
    let in_scope = |name: &str, scope: &Scope| ScopedName::new(name, scope.clone()).unwrap();
    let (global, users) = (Scope::GlobalScope, Scope::UserScope { user_id });
    unique_by_name!(store, |name| Personality { name: in_scope(name, &global), id: PersonalityId::new(), ..personality() },
        elsewhere: |name| Personality { name: in_scope(name, &users), id: PersonalityId::new(), ..personality() },
        rename: |value: Personality, name| Personality { name: in_scope(name, &value.name.scope), ..value }, "personality",
        create_personality, update_personality, delete_personality, restore_personality);
    unique_by_name!(store, |name| SessionTemplate { name: in_scope(name, &global), id: SessionTemplateId::new(), ..session_template() },
        elsewhere: |name| SessionTemplate { name: in_scope(name, &users), id: SessionTemplateId::new(), ..session_template() },
        rename: |value: SessionTemplate, name| SessionTemplate { name: in_scope(name, &value.name.scope), ..value }, "session_template",
        create_session_template, update_session_template, delete_session_template, restore_session_template);
    unique_by_name!(store, |name| Agent::new(in_scope(name, &global), &personality(), crate::actor::ActorId::None),
        elsewhere: |name| Agent::new(in_scope(name, &users), &personality(), crate::actor::ActorId::None),
        rename: |value: Agent, name| Agent { name: in_scope(name, &value.name.scope), ..value }, "agent",
        create_agent, update_agent, delete_agent, restore_agent);
    let (session_id, other_session) = (SessionId::new(), SessionId::new());
    unique_by_name!(store, |name| Channel { name: Name::new(name).unwrap(), session_id, id: ChannelId::new(), ..channel() },
        elsewhere: |name| Channel { name: Name::new(name).unwrap(), session_id: other_session, id: ChannelId::new(), ..channel() },
        rename: |value: Channel, name| Channel { name: Name::new(name).unwrap(), ..value }, "channel",
        create_channel, update_channel, delete_channel, restore_channel);
    unique_by_name!(store, user, rename: |value: User, name| User { username: Name::new(name).unwrap(), ..value }, "user",
        create_user, update_user, delete_user, restore_user);
    let (alice, bob) = (UserId::new(), UserId::new());
    unique_by_name!(stores.workspaces.as_ref(), |name| Workspace { name: in_scope(name, &global), id: WorkspaceId::new(), ..workspace(alice) },
        elsewhere: |name| Workspace { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: WorkspaceId::new(), ..workspace(bob) },
        rename: |value: Workspace, name| Workspace { name: in_scope(name, &value.name.scope), ..value }, "workspace",
        create, update, delete, restore);
    unique_by_name!(stores.skills.as_ref(), |name| Skill { name: in_scope(name, &global), id: SkillId::new(), ..skill(alice) },
        elsewhere: |name| Skill { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: SkillId::new(), ..skill(bob) },
        rename: |value: Skill, name| Skill { name: in_scope(name, &value.name.scope), ..value }, "skill",
        create, update, delete, restore);
    unique_by_name!(stores.artifacts.as_ref(), |name| Artifact { name: in_scope(name, &global), id: ArtifactId::new(), ..artifact(alice) },
        elsewhere: |name| Artifact { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: ArtifactId::new(), ..artifact(bob) },
        rename: |value: Artifact, name| Artifact { name: in_scope(name, &value.name.scope), ..value }, "artifact",
        create, update, delete, restore);
    unique_by_name!(stores.groups.as_ref(), |name| Group { name: in_scope(name, &global), id: GroupId::new(), ..group(alice) },
        elsewhere: |name| Group { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: GroupId::new(), ..group(bob) },
        rename: |value: Group, name| Group { name: in_scope(name, &value.name.scope), ..value }, "group",
        create, update, delete, restore);

    // Moved into a scope where its name is taken.
    let ctx = SecurityContext::default();
    let mine = Agent::new(in_scope("Moving", &users), &personality(), crate::actor::ActorId::None);
    store.create_agent(&ctx, &mine).await.expect("create");
    store.create_agent(&ctx, &Agent::new(in_scope("moving", &global), &personality(), crate::actor::ActorId::None)).await.expect("create");
    let moved = store.update_agent(&ctx, &Agent { name: in_scope("Moving", &global), ..mine }).await;
    assert!(matches!(moved, Err(StoreError::NameTaken { entity: "agent" })), "nor moved where its name is taken: {moved:?}");

    // Sessions have names, but no unique ones.
    store.create_session(&ctx, &session()).await.expect("create");
    store.create_session(&ctx, &session()).await.expect("a second session of the same name");
}

/// A name is taken also by what the caller may not read (issue 250): the
/// store must ask an `OwnershipSecurityManager`.
pub(crate) async fn names_are_unique_beyond_what_is_read(stores: &EntityStores) {
    let store = stores.entities.as_ref();
    let as_user = |id: UserId| SecurityContext::new(crate::actor::ActorId::User(id), Default::default(), Default::default());
    let (alice, bob) = (UserId::new(), UserId::new());
    let owned_by = |name: &str, owner: UserId| Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), &personality(), crate::actor::ActorId::User(owner));
    store.create_agent(&as_user(alice), &owned_by("Helper", alice)).await.expect("alice makes hers");
    let hidden = store.find_agent_by_name(&as_user(bob), &ScopedName::new("helper", Scope::GlobalScope).unwrap()).await;
    assert!(matches!(hidden, Err(StoreError::NotFound { .. })), "bob does not see hers: {hidden:?}");
    let refused = store.create_agent(&as_user(bob), &owned_by("helper", bob)).await;
    assert!(matches!(refused, Err(StoreError::NameTaken { entity: "agent" })), "but cannot take its name: {refused:?}");
}

/// Agent states that were never stored read as their default, a channel
/// state as `NotFound`; deleting a channel's state takes its agents' states
/// in the channel along - of either kind - and nothing else.
pub(crate) async fn state_store_behaves(store: &dyn StateStore) {
    let (agent, other_agent, remote_agent) = (AgentId::new(), AgentId::new(), AgentId::new());
    let (channel, other_channel) = (ChannelId::new(), ChannelId::new());

    store.get_agent_state(agent).await.expect("a missing agent state is no error");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("a missing agent channel state is no error").turn.is_none());
    let missing = store.get_channel_state(channel).await;
    assert!(matches!(missing, Err(StoreError::NotFound { entity: "channel_state", .. })), "a missing channel state is NotFound, got {missing:?}");

    let mut in_turn = AgentChannelState { turn: Some(TurnState::default()), ..AgentChannelState::default() };
    store.put_agent_channel_state(agent, channel, &mut in_turn).await.expect("put agent channel state");
    store.put_agent_channel_state(other_agent, channel, &mut in_turn).await.expect("put agent channel state");
    store.put_agent_channel_state(agent, other_channel, &mut in_turn).await.expect("put agent channel state");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("get agent channel state").turn.is_some(), "the stored state comes back");

    // An external agent's: its history, its transcript and its remote part.
    let mut talking = AgentChannelState::default();
    talking.remote.conversation = Some(ConversationId("c-1".to_string()));
    let hello = crate::message::Message { content: Some("hello".to_string()), ..crate::message::Message::new(Classification::CHANNEL_DEFAULT) };
    let answer = crate::message::Message { content: Some("hi".to_string()), ..crate::message::Message::new(Classification::CHANNEL_DEFAULT) };
    talking.history.extend(vec![hello.clone(), answer]);
    talking.transcript.extend(vec![hello]);
    store.put_agent_channel_state(remote_agent, channel, &mut talking).await.expect("put agent channel state");
    store.put_agent_channel_state(remote_agent, other_channel, &mut talking).await.expect("put agent channel state");
    let stored = store.get_agent_channel_state(remote_agent, channel).await.expect("get");
    assert_eq!(stored.remote, talking.remote, "the stored state comes back");
    assert_eq!(serde_json::to_value(talking.changes()).expect("json"), serde_json::to_value(stored.changes()).expect("json"), "what was put is marked stored: nothing pending beyond what a loaded state has");
    assert_eq!((stored.history.digest(), stored.transcript.digest()), (talking.history.digest(), talking.transcript.digest()), "with its history and its transcript");

    store.put_agent_state(agent, &mut AgentState::default()).await.expect("put agent state");
    store.get_agent_state(agent).await.expect("get agent state");

    store.put_channel_state(channel, &mut ChannelState::new(channel)).await.expect("put channel state");
    store.put_channel_state(other_channel, &mut ChannelState::new(other_channel)).await.expect("put channel state");
    assert_eq!(store.get_channel_state(channel).await.expect("get channel state").channel_id, channel);

    store.delete_channel_state(channel).await.expect("delete channel state");
    assert!(matches!(store.get_channel_state(channel).await, Err(StoreError::NotFound { .. })), "the channel state is gone");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("get").turn.is_none(), "the agents' states in the channel are gone");
    assert!(store.get_agent_channel_state(other_agent, channel).await.expect("get").turn.is_none(), "the agents' states in the channel are gone");
    assert!(store.get_agent_channel_state(agent, other_channel).await.expect("get").turn.is_some(), "other channels keep theirs");

    store.put_agent_channel_state(agent, channel, &mut in_turn).await.expect("put agent channel state");
    store.delete_agent_state(agent).await.expect("delete agent state");
    assert!(store.get_agent_channel_state(agent, channel).await.expect("get").turn.is_none(), "the agent's states in every channel are gone");
    assert!(store.get_agent_channel_state(agent, other_channel).await.expect("get").turn.is_none(), "the agent's states in every channel are gone");
    assert!(store.get_agent_channel_state(other_agent, other_channel).await.is_ok(), "another agent's stay");
    store.delete_agent_state(agent).await.expect("nothing to delete is no error");
    assert!(store.get_agent_channel_state(remote_agent, channel).await.expect("get").remote.is_empty(), "the external agents' states in the channel are gone");
    assert!(store.get_agent_channel_state(remote_agent, other_channel).await.expect("get").remote.conversation.is_some(), "other channels keep theirs");
    store.get_channel_state(other_channel).await.expect("other channels keep theirs");
    store.delete_channel_state(channel).await.expect("nothing left to delete is no error");
}
/// `value` marked deleted, as only deleting may.
fn marked<T: Deletable>(mut value: T) -> T {
    value.set_deleted(Some(Removal::now(crate::actor::ActorId::System)));
    value
}

/// Deleting marks (issue 164): what is marked is not there for any of the
/// five operations, also not for the system, and its id stays taken;
/// nothing created or changed may carry the mark. What was deleted is
/// read, restored and purged by operations of its own.
pub(crate) async fn deleting_marks(stores: &EntityStores) {
    let store = stores.entities.as_ref();
    let system = SecurityContext::system();
    let kept = session();
    store.create_session(&system, &kept).await.expect("create");
    let gone = session();
    store.create_session(&system, &gone).await.expect("create");
    assert!(store.delete_session(&system, gone.id).await.expect("delete"));

    let read = store.get_session(&system, gone.id).await;
    assert!(matches!(read, Err(StoreError::NotFound { entity: "session", .. })), "not read, not even by the system: {read:?}");
    let listed = store.list_sessions(&system, &Query::all()).await.map(|page| page.items).expect("list");
    assert_eq!(listed.iter().map(|s| s.id).collect::<Vec<_>>(), [kept.id], "not listed");
    let changed = store.update_session(&system, &gone).await;
    assert!(matches!(changed, Err(StoreError::NotFound { .. })), "not changed: {changed:?}");
    assert!(!store.delete_session(&system, gone.id).await.expect("delete"), "not deleted again");
    let again = store.create_session(&system, &gone).await;
    assert!(matches!(again, Err(StoreError::Conflict(_))), "its id stays taken: {again:?}");

    let created = store.create_session(&system, &marked(session())).await;
    assert!(matches!(created, Err(StoreError::Validation(_))), "nothing is created marked: {created:?}");
    let changed = store.update_session(&system, &marked(kept.clone())).await;
    assert!(matches!(changed, Err(StoreError::Validation(_))), "nor changed to be: {changed:?}");

    // The trash.
    let deleted = store.get_deleted_session(&system, gone.id).await.expect("read as deleted");
    let mark = deleted.deleted.expect("marked");
    assert_eq!(mark.by, system.caller_id, "by whom");
    let trash = store.list_deleted_sessions(&system, &Query::all()).await.map(|page| page.items).expect("list deleted");
    assert_eq!(trash.iter().map(|s| s.id).collect::<Vec<_>>(), [gone.id], "only what was deleted");
    let not_deleted = store.get_deleted_session(&system, kept.id).await;
    assert!(matches!(not_deleted, Err(StoreError::NotFound { .. })), "what is there is not in the trash: {not_deleted:?}");

    // Restored: there again, as it was.
    let restored = store.restore_session(&system, gone.id).await.expect("restore");
    assert!(restored.deleted.is_none(), "returned without the mark");
    assert!(store.get_session(&system, gone.id).await.expect("there again").deleted.is_none());
    let again = store.restore_session(&system, gone.id).await;
    assert!(matches!(again, Err(StoreError::NotFound { .. })), "only what was deleted is restored: {again:?}");

    // Purged: only what was deleted, gone for good, and its id free again.
    assert!(!store.purge_session(&system, gone.id).await.expect("purge restored"), "what is not marked is not purged");
    assert!(store.get_session(&system, gone.id).await.is_ok(), "and stays");
    assert!(store.delete_session(&system, gone.id).await.expect("delete"));
    assert!(store.purge_session(&system, gone.id).await.expect("purge marked"));
    assert!(!store.purge_session(&system, gone.id).await.expect("purge again"), "nothing left");
    assert!(store.list_deleted_sessions(&system, &Query::all()).await.map(|page| page.items).expect("list deleted").is_empty());
    store.create_session(&system, &gone).await.expect("its id is free again");

    let carol = user("carol");
    store.create_user(&system, &carol).await.expect("create user");
    assert!(store.delete_user(&system, carol.id).await.expect("delete user"));
    let found = store.find_user_by_name(&system, "carol").await;
    assert!(matches!(found, Err(StoreError::NotFound { .. })), "not found by name: {found:?}");
}
