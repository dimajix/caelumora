//! The Postgres store against a database: the one `test/docker-compose.yml`
//! starts, named by the variable `CAELUMORA_TEST_POSTGRES_URL` - each test in
//! a schema of its own, dropped when it is done. Without the variable the
//! tests that need a database are skipped, and say so.

use super::*;
use crate::connection::{quoted, sql};
use caelumora_model::config::BackendConfig;
use caelumora_model::confidentiality::ConfidentialityRules;
use caelumora_model::group::Group;
use caelumora_model::scope::Scope;
use caelumora_model::secret::env::EnvSecretProvider;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{AccessPolicy, AllowAllPolicy, OwnershipPolicy, SecurityContext};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::store::suite;
use caelumora_model::store::{EntityStoreFactory, Persistence};
use caelumora_model::user::UserId;
use caelumora_model::value::name::ScopedName;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::runtime::Services;
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
/// store, and keeps the entities alone.
#[test]
fn the_entities_are_kept_in_postgres_and_the_states_not() {
    let backends = Backends::with_plugins(&[&PostgresStorePlugin]);
    let services = Services::empty().with(secrets());
    let postgres = BackendConfig::new("postgres", json!({ "url_secret": URL_SECRET, "schema": "caelumora", "max_connections": 2 }));
    let made = backends.make_one::<dyn PersistenceBackend>("entity store", "entity_store", &postgres, &services).unwrap();
    let alone = Persistence::new(Arc::clone(&made), made, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default())).map(|_| ()).expect_err("keeps no states");
    assert!(alone.to_string().contains("keeps no states"), "{alone}");

    let misspelt = BackendConfig::new("postgres", json!({ "url": "postgres://somewhere" }));
    let refused = backends.make_one::<dyn PersistenceBackend>("entity store", "entity_store", &misspelt, &services).map(|_| ()).unwrap_err().to_string();
    assert!(refused.contains("the entity store (postgres)") && refused.contains("url"), "{refused}");
}
