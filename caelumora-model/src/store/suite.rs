//! The behaviour every store implementation owes (issue 93): one suite that
//! every backend runs - the `MemoryStore` here, the `JsonFileStore` in
//! `caelumora-store-json` (issue 446), with the feature `test-support` -, so
//! that they can't drift apart. What is specific to one store - files on
//! disk, concurrent writers - stays in that store's own tests.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use crate::history::testing::items_of;
use crate::store::query::{Field, Filter, Query};
use crate::actor::{ActorId, ActorState};
use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::turn::TurnState;
use crate::channel::{Channel, ChannelId, ChannelState, moderation_components};
use crate::component::{ComponentBundle, ComponentBundleId, ComponentConfig, ComponentDescriptor};
use crate::confidentiality::{Classification, Visibility};
use crate::external_agent::ConversationId;
use crate::value::name::{Name, ScopedName};
use crate::value::text::Introduction;
use crate::scope::{Scoped, Scope};
use crate::security::{AccessPolicy, AclEntry, ActionType, Grant, OwnershipPolicy, Secured, SecurityAttributes, SecurityContext, Subject, Verb};
use crate::store::memberships::StoreMemberships;
use std::sync::Arc;
use crate::session::{Session, SessionId};
use crate::store::{EntityKind, EntityStore, EntityStores, Journaled, NoEntity, Presence, StateStore, StoreError, Stored};
use crate::store::history::HistoryStore;
use crate::store::transcript::TranscriptStore;
use crate::actor::{ActorEvent, Envelope, EventContext};
use crate::history::History;
use crate::work::TurnId;
use crate::tool::{ToolCall, ToolSet, ToolVisibility};
use std::collections::{BTreeMap, HashSet};
use crate::chain::{Node, Position};
use crate::digest;
use crate::transcript::TranscriptEntry;
use crate::history::{Audience, HistoryEntry, HistoryPage, Reader, Recorded, EVERY_POSITION};
use crate::template::{ChannelTemplate, ParticipantTemplate, SessionTemplate, SessionTemplateId};
use crate::removal::Removal;
use crate::user::{ReadMark, User, UserChannelState, UserId, UserProfile};
use crate::message::MessageId;
use crate::workspace::{Workspace, WorkspaceId};
use crate::group::{Group, GroupId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::message::{Message, MessageRole};
use crate::role::RoleId;
use crate::work::Work;

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
        assert!(matches!(conflict, Err(StoreError::Conflict(_))), concat!($entity, ": creating twice is a conflict - its id before its name -, got {:?}"), conflict);

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
        let taken = store.$create(&ctx, &value).await;
        assert!(matches!(taken, Err(StoreError::Conflict(_))), concat!($entity, ": its id stays taken once it is deleted, got {:?}"), taken);
    }};
}

fn scoped(name: &str) -> ScopedName {
    ScopedName::new(name, Scope::GlobalScope).unwrap()
}

fn personality() -> Personality {
    Personality {
        introduction: None,
        reasoning_visibility: Visibility::Channel,
        hears_during_turn: Default::default(),
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
        access: Vec::new(),
        summary: None, briefing: None, language: None, time_zone: None,
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
            access: Vec::new(),
            summary: None, briefing: None, language: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            name: Name::new("general").unwrap(),
            components: moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            participants: vec!["user".to_string()],
            internal_actors: Vec::new(),
        }],
        deleted: None,
    }
}

fn session() -> Session {
    Session {
        summary: None, briefing: None, language: None, time_zone: None,
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
        components: moderation_components(None, Some("broadcast_policy")),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        former: Vec::new(),
        participants: Vec::new(),
        internal_actors: Vec::new(),
        security_attributes: Default::default(),
        deleted: None,
    }
}

fn user(username: &str) -> User {
    User {
        id: UserId::new(),
        username: Name::new(username).unwrap(),
        email: String::new(),
        roles: vec![RoleId::new()],
        deleted: None,
    }
}

fn workspace(owner: UserId) -> Workspace {
    workspace_named(ScopedName::new("docs", Scope::UserScope { user_id: owner }).unwrap())
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

define_id!(FolderId);

/// A kind with parts that have access lists of their own (issue 328): a
/// folder of documents, each published in a channel and read by whom its
/// own list names, as an artifact of the plugin `caelumora-artifacts` has
/// versions. None of the core's kinds has such parts, so the suite brings
/// one, to ask for what lies within an element of a collection in an
/// element of a collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    id: FolderId,
    documents: Vec<Document>,
    security_attributes: SecurityAttributes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deleted: Option<Removal>,
}

/// A part of a `Folder`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    session_id: SessionId,
    channel_id: ChannelId,
    acl: Vec<AclEntry>,
}

/// What a query of folders may name, as `ArtifactField` of the plugin
/// does: those of a document lie within an element of `Documents`, those of
/// an entry within an element of `Acl` or `DocumentAcl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderField {
    Owner,
    Acl,
    Documents,
    DocumentSession,
    DocumentChannel,
    DocumentAcl,
    Subject,
}

impl Field for FolderField {
    fn path(self) -> &'static [&'static str] {
        match self {
            FolderField::Owner => &["security_attributes", "owner_id"],
            FolderField::Acl => &["security_attributes", "acl"],
            FolderField::Documents => &["documents"],
            FolderField::DocumentSession => &["session_id"],
            FolderField::DocumentChannel => &["channel_id"],
            FolderField::DocumentAcl => &["acl"],
            FolderField::Subject => &["subject"],
        }
    }
}

impl Folder {
    /// A folder of `owner` with one document, published in a channel of a
    /// session of its own.
    fn of(owner: UserId) -> Self {
        let document = Document { session_id: SessionId::new(), channel_id: ChannelId::new(), acl: Vec::new() };
        Self { id: FolderId::new(), documents: vec![document], security_attributes: SecurityAttributes::owned_by(ActorId::User(owner)), deleted: None }
    }
}

impl Secured for Folder {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Stored for Folder {
    const KIND: EntityKind = EntityKind::new("folder");
    type Id = FolderId;
    type Within = NoEntity;
    type Field = FolderField;

    fn id(&self) -> FolderId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }
}

fn bundle() -> ComponentBundle {
    ComponentBundle::new(scoped("search"), ComponentConfig { component_nodes: vec![ComponentDescriptor::new("search", "web_search")] }, Default::default())
}

