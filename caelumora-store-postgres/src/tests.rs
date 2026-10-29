//! The Postgres store against a database: the one `test/docker-compose.yml`
//! starts, named by the variable `CAELUMORA_TEST_POSTGRES_URL` - each test in
//! a schema of its own, dropped when it is done. Without the variable the
//! tests that need a database are skipped, and say so.

use super::*;
use crate::connection::{quoted, sql};
use crate::filter::Condition;
use crate::plugin::PostgresStorePlugin;
use caelumora_model::actor::{ActorId, ActorState, EventContext};
use caelumora_model::agent::{AgentChannelState, AgentId, AgentState};
use caelumora_model::chain::Position;
use caelumora_model::channel::{ChannelId, ChannelState};
use caelumora_model::confidentiality::Classification;
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::digest;
use caelumora_model::approval::ApproverId;
use caelumora_model::history::{Audience, HistoryEntry, Reader, Recorded};
use caelumora_model::store::history::Memberships;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::tool::{ToolCall, ToolCallState, ToolSet, ToolVisibility};
use caelumora_model::work::Work;
use caelumora_model::confidentiality::ConfidentialityRules;
use caelumora_model::group::Group;
use caelumora_model::removal::RemovalId;
use caelumora_model::scope::Scope;
use caelumora_model::secret::env::EnvSecretProvider;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{AccessPolicy, Admission, AllowAllPolicy, Grant, OwnershipPolicy, SecurityAttributes, SecurityContext, Subject};
use caelumora_model::role::RoleId;
use caelumora_model::store::documents::Document;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::query::{FieldPath, FieldValue, Filter};
use caelumora_model::store::suite;
use caelumora_model::store::{EntityStoreFactory, Persistence, Presence, StoreError};
use caelumora_model::user::UserId;
use caelumora_model::value::name::ScopedName;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::runtime::{RuntimeBuilder, Services};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use std::env;

/// The variable, read as a secret, that holds the URL of the test database.
const URL_SECRET: &str = "CAELUMORA_TEST_POSTGRES_URL";

/// The secrets of the tests: the environment.
fn secrets() -> Arc<SecretManager> {
    Arc::new(SecretManager::new(vec![Arc::new(EnvSecretProvider::new())], Arc::new(AllowAllPolicy::new())))
}

/// A schema of its own in the test database.
struct Database {
    connection: Arc<Connection>,
}

impl Database {
    /// A new schema - `None`, and the test skipped, if no test database is
    /// named.
    fn new() -> Option<Self> {
        if env::var(URL_SECRET).is_err() {
            eprintln!("skipped: {URL_SECRET} names no test database (test/docker-compose.yml)");
            return None;
        }
        let schema = format!("test_{}", Uuid::new_v4().simple());
        Some(Self { connection: Arc::new(Connection::new(URL_SECRET, &schema, 4, secrets())) })
    }

    /// A backend over the schema - another for every call, as of another
    /// process, over the same pool.
    fn backend(&self) -> Arc<PostgresStore> {
        Arc::new(PostgresStore::new(Arc::clone(&self.connection)))
    }

    /// The stores of the entities in the schema, of a backend of their own,
    /// checked by `policy` - the states in memory -, with the core's kinds
    /// opened and the backends opened after them, as at start.
    async fn stores(&self, policy: Arc<dyn AccessPolicy>) -> Persistence {
        let stores = Persistence::new(self.backend(), Arc::new(MemoryStore::new()), policy, Arc::new(ConfidentialityRules::default())).unwrap();
        stores.entity_stores().unwrap();
        stores.open().await.expect("opened against the test database");
        stores
    }

    /// Drops the schema with everything in it.
    async fn drop(self) {
        let pool = self.connection.pool().await.unwrap();
        sqlx::query(sql(format!("DROP SCHEMA {} CASCADE", quoted(self.connection.schema())))).execute(pool).await.unwrap();
    }
}

#[tokio::test]
async fn behaves_as_an_entity_store() {
    let Some(database) = Database::new() else { return };
    suite::entity_store_behaves(&database.stores(Arc::new(AllowAllPolicy::new())).await.entity_stores().unwrap()).await;
    database.drop().await;
}

