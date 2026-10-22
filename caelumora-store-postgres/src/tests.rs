//! The Postgres store against a database: the one `test/docker-compose.yml`
//! starts, named by the variable `CAELUMORA_TEST_POSTGRES_URL` - each test in
//! a schema of its own, dropped when it is done. Without the variable the
//! tests that need a database are skipped, and say so.

use super::*;
use crate::connection::{quoted, sql};
use crate::plugin::PostgresStorePlugin;
use caelumora_model::config::{AppConfig, BackendConfig};
use caelumora_model::confidentiality::ConfidentialityRules;
use caelumora_model::group::Group;
use caelumora_model::scope::Scope;
use caelumora_model::secret::env::EnvSecretProvider;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{AccessPolicy, AllowAllPolicy, OwnershipPolicy, SecurityContext};
use caelumora_model::store::documents::Document;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::query::{FieldPath, FieldValue, Filter};
use caelumora_model::store::suite;
use caelumora_model::store::{EntityStoreFactory, Persistence};
use caelumora_model::user::UserId;
use caelumora_model::value::name::ScopedName;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::runtime::{RuntimeBuilder, Services};
use serde_json::json;
use uuid::Uuid;

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
        if std::env::var(URL_SECRET).is_err() {
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
async fn behaves_as_a_component_store() {
    let Some(database) = Database::new() else { return };
    suite::component_store_behaves(database.stores(Arc::new(AllowAllPolicy::new())).await.component_store().unwrap().as_ref()).await;
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
/// store, and keeps the entities and the states, but no histories yet.
#[test]
fn the_backend_is_made_from_its_parameters() {
    let backends = Backends::with_plugins(&[&PostgresStorePlugin]);
    let services = Services::empty().with(secrets());
    let postgres = BackendConfig::new("postgres", json!({ "url_secret": URL_SECRET, "schema": "caelumora", "max_connections": 2 }));
    let made = backends.make_one::<dyn PersistenceBackend>("entity store", "entity_store", &postgres, &services).unwrap();
    let alone = Persistence::new(Arc::clone(&made), made, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).map(|_| ()).expect_err("keeps no histories");
    assert!(alone.to_string().contains("keeps no histories"), "{alone}");

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
async fn keeps_the_transcripts() {
    let Some(database) = Database::new() else { return };
    let store = database.backend();
    suite::transcript_store_behaves(store.as_ref(), store.as_ref()).await;
    suite::given_store_behaves(store.as_ref(), store.as_ref()).await;
    database.drop().await;
}

/// Whether `filter` compares names folded somewhere - what SQL cannot judge
/// and leaves to the store.
fn folded(filter: &Filter<FieldPath>) -> bool {
    match filter {
        Filter::NamePrefix(_, _) | Filter::Contains(_, _) => true,
        Filter::And(conditions) | Filter::Or(conditions) => conditions.iter().any(folded),
        Filter::Not(condition) | Filter::Any(_, condition) => folded(condition),
        Filter::Eq(_, _) | Filter::In(_, _) | Filter::IsNull(_) => false,
    }
}

/// A scan hands out every document a filter meets, as `Filter::matches`
/// judges it - and exactly those, unless the filter compares names folded,
/// which the database leaves to the store.
#[tokio::test]
async fn a_scan_hands_out_what_a_filter_meets() {
    let Some(database) = Database::new() else { return };
    database.connection.pool().await.unwrap();
    let storage = PgDocuments::new(Arc::clone(&database.connection), EntityKind::new("probe")).unwrap();
    let id = "7f1e3c1a-2d3b-4c5d-8e9f-0a1b2c3d4e5f";
    let bodies = [
        json!({ "a": "x" }), json!({ "a": ["x"] }), json!({ "a": { "User": "u1" } }), json!({ "a": { "User": "u2", "Extra": 1 } }),
        json!({ "a": "GlobalScope" }), json!({ "a": null }), json!({}), json!({ "a": true }), json!({ "a": { "b": "x" } }),
        json!({ "list": [{ "b": "x", "c": true }, { "b": "y" }] }), json!({ "list": "x" }), json!({ "list": ["x", "y"] }),
        json!({ "a": id }), json!({ "name": "Zoë Smith" }), json!({ "a": { "Session": "s" } }), json!({ "a": { "User": ["u1"] } }),
    ];
    for (at, body) in bodies.iter().enumerate() {
        storage.insert(Document { id: format!("{at:02}"), name: None, deleted: false, body: body.clone() }).await.unwrap();
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
    ];
    for filter in filters {
        let scanned: Vec<String> = storage.scan(Some(&filter), false, None, 100).await.unwrap().into_iter().map(|(id, _)| id).collect();
        let meeting: Vec<String> = bodies.iter().enumerate().filter(|(_, body)| filter.matches(body)).map(|(at, _)| format!("{at:02}")).collect();
        assert!(meeting.iter().all(|id| scanned.contains(id)), "{filter:?}: meets {meeting:?}, scanned {scanned:?}");
        if !folded(&filter) {
            assert_eq!(scanned, meeting, "exactly what meets {filter:?}");
        }
    }
    database.drop().await;
}

/// A runtime built from its configuration keeps its entities in Postgres:
/// the backend is made with the secrets at hand and opened once the plugins
/// registered, so that the tables of the core's kinds are there before
/// anything runs (issue 447).
#[tokio::test]
async fn a_runtime_keeps_its_entities_in_postgres() {
    let Some(database) = Database::new() else { return };
    let yaml = format!(
        "secrets:\n  providers:\n    - type: env\npersistence:\n  entity_store:\n    type: postgres\n    url_secret: {URL_SECRET}\n    schema: {}\n  state_store:\n    type: memory\n",
        database.connection.schema(),
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
    database.drop().await;
}