/// A store answers a query of a list (issue 293): those that meet its
/// filter - fields of the entity, of an element of a collection in it, of
/// an element of a collection in that -, in the order of their ids, in pages
/// of its limit, each going on after the cursor the one before handed out.
/// Asked of `folders`, a kind with parts (`Folder`), as a plugin's kind may
/// have them.
pub async fn queries_reach_parts(folders: &dyn EntityStore<Folder>) {
    let ctx = SecurityContext::system();
    let (alice, bob) = (UserId::new(), UserId::new());
    let (session, channel) = (SessionId::new(), ChannelId::new());
    let bobs = Subject::Actor(ActorId::User(bob));
    let mut made = Vec::new();
    for (index, owner) in [alice, alice, bob, alice, bob].into_iter().enumerate() {
        let mut folder = Folder::of(owner);
        match index {
            // A document published in the channel asked about.
            1 => (folder.documents[0].session_id, folder.documents[0].channel_id) = (session, channel),
            // Shared with bob as a whole.
            3 => folder.security_attributes.acl.push(AclEntry { subject: bobs, grant: Grant::Read }),
            // A document of it shared with bob, published elsewhere in the session.
            4 => {
                folder.documents[0].session_id = session;
                folder.documents[0].acl.push(AclEntry { subject: bobs, grant: Grant::Read });
            }
            _ => {}
        }
        folders.create(&ctx, &folder).await.expect("a folder");
        made.push(folder.id);
    }
    let ids = |page: &[Folder]| page.iter().map(|f| f.id).collect::<Vec<_>>();
    let mut sorted = made.clone();
    sorted.sort();
    let listed = folders.list(&ctx, &Query::all()).await.expect("all of them");
    assert_eq!(ids(&listed.items), sorted, "in the order of their ids");
    assert!(listed.next.is_none());

    let owned = Filter::eq(FolderField::Owner, ActorId::User(bob));
    let found = folders.list(&ctx, &Query::filtered(owned)).await.unwrap();
    let mut bobs_own = vec![made[2], made[4]];
    bobs_own.sort();
    assert_eq!(ids(&found.items), bobs_own, "a field of the entity");

    let place = Filter::And(vec![Filter::eq(FolderField::DocumentSession, session), Filter::eq(FolderField::DocumentChannel, channel)]);
    let published = folders.list(&ctx, &Query::filtered(Filter::any(FolderField::Documents, place))).await.unwrap();
    assert_eq!(ids(&published.items), [made[1]], "a document published in the channel, not one elsewhere in the session");

    let named = Filter::eq(FolderField::Subject, bobs);
    let shared = Filter::Or(vec![
        Filter::any(FolderField::Acl, named.clone()),
        Filter::any(FolderField::Documents, Filter::any(FolderField::DocumentAcl, named)),
    ]);
    let mut with_bob = vec![made[3], made[4]];
    with_bob.sort();
    assert_eq!(ids(&folders.list(&ctx, &Query::filtered(shared)).await.unwrap().items), with_bob, "an entry of its list or of a document's");

    let mut paged = Vec::new();
    let mut query = Query::all().limited(2);
    loop {
        let page = folders.list(&ctx, &query).await.unwrap();
        assert!(page.items.len() <= 2, "at most its limit");
        paged.extend(ids(&page.items));
        match page.next {
            Some(cursor) => query = Query::all().limited(2).after(cursor),
            None => break,
        }
    }
    assert_eq!(paged, sorted, "the pages, one after the other, are the whole list");
}