/// The kind of the folders is opened after the backend was: its table is
/// made when it is first used.
#[tokio::test]
async fn answers_queries() {
    let Some(database) = Database::new() else { return };
    let stores = database.stores(Arc::new(AllowAllPolicy::new())).await;
    suite::queries_behave(&stores.entity_stores().unwrap()).await;
    suite::queries_reach_parts(stores.entity_store::<suite::Folder>().unwrap().as_ref()).await;
    database.drop().await;
}

#[tokio::test]
async fn lists_what_check_decides() {
    let Some(database) = Database::new() else { return };
    let (memberships, access_policy) = suite::admitting_policy();
    let stores = database.stores(Arc::clone(&access_policy)).await;
    let folders = stores.entity_store::<suite::Folder>().unwrap();
    suite::lists_admit_what_check_decides(&stores.entity_stores().unwrap(), folders.as_ref(), &memberships, access_policy.as_ref()).await;
    database.drop().await;
}

#[tokio::test]
async fn keeps_what_is_deleted_marked() {
    let Some(database) = Database::new() else { return };
    suite::deleting_marks(&database.stores(Arc::new(AllowAllPolicy::new())).await.entity_stores().unwrap()).await;
    database.drop().await;
}

#[tokio::test]
async fn keeps_names_unique() {
    let Some(database) = Database::new() else { return };
    suite::names_are_unique(&database.stores(Arc::new(AllowAllPolicy::new())).await.entity_stores().unwrap()).await;
    database.drop().await;
}

#[tokio::test]
async fn keeps_names_unique_beyond_what_is_read() {
    let Some(database) = Database::new() else { return };
    suite::names_are_unique_beyond_what_is_read(&database.stores(Arc::new(OwnershipPolicy::new())).await.entity_stores().unwrap()).await;
    database.drop().await;
}

/// Two backends over one schema - as two processes - keep what the other
/// wrote, and a name unique against the other, by the database's index.
#[tokio::test]
async fn backends_of_one_schema_keep_one_store() {
    let Some(database) = Database::new() else { return };
    let (here, there) = (database.stores(Arc::new(AllowAllPolicy::new())).await, database.stores(Arc::new(AllowAllPolicy::new())).await);
    let owner = UserId::new();
    let team = |name: &str| Group::team(ScopedName::new(name, Scope::UserScope { user_id: owner }).unwrap(), owner);
    let system = SecurityContext::system();
    let ours = team("Ours");
    here.entity_stores().unwrap().groups.create(&system, &ours).await.unwrap();
    assert!(there.entity_stores().unwrap().groups.get(&system, ours.id).await.is_ok(), "read by the other");
    let taken = there.entity_stores().unwrap().groups.create(&system, &team("ours")).await;
    assert!(matches!(taken, Err(StoreError::NameTaken { entity: "group" })), "its name taken: {taken:?}");
    database.drop().await;
}

/// A table of a form this program does not know is refused when the
/// backend is opened, naming the kind.
#[tokio::test]
async fn a_table_of_another_form_is_refused() {
    let Some(database) = Database::new() else { return };
    database.stores(Arc::new(AllowAllPolicy::new())).await;
    let pool = database.connection.pool().await.unwrap();
    sqlx::query("UPDATE kinds SET schema_version = 99 WHERE name = 'group'").execute(pool).await.unwrap();
    let stores = Persistence::new(database.backend(), Arc::new(MemoryStore::new()), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).unwrap();
    stores.entity_stores().unwrap();
    let refused = stores.open().await.expect_err("of another form");
    assert!(refused.to_string().contains("the kind group has the form 99"), "{refused}");
    database.drop().await;
}

/// A secret that names nothing fails the opening, naming the secret.
#[tokio::test]
async fn a_secret_that_names_nothing_fails_the_start() {
    let connection = Arc::new(Connection::new("CAELUMORA_TEST_POSTGRES_NO_SUCH_URL", "public", 1, secrets()));
    let refused = PostgresStore::new(connection).open().await.expect_err("no URL");
    assert!(refused.to_string().contains("'CAELUMORA_TEST_POSTGRES_NO_SUCH_URL'"), "{refused}");
}

/// The backend is made from its parameters, refuses wrong ones naming the
/// store, and keeps the entities and the states.
#[test]
fn the_backend_is_made_from_its_parameters() {
    let backends = Backends::with_plugins(&[&PostgresStorePlugin]);
    let services = Services::empty().with(secrets());
    let postgres = BackendConfig::new("postgres", json!({ "url_secret": URL_SECRET, "schema": "caelumora", "max_connections": 2 }));
    let made = backends.make_one::<dyn PersistenceBackend>("entity store", "entity_store", &postgres, &services).unwrap();
    Persistence::over(made, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).expect("keeps the states too");

    let misspelt = BackendConfig::new("postgres", json!({ "url": "postgres://somewhere" }));
    let refused = backends.make_one::<dyn PersistenceBackend>("entity store", "entity_store", &misspelt, &services).map(|_| ()).unwrap_err().to_string();
    assert!(refused.contains("the entity store (postgres)") && refused.contains("url"), "{refused}");
}

#[tokio::test]
async fn behaves_as_a_state_store() {
    let Some(database) = Database::new() else { return };
    suite::state_store_behaves(database.backend().as_ref()).await;
    database.drop().await;
}

#[tokio::test]
async fn indexes_the_work_under_way() {
    let Some(database) = Database::new() else { return };
    suite::work_under_way_is_indexed(database.backend().as_ref()).await;
    database.drop().await;
}

#[tokio::test]
async fn keeps_the_histories() {
    let Some(database) = Database::new() else { return };
    let store = database.backend();
    suite::history_store_behaves(store.as_ref(), store.as_ref()).await;
    database.drop().await;
}

#[tokio::test]
async fn keeps_the_transcripts() {
    let Some(database) = Database::new() else { return };
    let store = database.backend();
    suite::transcript_store_behaves(store.as_ref(), store.as_ref()).await;
    suite::given_store_behaves(store.as_ref(), store.as_ref()).await;
    database.drop().await;
}