/// A store answers a query by the fields of the core's kinds: by scope, by
/// the session of a channel, by a participant, by a member.
pub async fn queries_behave(stores: &EntityStores) {
    let ctx = SecurityContext::system();
    let (alice, bob) = (UserId::new(), UserId::new());
    // By scope, by the session of a channel, by a participant.
    use crate::channel::{ChannelField, Participant};
    let session_id = SessionId(Uuid::new_v4());
    let in_session = Personality { name: ScopedName::new("in-session", Scope::SessionScope { session_id }).unwrap(), ..personality() };
    let in_channel = Personality { name: ScopedName::new("in-channel", Scope::ChannelScope { session_id, channel_id: ChannelId(Uuid::new_v4()) }).unwrap(), ..personality() };
    let elsewhere = Personality { name: scoped("elsewhere"), ..personality() };
    for personality in [&in_session, &in_channel, &elsewhere] {
        stores.personalities.create(&ctx, personality).await.unwrap();
    }
    let holder = Scope::SessionScope { session_id };
    let lying = stores.personalities.list(&ctx, &Query::filtered(Filter::lying_within(&holder))).await.unwrap();
    let mut expected = vec![in_session.id, in_channel.id];
    expected.sort();
    assert_eq!(lying.items.iter().map(|p| p.id).collect::<Vec<_>>(), expected, "what lies in the session or a channel of it");

    let agent = AgentId(Uuid::new_v4());
    let mut with_agent = self::session();
    with_agent.participants.push(Participant::Agent(agent));
    let without = self::session();
    stores.sessions.create(&ctx, &with_agent).await.unwrap();
    stores.sessions.create(&ctx, &without).await.unwrap();
    let found = stores.sessions.list(&ctx, &Query::filtered(Filter::with_participant(ActorId::Agent(agent)))).await.unwrap();
    assert_eq!(found.items.iter().map(|s| s.id).collect::<Vec<_>>(), [with_agent.id], "a participant, the whole element");

    let of_it = Channel { session_id: with_agent.id, ..self::channel() };
    stores.channels.create(&ctx, &of_it).await.unwrap();
    stores.channels.create(&ctx, &Channel { session_id: without.id, ..self::channel() }).await.unwrap();
    let found = stores.channels.list(&ctx, &Query::filtered(Filter::eq(ChannelField::Session, with_agent.id))).await.unwrap();
    assert_eq!(found.items.iter().map(|c| c.id).collect::<Vec<_>>(), [of_it.id], "the channels of a session");

    // By a member (issue 308).
    let mut his = group(alice);
    his.members.push(ActorId::User(bob));
    stores.groups.create(&ctx, &his).await.unwrap();
    stores.groups.create(&ctx, &group(UserId::new())).await.unwrap();
    let belonging = Filter::with_member(ActorId::User(bob));
    let found = stores.groups.list(&ctx, &Query::filtered(belonging)).await.unwrap();
    assert_eq!(found.items.iter().map(|g| g.id).collect::<Vec<_>>(), [his.id], "the groups of a member, not those of their owner");
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

/// Every kind of entity goes through the same life; users are also found
/// by name, and so are definitions in their scope.
pub async fn entity_store_behaves(stores: &EntityStores) {
    lifecycle!(stores.personalities.as_ref(), personality(), |v: &Personality| v.id, name.name, "personality",
        create, get, update, delete, list);
    lifecycle!(stores.session_templates.as_ref(), session_template(), |v: &SessionTemplate| v.id, name.name, "session_template",
        create, get, update, delete, list);
    lifecycle!(stores.agents.as_ref(), Agent::new(scoped("assistant-1"), personality().id, ActorId::None), |v: &Agent| v.id, name.name, "agent",
        create, get, update, delete, list);
    lifecycle!(stores.sessions.as_ref(), session(), |v: &Session| v.id, name, "session",
        create, get, update, delete, list);
    lifecycle!(stores.channels.as_ref(), channel(), |v: &Channel| v.id, name, "channel",
        create, get, update, delete, list);
    lifecycle!(stores.users.as_ref(), user("alice"), |v: &User| v.id, username, "user",
        create, get, update, delete, list);
    lifecycle!(stores.user_profiles.as_ref(), UserProfile::new(UserId::new(), None), |v: &UserProfile| v.user_id(), introduction = Some(Introduction::new("renamed").unwrap()), "user_profile",
        create, get, update, delete, list);
    lifecycle!(stores.workspaces.as_ref(), workspace(UserId::new()), |v: &Workspace| v.id, name.name, "workspace",
        create, get, update, delete, list);
    lifecycle!(stores.groups.as_ref(), group(UserId::new()), |v: &Group| v.id, name.name, "group",
        create, get, update, delete, list);
    lifecycle!(stores.component_bundles.as_ref(), bundle(), |v: &ComponentBundle| v.id, name.name, "component_bundle",
        create, get, update, delete, list);

    let ctx = SecurityContext::default();
    let alice = user("alice");
    stores.users.create(&ctx, &alice).await.expect("create user");
    stores.users.create(&ctx, &user("bob")).await.expect("create user");
    let found = stores.users.find_by_name(&ctx, &scoped("alice")).await.expect("alice is found by name");
    assert_eq!(found.id, alice.id);
    let missing = stores.users.find_by_name(&ctx, &scoped("carol")).await;
    assert!(matches!(missing, Err(StoreError::NotFound { entity: "user", .. })), "an unknown name is NotFound, got {missing:?}");

    // A template that would make sessions it can never lie in (issue 178).
    let ctx = SecurityContext::default();
    let in_session = SessionTemplate { name: ScopedName::new("nested", Scope::SessionScope { session_id: SessionId::new() }).unwrap(), ..session_template() };
    let refused = stores.session_templates.create(&ctx, &in_session).await;
    assert!(matches!(refused, Err(StoreError::Validation(_))), "a template of a session's scope is not stored, got {refused:?}");
    let template = session_template();
    stores.session_templates.create(&ctx, &template).await.expect("a global one is");
    let moved = SessionTemplate { name: ScopedName::new("nested", Scope::ChannelScope { session_id: SessionId::new(), channel_id: ChannelId::new() }).unwrap(), ..template };
    let refused = stores.session_templates.update(&ctx, &moved).await;
    assert!(matches!(refused, Err(StoreError::Validation(_))), "nor moved into a channel's, got {refused:?}");

    found_by_name!(stores.personalities.as_ref(), |name| Personality { name, id: PersonalityId::new(), ..personality() }, "personality",
        create, delete, find_by_name);
    found_by_name!(stores.session_templates.as_ref(), |name| SessionTemplate { name, id: SessionTemplateId::new(), ..session_template() }, "session_template",
        create, delete, find_by_name);
    found_by_name!(stores.agents.as_ref(), |name| Agent::new(name, personality().id, ActorId::None), "agent",
        create, delete, find_by_name);

    found_by_name!(stores.component_bundles.as_ref(), |name| ComponentBundle { name, ..bundle() }, "component_bundle",
        create, delete, find_by_name);

    found_by_name!(stores.workspaces.as_ref(), workspace_named, "workspace",
        create, delete, find_by_name);
    let nobodys = stores.workspaces.create(&ctx, &Workspace { security_attributes: Default::default(), ..workspace(UserId::new()) }).await;
    assert!(matches!(nobodys, Err(StoreError::Validation(_))), "workspace: one that belongs to no user is not stored, got {nobodys:?}");

    found_by_name!(stores.groups.as_ref(), group_named, "group",
        create, delete, find_by_name);
    let stray = stores.groups.create(&ctx, &Group { name: ScopedName::new("participants", Scope::SessionScope { session_id: SessionId::new() }).unwrap(), ..group(UserId::new()) }).await;
    assert!(matches!(stray, Err(StoreError::Validation(_))), "group: one in a session's scope that is not its participants is not stored, got {stray:?}");
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
pub async fn names_are_unique(stores: &EntityStores) {
    let user_id = UserId::new();
    let in_scope = |name: &str, scope: &Scope| ScopedName::new(name, scope.clone()).unwrap();
    let (global, users) = (Scope::GlobalScope, Scope::UserScope { user_id });
    unique_by_name!(stores.personalities.as_ref(), |name| Personality { name: in_scope(name, &global), id: PersonalityId::new(), ..personality() },
        elsewhere: |name| Personality { name: in_scope(name, &users), id: PersonalityId::new(), ..personality() },
        rename: |value: Personality, name| Personality { name: in_scope(name, &value.name.scope), ..value }, "personality",
        create, update, delete, restore);
    unique_by_name!(stores.session_templates.as_ref(), |name| SessionTemplate { name: in_scope(name, &global), id: SessionTemplateId::new(), ..session_template() },
        elsewhere: |name| SessionTemplate { name: in_scope(name, &users), id: SessionTemplateId::new(), ..session_template() },
        rename: |value: SessionTemplate, name| SessionTemplate { name: in_scope(name, &value.name.scope), ..value }, "session_template",
        create, update, delete, restore);
    unique_by_name!(stores.agents.as_ref(), |name| Agent::new(in_scope(name, &global), personality().id, ActorId::None),
        elsewhere: |name| Agent::new(in_scope(name, &users), personality().id, ActorId::None),
        rename: |value: Agent, name| Agent { name: in_scope(name, &value.name.scope), ..value }, "agent",
        create, update, delete, restore);
    let (session_id, other_session) = (SessionId::new(), SessionId::new());
    unique_by_name!(stores.channels.as_ref(), |name| Channel { name: Name::new(name).unwrap(), session_id, id: ChannelId::new(), ..channel() },
        elsewhere: |name| Channel { name: Name::new(name).unwrap(), session_id: other_session, id: ChannelId::new(), ..channel() },
        rename: |value: Channel, name| Channel { name: Name::new(name).unwrap(), ..value }, "channel",
        create, update, delete, restore);
    unique_by_name!(stores.users.as_ref(), user, rename: |value: User, name| User { username: Name::new(name).unwrap(), ..value }, "user",
        create, update, delete, restore);
    let (alice, bob) = (UserId::new(), UserId::new());
    unique_by_name!(stores.workspaces.as_ref(), |name| Workspace { name: in_scope(name, &global), id: WorkspaceId::new(), ..workspace(alice) },
        elsewhere: |name| Workspace { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: WorkspaceId::new(), ..workspace(bob) },
        rename: |value: Workspace, name| Workspace { name: in_scope(name, &value.name.scope), ..value }, "workspace",
        create, update, delete, restore);
    unique_by_name!(stores.groups.as_ref(), |name| Group { name: in_scope(name, &global), id: GroupId::new(), ..group(alice) },
        elsewhere: |name| Group { name: in_scope(name, &Scope::UserScope { user_id: bob }), id: GroupId::new(), ..group(bob) },
        rename: |value: Group, name| Group { name: in_scope(name, &value.name.scope), ..value }, "group",
        create, update, delete, restore);
    unique_by_name!(stores.component_bundles.as_ref(), |name| ComponentBundle { name: in_scope(name, &global), id: ComponentBundleId::new(), ..bundle() },
        elsewhere: |name| ComponentBundle { name: in_scope(name, &users), id: ComponentBundleId::new(), ..bundle() },
        rename: |value: ComponentBundle, name| ComponentBundle { name: in_scope(name, &value.name.scope), ..value }, "component_bundle",
        create, update, delete, restore);

    // Moved into a scope where its name is taken.
    let ctx = SecurityContext::default();
    let mine = Agent::new(in_scope("Moving", &users), personality().id, ActorId::None);
    stores.agents.create(&ctx, &mine).await.expect("create");
    stores.agents.create(&ctx, &Agent::new(in_scope("moving", &global), personality().id, ActorId::None)).await.expect("create");
    let moved = stores.agents.update(&ctx, &Agent { name: in_scope("Moving", &global), ..mine }).await;
    assert!(matches!(moved, Err(StoreError::NameTaken { entity: "agent" })), "nor moved where its name is taken: {moved:?}");

    // Sessions have names, but no unique ones.
    stores.sessions.create(&ctx, &session()).await.expect("create");
    stores.sessions.create(&ctx, &session()).await.expect("a second session of the same name");
}

/// A name is taken also by what the caller may not read (issue 250): the
/// store must ask an `OwnershipPolicy`.
pub async fn names_are_unique_beyond_what_is_read(stores: &EntityStores) {
    let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
    let (alice, bob) = (UserId::new(), UserId::new());
    let owned_by = |name: &str, owner: UserId| Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), personality().id, ActorId::User(owner));
    stores.agents.create(&as_user(alice), &owned_by("Helper", alice)).await.expect("alice makes hers");
    let hidden = stores.agents.find_by_name(&as_user(bob), &ScopedName::new("helper", Scope::GlobalScope).unwrap()).await;
    assert!(matches!(hidden, Err(StoreError::NotFound { .. })), "bob does not see hers: {hidden:?}");
    let refused = stores.agents.create(&as_user(bob), &owned_by("helper", bob)).await;
    assert!(matches!(refused, Err(StoreError::NameTaken { entity: "agent" })), "but cannot take its name: {refused:?}");
}