/// A restart (issue 04): what an agent suspended on an approval stored -
/// its turn with the call it waits for, its transcript, what its model was
/// given - and the channel's history are read back whole by the store of
/// another process, over a pool of its own in the same schema.
#[tokio::test]
async fn what_an_agent_waiting_for_an_approval_stored_survives_a_restart() {
    let Some(database) = Database::new() else { return };
    let (agent, channel) = (AgentId::new(), ChannelId::new());
    let said = |content: &str| Message { content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let mut channel_state = ChannelState::new(channel);
    channel_state.history.push(said("look it up"));

    let mut state = AgentChannelState { processing_state: ActorState::WaitingForUser, ..AgentChannelState::default() };
    let context = EventContext { sender_id: ActorId::Agent(agent), channel_id: channel, ..EventContext::default() };
    state.begin_turn(Work::new(context, None, SecurityContext::default(), Classification::CHANNEL_DEFAULT, Default::default()));
    let sets = vec![ToolSet::new("search", "searches", Vec::new())];
    state.offer_tools(sets.clone());
    let system = state.keep_prompt(MessageRole::System, "system".to_string(), None, None);
    state.transcript.push(said("look it up"));
    let call = ToolCall { id: Some("1".to_string()), function_name: "search".to_string(), arguments: json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let turn = state.turn.as_mut().expect("a turn");
    turn.work.context.frame.prompts.push(system.clone());
    turn.tool_calls.push(ToolCallState::pending(call, "searches the web".to_string(), false, None, None));
    let asked: Vec<_> = turn.pending_approvals().into_iter().map(|request| request.id).collect();
    assert_eq!(asked.len(), 1, "waits for an approval");

    let before = database.backend();
    before.put_channel_state(channel, &mut channel_state).await.unwrap();
    before.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
    drop(before);

    let restarted = PostgresStore::new(Arc::new(Connection::new(URL_SECRET, database.connection.schema(), 2, secrets())));
    restarted.open().await.expect("opened again");
    let loaded = restarted.get_agent_channel_state(agent, channel).await.unwrap();
    let pending: Vec<_> = loaded.turn.as_ref().expect("the turn under way").pending_approvals().into_iter().map(|request| request.id).collect();
    assert_eq!(pending, asked, "the same request, asked again from the turn");
    assert_eq!((loaded.processing_state, loaded.transcript.digest()), (ActorState::WaitingForUser, state.transcript.digest()), "with its transcript");
    assert_eq!(loaded.prompts.get(&system.text).map(String::as_str), Some("system"), "what its turn names of what was given");
    assert_eq!(loaded.offered_tools.get(&digest::tool_sets(&sets)), Some(&sets));
    let history = restarted.get_channel_state(channel).await.unwrap().history;
    assert_eq!(history.digest(), channel_state.history.digest(), "the channel's history");
    let page = restarted.page(channel, &Reader::Channel, Position::START, None, 10, None).await.unwrap();
    assert_eq!(page.entries().filter_map(HistoryEntry::said).filter_map(|m| m.content.clone()).collect::<Vec<_>>(), ["look it up"]);
    database.drop().await;
}

/// The database gives a reader of a history what the model gives them
/// (issue 451): what they sent, what went to them by name, what went to
/// every member while they were one, an approver what went to the
/// approvers, an outsider what went to every user - each only if anything
/// of it is published.
#[tokio::test]
async fn the_database_gives_a_reader_what_the_model_gives_them() {
    let Some(database) = Database::new() else { return };
    let store = database.backend();
    let channel = ChannelId::new();
    let (alice, bob, agent) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()), ActorId::Agent(AgentId::new()));
    let approver = ActorId::Approver(ApproverId::new());
    let joined = |participant| HistoryEntry::ParticipantJoined { participant, name: String::new(), by: participant, by_name: String::new(), at: Utc::now() };
    let said = |by: ActorId, content: Option<&str>| HistoryEntry::Said { message: Message { sender_id: by, content: content.map(str::to_string), ..Message::new(Classification::CHANNEL_DEFAULT) } };
    let mut state = ChannelState::new(channel);
    for (entry, sender, audience) in [
        (joined(alice), alice, Audience::Everybody),
        (said(alice, Some("hello")), alice, Audience::Everybody),
        (HistoryEntry::HistoryShared { participant: bob, from: Position::new(1), by: alice, by_name: String::new(), at: Utc::now() }, ActorId::System, Audience::Everybody),
        (joined(bob), alice, Audience::Everybody),
        (said(agent, Some("to alice")), agent, Audience::Only(vec![alice])),
        (said(bob, None), bob, Audience::Everybody),
        (said(agent, Some("to the approvers")), agent, Audience::Only(vec![approver])),
        (HistoryEntry::ParticipantLeft { participant: alice, name: String::new(), by: alice, by_name: String::new(), until: Position::new(6), at: Utc::now() }, alice, Audience::Everybody),
        (said(bob, Some("after her")), bob, Audience::Everybody),
    ] {
        state.history.enter(Recorded::new(entry, sender, audience));
    }
    store.put_channel_state(channel, &mut state).await.unwrap();
    let asked_of_all = |state: &ChannelState| state.history.all_entries().filter(|node| node.entry.entry.is_item()).count();
    let pool = database.connection.pool().await.unwrap();
    let memberships = Memberships::of(state.history.all_entries());
    for reader in [Reader::Channel, Reader::Outsider, Reader::Participant(alice), Reader::Participant(bob), Reader::Participant(agent), Reader::Participant(approver)] {
        let ranges = memberships.of_reader(&reader);
        let given: Vec<i64> = state.history.all_entries()
            .filter(|node| node.entry.entry.is_item() && node.entry.seen_by(node.seq, &reader, &ranges))
            .map(|node| i64::try_from(node.seq.get()).unwrap()).collect();
        let condition = Filter::seen_by(&reader, &ranges).map(|rule| Condition::of(&rule.by_path(), "item", 2));
        let meeting = condition.as_ref().map_or("true", |condition| condition.sql.as_str());
        let query = sqlx::query_as(sql(format!("SELECT seq FROM history_items WHERE channel_id = $1 AND {meeting} ORDER BY seq"))).bind(channel.to_string());
        let query = match &condition {
            Some(condition) => condition.bound(query),
            None => query,
        };
        let asked: Vec<(i64,)> = query.fetch_all(pool).await.unwrap();
        assert_eq!(asked.into_iter().map(|(seq,)| seq).collect::<Vec<_>>(), given, "{reader:?}");
        assert!(!given.is_empty() && (matches!(reader, Reader::Channel) || given.len() < asked_of_all(&state)), "{reader:?} is given some, less than everything");
    }
    database.drop().await;
}