/// Agent states that were never stored read as their default, a channel
/// state as `NotFound`; deleting a channel's state takes its agents' states
/// in the channel along - of either kind - and nothing else.
pub async fn state_store_behaves(store: &dyn StateStore) {
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

    // An external agent's: its transcript and its remote part.
    let mut talking = AgentChannelState::default();
    talking.remote.conversation = Some(ConversationId("c-1".to_string()));
    let hello = Message { content: Some("hello".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    talking.transcript.extend(vec![hello]);
    store.put_agent_channel_state(remote_agent, channel, &mut talking).await.expect("put agent channel state");
    store.put_agent_channel_state(remote_agent, other_channel, &mut talking).await.expect("put agent channel state");
    let stored = store.get_agent_channel_state(remote_agent, channel).await.expect("get");
    assert_eq!(stored.remote, talking.remote, "the stored state comes back");
    assert_eq!(serde_json::to_value(talking.changes()).expect("json"), serde_json::to_value(stored.changes()).expect("json"), "what was put is marked stored: nothing pending beyond what a loaded state has");
    assert_eq!(stored.transcript.digest(), talking.transcript.digest(), "with its transcript");
    // Stored in a second put, as what changed since the first (issue 376).
    talking.transcript.extend(vec![Message { content: Some("and more".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }]);
    store.put_agent_channel_state(remote_agent, channel, &mut talking).await.expect("put agent channel state");
    let stored = store.get_agent_channel_state(remote_agent, channel).await.expect("get");
    let positions = |state: &AgentChannelState| state.transcript.all_entries().map(|node| (node.seq, node.digest)).collect::<Vec<_>>();
    assert_eq!(positions(&stored), positions(&talking), "every entry where it was added");
    assert_eq!(stored.transcript.end().get(), 2);

    store.put_agent_state(agent, &mut AgentState::default()).await.expect("put agent state");
    store.get_agent_state(agent).await.expect("get agent state");

    store.put_channel_state(channel, &mut ChannelState::new(channel)).await.expect("put channel state");
    store.put_channel_state(other_channel, &mut ChannelState::new(other_channel)).await.expect("put channel state");
    assert_eq!(store.get_channel_state(channel).await.expect("get channel state").channel_id, channel);

    // A user's: what they read (issue 404).
    let (user, other_user) = (UserId::new(), UserId::new());
    assert!(store.get_user_channel_state(user, channel).await.expect("a missing user channel state is no error").read.is_none(), "nothing read");
    let mark = ReadMark { position: Position::new(7), message: MessageId::new() };
    let mut read = UserChannelState::default();
    read.read_up_to(mark);
    store.put_user_channel_state(user, channel, &mut read).await.expect("put user channel state");
    store.put_user_channel_state(user, other_channel, &mut read).await.expect("put user channel state");
    read.read_up_to(ReadMark { position: Position::new(9), message: MessageId::new() });
    store.put_user_channel_state(other_user, channel, &mut read).await.expect("put user channel state");
    assert_eq!(store.get_user_channel_state(user, channel).await.expect("get").read, Some(mark), "the stored state comes back");
    assert_eq!(store.get_user_channel_state(other_user, channel).await.expect("get").read.map(|read| read.position.get()), Some(9), "each user's own");

    store.delete_channel_state(channel).await.expect("delete channel state");
    assert!(matches!(store.get_channel_state(channel).await, Err(StoreError::NotFound { .. })), "the channel state is gone");
    assert!(store.get_user_channel_state(user, channel).await.expect("get").read.is_none(), "the users' states in the channel are gone");
    assert!(store.get_user_channel_state(other_user, channel).await.expect("get").read.is_none(), "the users' states in the channel are gone");
    assert_eq!(store.get_user_channel_state(user, other_channel).await.expect("get").read, Some(mark), "other channels keep theirs");
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

/// The index of the work under way (issue 388): a channel is listed while
/// an agent's state stored in it has work under way, once however many
/// agents have; it leaves the list with the put that ends the last one's
/// work, and with the agent's or the channel's state deleted.
pub async fn work_under_way_is_indexed(store: &dyn StateStore) {
    let (agent, other_agent) = (AgentId::new(), AgentId::new());
    let (channel, other_channel, waiting_channel) = (ChannelId::new(), ChannelId::new(), ChannelId::new());
    let listed = async || store.channels_with_work_under_way().await.expect("listed").into_iter().collect::<HashSet<_>>();
    assert!(listed().await.is_empty(), "nothing stored, nothing under way");

    let in_turn = |processing_state: ActorState| AgentChannelState { processing_state, turn: Some(TurnState::default()), ..AgentChannelState::default() };
    let mut planning = in_turn(ActorState::Planning);
    store.put_agent_channel_state(agent, channel, &mut planning).await.expect("put");
    store.put_agent_channel_state(other_agent, channel, &mut planning).await.expect("put");
    store.put_agent_channel_state(agent, other_channel, &mut planning).await.expect("put");
    store.put_agent_channel_state(agent, waiting_channel, &mut in_turn(ActorState::WaitingForUser)).await.expect("put");
    store.put_agent_channel_state(other_agent, waiting_channel, &mut AgentChannelState::default()).await.expect("put");
    assert_eq!(listed().await, HashSet::from([channel, other_channel]), "each channel with work under way once, none that waits or idles");
    store.put_agent_channel_state(agent, channel, &mut planning).await.expect("put");
    assert_eq!(listed().await, HashSet::from([channel, other_channel]), "a put that goes on with the work keeps it listed");

    store.put_agent_channel_state(agent, channel, &mut AgentChannelState::default()).await.expect("put");
    assert_eq!(listed().await, HashSet::from([channel, other_channel]), "another agent's work in the channel is still under way");
    store.put_agent_channel_state(other_agent, channel, &mut in_turn(ActorState::WaitingForUser)).await.expect("put");
    assert_eq!(listed().await, HashSet::from([other_channel]), "the last one's work ended");

    store.delete_agent_state(agent).await.expect("delete agent state");
    assert!(listed().await.is_empty(), "the agent's work goes with its state");
    store.put_agent_channel_state(other_agent, channel, &mut planning).await.expect("put");
    store.delete_channel_state(channel).await.expect("delete channel state");
    assert!(listed().await.is_empty(), "the channel's work goes with its state");
}
/// A history's item as a test compares it: its message's id and how it
/// stands, or what else it is.
fn standing(item: &HistoryEntry) -> String {
    match item.said() {
        Some(message) => format!("{} {:?} {:?} {:?}", message.id, message.content, message.revoked.is_some(), message.classification),
        None => format!("{item:?}"),
    }
}

/// Every page of `key` from `from` on, read backwards from the end and put
/// together, oldest first.
async fn all_pages(histories: &dyn HistoryStore, key: ChannelId, from: Position, limit: usize) -> Vec<String> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = histories.page(key, &Reader::Channel, from, before, limit, None).await.expect("a page");
        assert!(page.items.len() <= limit, "no more than asked for");
        pages.push(page.entries().map(standing).collect::<Vec<_>>());
        match page.earlier {
            Some(earlier) => before = Some(earlier),
            None => break,
        }
    }
    pages.into_iter().rev().flatten().collect()
}

/// The histories a store keeps with the states (issue 379): written by the
/// puts, read as they stand, in pages, by message, by position; the items
/// derived from the chain, after changes, after a fitting, after a rebuild;
/// loaded from where a state holds its history; read for a reader (issue
/// 417); deleted with the channel's state. More entries than a segment of
/// the JSON store holds.
#[expect(clippy::too_many_lines, reason = "to be simplified; older than the lint (issue 452)")]
pub async fn history_store_behaves(states: &dyn StateStore, histories: &dyn HistoryStore) {
    use crate::message::testing::{id, make_message};
    use crate::confidentiality::Confidentiality;
    let (channel, agent) = (ChannelId::new(), AgentId::new());
    let key = channel;
    assert!(histories.page(key, &Reader::Channel, Position::START, None, 10, None).await.unwrap().items.is_empty(), "no history yet");
    assert!(histories.said(key, &Reader::Channel, id(1)).await.unwrap().is_none());

    let mut state = ChannelState::new(channel);
    for n in 1..=300 {
        state.history.push(make_message(id(n)));
    }
    states.put_channel_state(channel, &mut state).await.expect("put");
    assert!(state.history.changes().is_none(), "the history is marked stored");
    for n in 301..=600 {
        state.history.push(make_message(id(n)));
    }
    state.history.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) });
    assert!(state.history.revoke(id(300), Utc::now()));
    state.history.fit_into(&Classification::general(Confidentiality::Public));
    assert!(state.history.reclassify(id(599), Classification::general(Confidentiality::Public)));
    states.put_channel_state(channel, &mut state).await.expect("put what changed");
    let expected: Vec<String> = items_of(&state.history).iter().map(standing).collect();

    assert_eq!(all_pages(histories, key, Position::START, 70).await, expected, "the items as they stand, page by page");
    let last = histories.page(key, &Reader::Channel, Position::START, None, 3, None).await.unwrap();
    assert_eq!((last.entries().map(standing).collect::<Vec<_>>(), last.earlier), (expected[597..].to_vec(), Some(Position::new(597))));
    let from_middle = all_pages(histories, key, Position::new(250), 100).await;
    assert_eq!(from_middle, expected[250..].to_vec(), "none before where a view begins");
    let (at, first) = histories.said(key, &Reader::Channel, id(1)).await.unwrap().expect("said");
    assert_eq!((at, first.said().and_then(|m| m.content.as_deref())), (Position::START, Some("changed")), "as it stands, where it was said");
    assert!(histories.said(key, &Reader::Channel, id(300)).await.unwrap().is_some_and(|(_, item)| item.said().is_some_and(|m| m.revoked.is_some())));
    assert_eq!(histories.page(key, &Reader::Channel, Position::START, None, 1, None).await.unwrap().digest, state.history.digest(), "read from the history as it is");
    let entries = histories.entries(key, &Reader::Channel, Position::new(598)..Position::MAX).await.unwrap();
    assert_eq!(entries.iter().map(|node| node.seq.get()).collect::<Vec<_>>(), state.history.from(Position::new(598)).map(|node| node.seq.get()).collect::<Vec<_>>(), "the chain itself from a position on");
    let between = histories.entries(key, &Reader::Channel, Position::new(250)..Position::new(253)).await.unwrap();
    assert_eq!(between.iter().map(|node| node.seq.get()).collect::<Vec<_>>(), [250, 251, 252], "and up to where a range ends");
    assert!(histories.entries(key, &Reader::Channel, Position::new(10_000)..Position::MAX).await.unwrap().is_empty());

    let changed_or_revoked = Filter::Or(vec![Filter::containing("changed"), Filter::all_of([Filter::said(), Filter::negated(Filter::written())])]);
    let kept = histories.page(key, &Reader::Channel, Position::START, None, 5, Some(&changed_or_revoked)).await.unwrap();
    assert_eq!((kept.items.len(), kept.earlier), (2, None), "a page of those kept, read back as far as there are any");

    histories.rebuild(key).await.expect("rebuild");
    assert_eq!(all_pages(histories, key, Position::START, 200).await, expected, "the items derived anew are the same");

    let loaded = states.get_channel_state(channel).await.expect("get");
    assert_eq!((loaded.history.digest(), loaded.history.end().get(), loaded.history.base().get()), (state.history.digest(), state.history.end().get(), 0), "the history comes back whole");
    state.history_from = Position::new(590);
    states.put_channel_state(channel, &mut state).await.expect("put where the window begins");
    let loaded = states.get_channel_state(channel).await.expect("get");
    assert_eq!((loaded.history.base().get(), loaded.history.end().get(), loaded.history.digest()), (590, state.history.end().get(), state.history.digest()), "from where the state holds it");
    let mut going_on = loaded;
    going_on.history.push(make_message(id(601)));
    states.put_channel_state(channel, &mut going_on).await.expect("appended to what was loaded in part");
    assert_eq!(histories.said(key, &Reader::Channel, id(601)).await.unwrap().map(|(at, _)| at), Some(state.history.end()));

    // Read for a reader (issue 417): what went to them and what they sent,
    // the latter in full; what went to every member for whoever was one
    // then, and for an outsider. Both users are given all of it as they join.
    let (user, other, stranger) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()), ActorId::User(UserId::new()));
    let speaker = ActorId::Agent(agent);
    let hidden = ToolCall { id: Some("1".to_string()), function_name: "look".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::HIDDEN, presentation: Default::default() };
    let answer = Message { sender_id: speaker, role: MessageRole::Assistant, content: Some("found it".to_string()), tool_calls: vec![hidden.clone()], ..make_message(id(602)) };
    let mut more = states.get_channel_state(channel).await.expect("get");
    for member in [user, other] {
        more.history.enter(Recorded::new(HistoryEntry::HistoryShared { participant: member, from: Position::new(0), by: speaker, by_name: String::new(), at: Utc::now() }, ActorId::System, Audience::Everybody));
        more.history.enter(Recorded::new(HistoryEntry::ParticipantJoined { participant: member, name: String::new(), by: speaker, by_name: String::new(), at: Utc::now() }, speaker, Audience::Everybody));
    }
    more.history.enter(Recorded::new(HistoryEntry::Said { message: answer }, speaker, Audience::Everybody));
    more.history.enter(Recorded::new(HistoryEntry::Said { message: Message { sender_id: ActorId::System, ..make_message(id(603)) } }, ActorId::System, Audience::Only(vec![user])));
    states.put_channel_state(channel, &mut more).await.expect("put");
    let calls = |said: Option<(Position, HistoryEntry)>| said.and_then(|(_, item)| item.said().map(|m| m.tool_calls.len()));
    assert_eq!(calls(histories.said(key, &Reader::Participant(speaker), id(602)).await.unwrap()), Some(1), "its sender reads it in full");
    assert_eq!(calls(histories.said(key, &Reader::Participant(user), id(602)).await.unwrap()), Some(0), "everyone else as it was published");
    assert_eq!(calls(histories.said(key, &Reader::Outsider, id(602)).await.unwrap()), Some(0));
    assert!(histories.said(key, &Reader::Participant(user), id(603)).await.unwrap().is_some(), "what went to them alone");
    assert!(histories.said(key, &Reader::Participant(other), id(603)).await.unwrap().is_none(), "not to another");
    assert!(histories.said(key, &Reader::Outsider, id(603)).await.unwrap().is_none(), "nor to an outsider");
    assert!(histories.said(key, &Reader::Participant(stranger), id(602)).await.unwrap().is_none(), "nothing that went to every member to who never was one");
    assert_eq!(histories.memberships(key, other).await.unwrap(), vec![EVERY_POSITION], "a member from what was shared, and still");
    assert!(histories.memberships(key, stranger).await.unwrap().is_empty());
    let last = |page: HistoryPage| page.entries().filter_map(HistoryEntry::said).map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(last(histories.page(key, &Reader::Participant(user), Position::START, None, 2, None).await.unwrap()), [id(602), id(603)]);
    // Newest first, what they see: the answer, their join and share, the other's join, 601.
    assert_eq!(last(histories.page(key, &Reader::Participant(other), Position::START, None, 5, None).await.unwrap()), [id(601), id(602)], "a page is filled with what they may see");
    let seen = histories.entries(key, &Reader::Participant(other), Position::new(600)..Position::MAX).await.unwrap();
    assert_eq!(seen.iter().filter_map(|node| node.entry.entry.said()).map(|m| m.id).collect::<Vec<_>>(), [id(601), id(602)], "and so is the chain");

    // Issue 223: what was recorded of an envelope is read with its head, and
    // the answer of a turn from the messages said before its end - those of
    // the turn that are not internal, as the reader may see them, back to
    // the turn before, also over more than a segment of the JSON store.
    let (earlier, turn) = (TurnId::new(), TurnId::new());
    let by_speaker = |n: u128, turn_id: TurnId, internal: bool| Message { sender_id: speaker, role: MessageRole::Assistant, content: Some(format!("{n}")), turn_id: Some(turn_id), internal, tool_calls: vec![hidden.clone()], ..make_message(id(n)) };
    let sent = |event: ActorEvent| Envelope::new(SecurityContext::default(), EventContext { sender_id: speaker, initiator_id: user, agent_hops: 2, ..EventContext::default() }, event);
    let mut turns = states.get_channel_state(channel).await.expect("get");
    let record = |history: &mut History, event: ActorEvent| assert!(history.enter(Recorded::of(&sent(event), speaker, Audience::Everybody).expect("recorded")));
    let first = turns.history.end();
    record(&mut turns.history, ActorEvent::CompleteMessageGenerated(by_speaker(700, earlier, false)));
    record(&mut turns.history, ActorEvent::TurnFinished { turn_id: earlier, messages: Vec::new() });
    record(&mut turns.history, ActorEvent::CompleteMessageGenerated(by_speaker(701, turn, false)));
    record(&mut turns.history, ActorEvent::IntermediateMessageGenerated(by_speaker(702, turn, true)));
    for n in 0..300 {
        turns.history.push(Message { sender_id: user, ..make_message(id(800 + n)) });
    }
    record(&mut turns.history, ActorEvent::CompleteMessageGenerated(by_speaker(703, turn, false)));
    let ended = turns.history.end();
    record(&mut turns.history, ActorEvent::TurnFinished { turn_id: turn, messages: Vec::new() });
    states.put_channel_state(channel, &mut turns).await.expect("put");
    let head = histories.entries(key, &Reader::Participant(user), first..first.next()).await.unwrap();
    assert_eq!(head.first().map(|node| (node.entry.context.initiator_id, node.entry.context.agent_hops)), Some((user, 2)), "with the context it came in");
    let answer = histories.answer(key, &Reader::Participant(user), ended).await.unwrap();
    assert_eq!(answer.iter().map(|m| (m.id, m.tool_calls.len())).collect::<Vec<_>>(), [(id(701), 0), (id(703), 0)], "the turn's, not internal, as published");
    let of_sender = histories.answer(key, &Reader::Participant(speaker), ended).await.unwrap();
    assert_eq!(of_sender.iter().map(|m| m.tool_calls.len()).collect::<Vec<_>>(), [1, 1], "its sender reads it in full");
    assert_eq!(histories.answer(key, &Reader::Participant(user), first.next()).await.unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [id(700)], "the turn before");
    assert!(histories.answer(key, &Reader::Participant(user), first).await.unwrap().is_empty(), "no turn ends there");
    assert!(histories.answer(key, &Reader::Participant(user), Position::new(100_000)).await.unwrap().is_empty(), "nothing there");

    states.delete_channel_state(channel).await.expect("delete");
    assert!(histories.page(key, &Reader::Channel, Position::START, None, 10, None).await.unwrap().items.is_empty(), "the channel's history is gone with its state");
}