/// A scan hands out what its admission admits (issue 332), by the access
/// entries each write keeps in step with its document: by the owner, an
/// entry for an actor, a role, everyone, or a group an actor belongs to - a
/// deleted one only where the admission says so -, of a grant that covers
/// the one needed; nothing else. They go with the document.
#[tokio::test]
async fn a_scan_hands_out_what_its_admission_admits() {
    let Some(database) = Database::new() else { return };
    let groups = Arc::new(PgDocuments::new(Arc::clone(&database.connection), Group::KIND, None).unwrap());
    let storage = PgDocuments::new(Arc::clone(&database.connection), EntityKind::new("probe"), Some(Arc::clone(&groups))).unwrap();
    let (alice, bob) = (UserId::new(), UserId::new());
    let (hers, his) = (ActorId::User(alice), ActorId::User(bob));
    let role = RoleId::new();
    let group = |name: &str, members: Vec<ActorId>| Group { members, ..Group::team(ScopedName::new(name, Scope::GlobalScope).unwrap(), alice) };
    let (team, gone, other) = (group("team", vec![his]), group("gone", vec![his]), group("other", vec![hers]));
    for (group, removal_id) in [(&team, None), (&gone, Some(RemovalId::new())), (&other, None)] {
        groups.insert(Document { id: group.id.into(), name: None, removal_id, security_attributes: group.security_attributes.clone(), body: json!(group) }).await.unwrap();
    }
    let shared = |subject, grant| SecurityAttributes::owned_by(hers).granting(subject, grant);
    let made = [
        SecurityAttributes::owned_by(his),
        shared(Subject::Actor(his), Grant::Read),
        shared(Subject::Actor(his), Grant::Join),
        shared(Subject::Role(role), Grant::Read),
        shared(Subject::Everyone, Grant::Read),
        shared(Subject::Group(team.id), Grant::Write),
        shared(Subject::Group(gone.id), Grant::Read),
        SecurityAttributes::owned_by(hers),
        SecurityAttributes::owned_by(ActorId::System),
        shared(Subject::Group(other.id), Grant::Read),
    ];
    let id_of = |at: usize| Uuid::from_u128(at as u128 + 1);
    for (at, security_attributes) in made.iter().enumerate() {
        storage.insert(Document { id: id_of(at), name: None, removal_id: None, security_attributes: security_attributes.clone(), body: json!({}) }).await.unwrap();
    }
    let scanned = |admission: Admission| {
        let storage = &storage;
        async move { storage.scan(None, Some(&admission), Presence::Present, None, 100).await.unwrap().into_iter().map(|(id, _)| id).collect::<Vec<_>>() }
    };
    let reading = Admission { actors: vec![his], unowned: true, grant: Some(Grant::Read), roles: vec![role], everyone: true, deleted_groups: false };
    let at = |indexes: &[usize]| indexes.iter().map(|index| id_of(*index)).collect::<Vec<_>>();
    assert_eq!(scanned(reading.clone()).await, at(&[0, 1, 3, 4, 5, 8]), "his, shared with him, his role, everyone, his group by writing, the system's");
    assert_eq!(scanned(Admission { deleted_groups: true, ..reading.clone() }).await, at(&[0, 1, 3, 4, 5, 6, 8]), "a deleted group only where it is to count");
    assert_eq!(scanned(Admission { actors: vec![his], ..Admission::default() }).await, at(&[0]), "owning alone");
    assert!(scanned(Admission::default()).await.is_empty(), "nobody");

    // The groups themselves, filtered, as the teams of a viewer are asked
    // for: each read by its members through its own list, its parameters
    // numbered after the filter's.
    let teams = Filter::teams_of(his).by_path();
    let found: Vec<Uuid> = groups.scan(Some(&teams), Some(&reading), Presence::Present, None, 100).await.unwrap().into_iter().map(|(id, _)| id).collect();
    assert_eq!(found, [Uuid::from(team.id)], "his team, by its own list - not the one he is no member of, nor the deleted one");

    storage.replace(Document { id: id_of(1), name: None, removal_id: None, security_attributes: SecurityAttributes::owned_by(hers), body: json!({}) }).await.unwrap();
    assert_eq!(scanned(reading.clone()).await, at(&[0, 3, 4, 5, 8]), "an entry taken away goes with the write");
    storage.replace(Document { id: id_of(0), name: None, removal_id: Some(RemovalId::new()), security_attributes: SecurityAttributes::owned_by(his), body: json!({}) }).await.unwrap();
    assert!(storage.remove_deleted(id_of(0)).await.unwrap());
    let pool = database.connection.pool().await.unwrap();
    let left: i64 = sqlx::query_scalar(sql(format!("SELECT count(*) FROM {} WHERE entity_id = $1", quoted("entity_probe_access")))).bind(id_of(0)).fetch_one(pool).await.unwrap();
    assert_eq!(left, 0, "and its entries with a document removed");
    database.drop().await;
}