/// A transcript kept apart from its state (issue 380): written by the put,
/// loaded from where the state holds it, every branch read by the
/// `TranscriptStore` - a path to a digest, a message as it stands on its
/// branch, which messages it holds -, a digest there twice, and gone with
/// the state.
pub async fn transcript_store_behaves(states: &dyn StateStore, transcripts: &dyn TranscriptStore) {
    use crate::message::testing::{id, make_message};
    let (channel, agent) = (ChannelId::new(), AgentId::new());
    let seqs = |path: Option<Vec<Node<TranscriptEntry>>>| path.map(|path| path.iter().map(|node| node.seq.get()).collect::<Vec<_>>());
    assert!(transcripts.path(agent, channel, digest::prompt("nothing")).await.unwrap().is_none(), "no transcript yet");
    assert!(transcripts.message(agent, channel, id(1)).await.unwrap().is_none());

    let mut state = AgentChannelState::default();
    for n in 1..=300 {
        state.transcript.push(make_message(id(n)));
    }
    assert!(state.transcript.modify(Message { content: Some("changed".to_string()), ..make_message(id(1)) }));
    let first = state.transcript.start_anew().expect("a branch");
    state.transcript.push(make_message(id(301)));
    states.put_agent_channel_state(agent, channel, &mut state).await.expect("put");
    assert!(state.transcript.changes().is_none(), "the transcript is marked stored");

    let expected = seqs(state.transcript.prefix(&first).map(|path| path.into_iter().cloned().collect()));
    assert_eq!(seqs(transcripts.path(agent, channel, first).await.unwrap()), expected, "the path to a digest on an older branch");
    assert_eq!(seqs(transcripts.path(agent, channel, state.transcript.digest().unwrap()).await.unwrap()), Some(vec![301]), "on the active one");
    assert_eq!(transcripts.message(agent, channel, id(1)).await.unwrap().and_then(|m| m.content), Some("changed".to_string()), "as it stands on its branch");
    assert!(transcripts.message(agent, channel, id(999)).await.unwrap().is_none());
    let held = transcripts.in_transcript(agent, channel, &[id(1), id(301), id(999)]).await.unwrap();
    assert_eq!(held, [id(1), id(301)].into_iter().collect(), "which it holds, on whichever branch");

    let loaded = states.get_agent_channel_state(agent, channel).await.expect("get");
    assert_eq!((loaded.transcript.digest(), loaded.transcript.end().get(), loaded.transcript.base().get()), (state.transcript.digest(), state.transcript.end().get(), 0), "the transcript comes back whole");

    // Held from a position on, a root given again is added a second time.
    let end = state.transcript.end();
    state.transcript.start_anew();
    state.transcript_from = end;
    states.put_agent_channel_state(agent, channel, &mut state).await.expect("put where the window begins");
    let mut window = states.get_agent_channel_state(agent, channel).await.expect("get");
    assert_eq!((window.transcript.base(), window.transcript.is_empty()), (end, true), "from where the state holds it");
    let root = state.transcript.prefix(&first).and_then(|path| path.first().map(|node| node.digest)).expect("a root");
    window.transcript.push(make_message(id(1)));
    assert_eq!(window.transcript.digest(), Some(root), "the same digest");
    states.put_agent_channel_state(agent, channel, &mut window).await.expect("a digest there twice");
    assert_eq!(seqs(transcripts.path(agent, channel, root).await.unwrap()), Some(vec![end.get()]), "the newest");

    let mut other = AgentChannelState::default();
    other.transcript.push(make_message(id(1)));
    states.put_agent_channel_state(agent, ChannelId::new(), &mut other).await.expect("put");
    states.delete_channel_state(channel).await.expect("delete");
    assert!(transcripts.path(agent, channel, root).await.unwrap().is_none(), "gone with the channel's state");
    states.put_agent_channel_state(agent, channel, &mut AgentChannelState::default()).await.expect("put");
    let mut again = AgentChannelState::default();
    again.transcript.push(make_message(id(2)));
    states.put_agent_channel_state(agent, channel, &mut again).await.expect("put");
    states.delete_agent_state(agent).await.expect("delete");
    assert!(transcripts.in_transcript(agent, channel, &[id(2)]).await.unwrap().is_empty(), "and with the agent");
}