/// Whether `filter` compares names folded somewhere - what SQL cannot judge
/// and leaves to the store.
fn folded(filter: &Filter<FieldPath>) -> bool {
    match filter {
        Filter::NamePrefix(_, _) | Filter::Contains(_, _) => true,
        Filter::And(conditions) | Filter::Or(conditions) => conditions.iter().any(folded),
        Filter::Not(condition) | Filter::Any(_, condition) => folded(condition),
        Filter::Eq(_, _) | Filter::In(_, _) | Filter::IsNull(_) | Filter::AtLeast(_, _) | Filter::Below(_, _) => false,
    }
}

/// A scan hands out every document a filter meets, as `Filter::matches`
/// judges it - and exactly those, unless the filter compares names folded,
/// which the database leaves to the store.
#[tokio::test]
async fn a_scan_hands_out_what_a_filter_meets() {
    let Some(database) = Database::new() else { return };
    database.connection.pool().await.unwrap();
    let groups = Arc::new(PgDocuments::new(Arc::clone(&database.connection), Group::KIND, None).unwrap());
    let storage = PgDocuments::new(Arc::clone(&database.connection), EntityKind::new("probe"), Some(groups)).unwrap();
    let id = "7f1e3c1a-2d3b-4c5d-8e9f-0a1b2c3d4e5f";
    let bodies = [
        json!({ "a": "x" }), json!({ "a": ["x"] }), json!({ "a": { "User": "u1" } }), json!({ "a": { "User": "u2", "Extra": 1 } }),
        json!({ "a": "GlobalScope" }), json!({ "a": null }), json!({}), json!({ "a": true }), json!({ "a": { "b": "x" } }),
        json!({ "list": [{ "b": "x", "c": true }, { "b": "y" }] }), json!({ "list": "x" }), json!({ "list": ["x", "y"] }),
        json!({ "a": id }), json!({ "name": "Zoë Smith" }), json!({ "a": { "Session": "s" } }), json!({ "a": { "User": ["u1"] } }),
        json!({ "a": 3 }), json!({ "a": 7 }), json!({ "a": 4.5 }), json!({ "a": -1 }), json!({ "list": [{ "b": 2 }, { "b": 9 }] }),
    ];
    // The ids in the order of the bodies, as `Uuid` orders them.
    let id_of = |at: usize| Uuid::from_u128(at as u128);
    for (at, body) in bodies.iter().enumerate() {
        storage.insert(Document { id: id_of(at), name: None, removal_id: None, security_attributes: Default::default(), body: body.clone() }).await.unwrap();
    }
    const A: FieldPath = &["a"];
    const AB: FieldPath = &["a", "b"];
    const LIST: FieldPath = &["list"];
    const B: FieldPath = &["b"];
    const C: FieldPath = &["c"];
    const ITSELF: FieldPath = &[];
    const NAME: FieldPath = &["name"];
    let x = || Filter::eq(A, "x");
    let filters = [
        x(), Filter::eq(A, true), Filter::eq(A, id.parse::<Uuid>().unwrap()), Filter::eq(AB, "x"),
        Filter::eq(A, FieldValue::tagged("User", "u1")), Filter::eq(A, FieldValue::variant("User")), Filter::eq(A, FieldValue::variant("GlobalScope")),
        Filter::eq(A, FieldValue::tagged("User", FieldValue::variant("u1"))),
        Filter::is_in(A, ["x", "GlobalScope"]), Filter::is_in(A, Vec::<&str>::new()), Filter::IsNull(A), Filter::negated(Filter::IsNull(A)),
        Filter::negated(x()), Filter::negated(Filter::eq(A, FieldValue::variant("User"))),
        Filter::any(LIST, Filter::eq(B, "x")), Filter::any(LIST, Filter::all_of([Filter::eq(B, "x"), Filter::eq(C, true)])),
        Filter::any(LIST, Filter::all_of([Filter::eq(B, "y"), Filter::eq(C, true)])), Filter::any(LIST, Filter::eq(ITSELF, "x")),
        Filter::negated(Filter::any(LIST, Filter::eq(B, "x"))), Filter::any(LIST, Filter::negated(Filter::eq(B, "x"))),
        Filter::all_of([]), Filter::nothing(), Filter::all_of([x(), Filter::eq(A, "y")]), Filter::Or(vec![x(), Filter::IsNull(A)]),
        Filter::name_prefix(NAME, "zoe"), Filter::negated(Filter::name_prefix(NAME, "zoe")), Filter::all_of([Filter::name_prefix(NAME, "zoe"), x()]),
        Filter::contains(NAME, "smith"), Filter::any(LIST, Filter::name_prefix(B, "X")),
        Filter::negated(Filter::all_of([Filter::name_prefix(NAME, "zoe"), x()])), Filter::any(LIST, Filter::is_in(ITSELF, ["y", "z"])),
        Filter::within(A, 3..5), Filter::within(A, 4..u64::MAX), Filter::Below(A, 0), Filter::negated(Filter::within(A, 3..5)),
        Filter::any(LIST, Filter::within(B, 5..10)),
    ];
    for filter in filters {
        let scanned: Vec<Uuid> = storage.scan(Some(&filter), None, Presence::Present, None, 100).await.unwrap().into_iter().map(|(id, _)| id).collect();
        let meeting: Vec<Uuid> = bodies.iter().enumerate().filter(|(_, body)| filter.matches(body)).map(|(at, _)| id_of(at)).collect();
        assert!(meeting.iter().all(|id| scanned.contains(id)), "{filter:?}: meets {meeting:?}, scanned {scanned:?}");
        if !folded(&filter) {
            assert_eq!(scanned, meeting, "exactly what meets {filter:?}");
        }
    }
    database.drop().await;
}

/// A runtime built from its configuration keeps its entities and its states
/// in Postgres: the backends are made with the secrets at hand - over one
/// pool - and opened once the plugins registered, so that the tables of the
/// core's kinds are there before anything runs (issue 447).
#[tokio::test]
async fn a_runtime_keeps_its_entities_and_its_states_in_postgres() {
    let Some(database) = Database::new() else { return };
    let yaml = format!(
        "secrets:\n  providers:\n    - type: env\npersistence:\n  entity_store:\n    type: postgres\n    url_secret: {URL_SECRET}\n    schema: {}\n  state_store:\n    type: postgres\n    url_secret: {URL_SECRET}\n    schema: {}\n",
        database.connection.schema(), database.connection.schema(),
    );
    let config = AppConfig::from_yaml(&yaml).unwrap();
    let runtime = RuntimeBuilder::from_config(&config, &[&PostgresStorePlugin]).await.expect("opened against the test database").build().unwrap();
    let pool = database.connection.pool().await.unwrap();
    let kinds: Vec<String> = sqlx::query_scalar("SELECT name FROM kinds ORDER BY name").fetch_all(pool).await.unwrap();
    assert!(kinds.iter().any(|kind| kind == "group") && kinds.iter().any(|kind| kind == "session"), "the core's kinds, made at start: {kinds:?}");
    let owner = UserId::new();
    let team = Group::team(ScopedName::new("Ours", Scope::UserScope { user_id: owner }).unwrap(), owner);
    runtime.entity_stores().groups.create(&SecurityContext::system(), &team).await.unwrap();
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM entity_group").fetch_one(pool).await.unwrap();
    assert_eq!(kept, 1, "in its table");
    let agent = AgentId::new();
    runtime.state_store().put_agent_state(agent, &mut AgentState::default()).await.unwrap();
    let states: i64 = sqlx::query_scalar("SELECT count(*) FROM agent_state").fetch_one(pool).await.unwrap();
    assert_eq!(states, 1, "the states in theirs");
    database.drop().await;
}