/// The prompt texts and the tool sets an agent's model was given (issue
/// 383), and the frames of its contexts (issue 426): written by the put of
/// its state, apart from its document, each once by its digest - written
/// again as the same -, read by digest; a state loaded holds the texts and
/// sets its turn under way names, none between turns, and no frame; gone
/// with the state.
pub async fn given_store_behaves(states: &dyn StateStore, transcripts: &dyn TranscriptStore) {
    let (channel, agent) = (ChannelId::new(), AgentId::new());
    let sets = vec![ToolSet::new("calc", "calculates", Vec::new())];
    let offer = digest::tool_sets(&sets);
    let mut state = AgentChannelState::default();
    let before = state.keep_prompt(MessageRole::System, "before".to_string(), None, None);
    states.put_agent_channel_state(agent, channel, &mut state).await.expect("put");
    assert!(state.prompts.changes().is_empty(), "the texts are marked stored");

    state.begin_turn(Work::new(EventContext::default(), None, SecurityContext::default(), Classification::CHANNEL_DEFAULT, Default::default()));
    state.offer_tools(sets.clone());
    let system = state.keep_prompt(MessageRole::System, "system".to_string(), None, None);
    state.turn.as_mut().expect("a turn").work.context.frame.prompts.push(system.clone());
    let context = state.turn.as_ref().expect("a turn").work.context.clone();
    let frame = state.keep_frame(&context.frame);
    states.put_agent_channel_state(agent, channel, &mut state).await.expect("put");
    assert!(state.offered_tools.changes().is_empty(), "the tool sets are marked stored");
    assert!(state.frames.changes().is_empty(), "the frames too");

    let read = transcripts.prompts(agent, channel, &[before.text, system.text, digest::prompt("never")]).await.unwrap();
    let expected: BTreeMap<_, _> = [(before.text, "before".to_string()), (system.text, "system".to_string())].into_iter().collect();
    assert_eq!(read, expected, "those kept, by their digests");
    assert_eq!(transcripts.offered_tools(agent, channel, offer).await.unwrap(), Some(sets.clone()));
    assert!(transcripts.offered_tools(agent, channel, digest::prompt("never")).await.unwrap().is_none());
    assert_eq!(transcripts.frame(agent, channel, frame).await.unwrap(), Some(context.frame.clone()));
    assert_eq!(transcripts.context(agent, channel, context.transcript, frame).await.unwrap(), Some(context.clone()), "a message's context, whole");
    assert!(transcripts.context(agent, channel, context.transcript, digest::prompt("never")).await.unwrap().is_none(), "not without its frame");

    let loaded = states.get_agent_channel_state(agent, channel).await.expect("get");
    assert_eq!(loaded.prompts.get(&system.text).map(String::as_str), Some("system"), "what its turn under way names");
    assert!(loaded.prompts.get(&before.text).is_none(), "and nothing else");
    assert_eq!(loaded.offered_tools.get(&offer), Some(&sets));
    assert!(loaded.prompts.changes().is_empty() && loaded.offered_tools.changes().is_empty(), "nothing to write again");
    assert!(loaded.frames.is_empty(), "no frame: the turn holds its own");

    let mut next = AgentChannelState::default();
    next.keep_prompt(MessageRole::System, "system".to_string(), None, None);
    states.put_agent_channel_state(agent, channel, &mut next).await.expect("a text kept again is written as the same");
    assert_eq!(transcripts.prompts(agent, channel, &[system.text]).await.unwrap().get(&system.text).map(String::as_str), Some("system"));
    let between = states.get_agent_channel_state(agent, channel).await.expect("get");
    assert!(between.prompts.is_empty() && between.offered_tools.is_empty(), "between turns a state holds none");

    states.delete_channel_state(channel).await.expect("delete");
    assert!(transcripts.prompts(agent, channel, &[system.text]).await.unwrap().is_empty(), "gone with the channel's state");
    assert!(transcripts.frame(agent, channel, frame).await.unwrap().is_none(), "the frames too");
    let elsewhere = ChannelId::new();
    let mut other = AgentChannelState::default();
    other.keep_prompt(MessageRole::System, "system".to_string(), None, None);
    states.put_agent_channel_state(agent, elsewhere, &mut other).await.expect("put");
    states.delete_agent_state(agent).await.expect("delete");
    assert!(transcripts.prompts(agent, elsewhere, &[system.text]).await.unwrap().is_empty(), "and with the agent");
}

/// `value` marked deleted, as only deleting may.
fn marked<T: Stored>(mut value: T) -> T {
    value.set_deleted(Some(Removal::now(ActorId::System)));
    value
}

/// Deleting marks (issue 164): what is marked is not there for any of the
/// five operations, also not for the system, and its id stays taken;
/// nothing created or changed may carry the mark. What was deleted is
/// read, restored and purged by operations of its own.
pub async fn deleting_marks(stores: &EntityStores) {
    let system = SecurityContext::system();
    let kept = session();
    stores.sessions.create(&system, &kept).await.expect("create");
    let gone = session();
    stores.sessions.create(&system, &gone).await.expect("create");
    assert!(stores.sessions.delete(&system, gone.id).await.expect("delete"));

    let read = stores.sessions.get(&system, gone.id).await;
    assert!(matches!(read, Err(StoreError::NotFound { entity: "session", .. })), "not read, not even by the system: {read:?}");
    let listed = stores.sessions.list(&system, &Query::all()).await.map(|page| page.items).expect("list");
    assert_eq!(listed.iter().map(|s| s.id).collect::<Vec<_>>(), [kept.id], "not listed");
    let changed = stores.sessions.update(&system, &gone).await;
    assert!(matches!(changed, Err(StoreError::NotFound { .. })), "not changed: {changed:?}");
    assert!(!stores.sessions.delete(&system, gone.id).await.expect("delete"), "not deleted again");
    let again = stores.sessions.create(&system, &gone).await;
    assert!(matches!(again, Err(StoreError::Conflict(_))), "its id stays taken: {again:?}");

    let created = stores.sessions.create(&system, &marked(session())).await;
    assert!(matches!(created, Err(StoreError::Validation(_))), "nothing is created marked: {created:?}");
    let changed = stores.sessions.update(&system, &marked(kept.clone())).await;
    assert!(matches!(changed, Err(StoreError::Validation(_))), "nor changed to be: {changed:?}");

    // The trash.
    let deleted = stores.sessions.get_deleted(&system, gone.id).await.expect("read as deleted");
    let mark = deleted.deleted.expect("marked");
    assert_eq!(mark.by, system.caller_id, "by whom");
    let trash = stores.sessions.list_deleted(&system, &Query::all()).await.map(|page| page.items).expect("list deleted");
    assert_eq!(trash.iter().map(|s| s.id).collect::<Vec<_>>(), [gone.id], "only what was deleted");
    let not_deleted = stores.sessions.get_deleted(&system, kept.id).await;
    assert!(matches!(not_deleted, Err(StoreError::NotFound { .. })), "what is there is not in the trash: {not_deleted:?}");

    // Restored: there again, as it was.
    let restored = stores.sessions.restore(&system, gone.id).await.expect("restore");
    assert!(restored.deleted.is_none(), "returned without the mark");
    assert!(stores.sessions.get(&system, gone.id).await.expect("there again").deleted.is_none());
    let again = stores.sessions.restore(&system, gone.id).await;
    assert!(matches!(again, Err(StoreError::NotFound { .. })), "only what was deleted is restored: {again:?}");

    // Purged: only what was deleted, gone for good, and its id free again.
    assert!(!stores.sessions.purge(&system, gone.id).await.expect("purge restored"), "what is not marked is not purged");
    assert!(stores.sessions.get(&system, gone.id).await.is_ok(), "and stays");
    assert!(stores.sessions.delete(&system, gone.id).await.expect("delete"));
    assert!(stores.sessions.purge(&system, gone.id).await.expect("purge marked"));
    assert!(!stores.sessions.purge(&system, gone.id).await.expect("purge again"), "nothing left");
    assert!(stores.sessions.list_deleted(&system, &Query::all()).await.map(|page| page.items).expect("list deleted").is_empty());
    stores.sessions.create(&system, &gone).await.expect("its id is free again");

    let carol = user("carol");
    stores.users.create(&system, &carol).await.expect("create user");
    assert!(stores.users.delete(&system, carol.id).await.expect("delete user"));
    let found = stores.users.find_by_name(&system, &scoped("carol")).await;
    assert!(matches!(found, Err(StoreError::NotFound { .. })), "not found by name: {found:?}");
}

/// The ownership policy, learning who belongs to a group from what
/// `lists_admit_what_check_decides` binds it to - for the stores that suite
/// is run on.
pub fn admitting_policy() -> (Arc<StoreMemberships>, Arc<dyn AccessPolicy>) {
    let memberships = Arc::new(StoreMemberships::new());
    let access_policy: Arc<dyn AccessPolicy> = Arc::new(OwnershipPolicy::with_memberships(Arc::<StoreMemberships>::clone(&memberships)));
    (memberships, access_policy)
}

/// A list hands out what `check` decides of each entity (issue 332), though
/// the storage narrows it by the policy's admission first: of folders owned
/// by a user, an agent, the system and nobody, with an entry for a user, a role, a
/// group - one that was deleted too, one with an agent - and everyone, of
/// every grant; listed by users, by an agent as its owner and as itself, by
/// a role, by nobody; of what is there and what was deleted, in pages
/// smaller than a scan. `stores` and `folders` ask `access_policy`, made by
/// `admitting_policy` with `memberships`.
pub async fn lists_admit_what_check_decides(stores: &EntityStores, folders: &dyn EntityStore<Folder>, memberships: &StoreMemberships, access_policy: &dyn AccessPolicy) {
    memberships.bind(&stores.groups);
    let system = SecurityContext::system();
    let (alice, bob) = (UserId::new(), UserId::new());
    let (hers, his) = (ActorId::User(alice), ActorId::User(bob));
    let agent = ActorId::Agent(AgentId::new());
    let role = RoleId::new();
    let group = |name: &str, member: ActorId| Group { members: vec![member], ..Group::team(scoped(name), alice) };
    let (team, gone) = (group("team", his), group("gone", his));
    // The participants of a channel, among them an agent.
    let channel_id = ChannelId::new();
    let participants = ScopedName::new(Group::PARTICIPANTS, Scope::ChannelScope { session_id: SessionId::new(), channel_id }).unwrap();
    let place = Group { id: GroupId::of_channel(channel_id), name: participants, ..group("place", agent) };
    for group in [&team, &gone, &place] {
        stores.groups.create(&system, group).await.expect("create group");
    }
    assert!(stores.groups.delete(&system, gone.id).await.expect("delete group"));

    let subjects = [Subject::Actor(his), Subject::Role(role), Subject::Group(team.id), Subject::Group(gone.id), Subject::Group(place.id), Subject::Everyone];
    let mut made = Vec::new();
    for owner in [hers, his, agent, ActorId::System, ActorId::None] {
        made.push(SecurityAttributes::owned_by(owner));
        for subject in subjects {
            for grant in Grant::ALL {
                made.push(SecurityAttributes::owned_by(owner).granting(subject, grant));
            }
        }
    }
    for (index, security_attributes) in made.into_iter().enumerate() {
        let folder = Folder { security_attributes, ..Folder::of(alice) };
        folders.create(&system, &folder).await.expect("create folder");
        if index % 3 == 0 {
            assert!(folders.delete(&system, folder.id).await.expect("delete folder"));
        }
    }
    let there = all_folders(folders, &system, Presence::Present).await;
    let deleted = all_folders(folders, &system, Presence::Deleted).await;

    let contexts = [
        SecurityContext::new(hers, SessionId::new(), ChannelId::new()),
        SecurityContext::new(his, SessionId::new(), ChannelId::new()),
        SecurityContext::new(ActorId::User(UserId::new()), SessionId::new(), ChannelId::new()),
        SecurityContext::new(his, SessionId::new(), ChannelId::new()).with_roles(vec![role], None),
        SecurityContext::new(his, SessionId::new(), ChannelId::new()).next(agent, his, his),
        SecurityContext::new(his, SessionId::new(), ChannelId::new()).next(agent, his, his).with_roles(vec![role], Some(Vec::new())),
        SecurityContext::default(),
    ];
    for context in &contexts {
        for (presence, verb, stored) in [(Presence::Present, Verb::Get, &there), (Presence::Deleted, Verb::ReadDeleted, &deleted)] {
            let mut readable = Vec::new();
            for folder in stored {
                if access_policy.check(context, ActionType::entity(Folder::KIND, verb, &folder.security_attributes)).await {
                    readable.push(folder.id);
                }
            }
            let listed: Vec<FolderId> = all_folders(folders, context, presence).await.iter().map(|folder| folder.id).collect();
            assert_eq!(listed, readable, "listed what is read one by one, among {presence:?}, for {context:?}");
        }
    }
}

/// Every folder of `presence` that `context` lists, in pages of a few.
async fn all_folders(folders: &dyn EntityStore<Folder>, context: &SecurityContext, presence: Presence) -> Vec<Folder> {
    let mut query = Query::all().limited(7);
    let mut all = Vec::new();
    loop {
        let page = match presence {
            Presence::Present => folders.list(context, &query).await,
            Presence::Deleted => folders.list_deleted(context, &query).await,
        }.expect("list folders");
        all.extend(page.items);
        let Some(next) = page.next else { return all };
        query = query.after(next);
    }
}
