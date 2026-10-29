//! The connection to the database: a pool per secret and schema, made when
//! it is first needed.

use crate::PostgresStoreConfig;
use crate::{DEFAULT_MAX_CONNECTIONS, DEFAULT_SCHEMA};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{StoreError, StoreResult};
use sqlx::migrate::Migrator;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, Postgres, Transaction};
use std::collections::HashMap;
use std::fmt::{self, Debug};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::OnceCell;
use tracing::info;

/// The migrations of the crate's own tables, `kinds` among them; the tables
/// of the kinds are made in code (`PgDocuments`).
static MIGRATOR: Migrator = sqlx::migrate!();

/// A connection to a database, as a secret names it, in one schema: the
/// pool, made, and the migrations applied, when it is first asked for - at
/// start, when the backends are opened. Its connections find the tables of
/// the schema by their names alone (`search_path`).
pub(crate) struct Connection {
    url_secret: String,
    schema: String,
    max_connections: u32,
    secrets: Arc<SecretManager>,
    pool: OnceCell<PgPool>,
}

impl Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("url_secret", &self.url_secret)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// The connection the secret `url_secret` names, in `schema`, of at
    /// most `max_connections`, resolved by `secrets`.
    pub(crate) fn new(
        url_secret: &str,
        schema: &str,
        max_connections: u32,
        secrets: Arc<SecretManager>,
    ) -> Self {
        Self {
            url_secret: url_secret.to_string(),
            schema: schema.to_string(),
            max_connections,
            secrets,
            pool: OnceCell::new(),
        }
    }

    /// The pool, made if it is not yet.
    ///
    /// # Errors
    /// `StoreError::Backend` if the secret cannot be resolved or holds no
    /// URL, the database cannot be reached or its migrations not applied.
    pub(crate) async fn pool(&self) -> StoreResult<&PgPool> {
        self.pool.get_or_try_init(|| self.connect()).await
    }

    /// A connection of the pool, for what reads with several queries.
    ///
    /// # Errors
    /// As `pool`, or if the pool has none to give.
    pub(crate) async fn acquire(&self) -> StoreResult<PoolConnection<Postgres>> {
        self.pool().await?.acquire().await.map_err(|e| {
            StoreError::Backend(format!("no connection of the schema '{}': {e}", self.schema))
        })
    }

    /// A transaction, for what a put writes.
    ///
    /// # Errors
    /// As `pool`, or if the transaction cannot be begun.
    pub(crate) async fn begin(&self) -> StoreResult<Transaction<'static, Postgres>> {
        self.pool().await?.begin().await.map_err(|e| {
            StoreError::Backend(format!("no transaction in the schema '{}': {e}", self.schema))
        })
    }

    /// The schema its tables are kept in.
    #[cfg(test)]
    pub(crate) fn schema(&self) -> &str {
        &self.schema
    }

    /// Resolves the secret, connects, makes the schema and applies the
    /// migrations. No message names the URL: it holds the password.
    async fn connect(&self) -> StoreResult<PgPool> {
        let named = &self.url_secret;
        let url = self.secrets.resolve(&SecurityContext::system(), named).await.map_err(|e| {
            StoreError::Backend(format!(
                "the URL of the database cannot be resolved from the secret '{named}': {e}"
            ))
        })?;
        let options = PgConnectOptions::from_str(&url)
            .map_err(|_| {
                StoreError::Backend(format!("the secret '{named}' holds no URL of a database"))
            })?
            .options([("search_path", self.schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(self.max_connections)
            .connect_with(options)
            .await
            .map_err(|e| {
                StoreError::Backend(format!(
                    "the database of the secret '{named}' cannot be reached: {e}"
                ))
            })?;
        sqlx::query(sql(format!("CREATE SCHEMA IF NOT EXISTS {}", quoted(&self.schema))))
            .execute(&pool)
            .await
            .map_err(|e| {
                StoreError::Backend(format!("the schema '{}' cannot be made: {e}", self.schema))
            })?;
        MIGRATOR.run(&pool).await.map_err(|e| {
            StoreError::Backend(format!(
                "the migrations of the schema '{}' cannot be applied: {e}",
                self.schema
            ))
        })?;
        info!("Connected to the database of the secret '{named}', schema '{}'", self.schema);
        Ok(pool)
    }
}

/// The connections of the backends of the plugin, one per secret and
/// schema, shared by every backend named by them - the stores of the
/// entities and, later, of the states and the users (issues 04, 26).
#[derive(Debug, Default)]
pub(crate) struct Connections {
    made: Mutex<HashMap<(String, String), Arc<Connection>>>,
}

impl Connections {
    /// The connection `config` names, made if it is not yet.
    pub(crate) fn of(
        &self,
        config: &PostgresStoreConfig,
        secrets: &Arc<SecretManager>,
    ) -> Arc<Connection> {
        let schema = config.schema.as_deref().unwrap_or(DEFAULT_SCHEMA);
        let mut made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
        let connection =
            made.entry((config.url_secret.clone(), schema.to_string())).or_insert_with(|| {
                let max_connections = config.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS);
                Arc::new(Connection::new(
                    &config.url_secret,
                    schema,
                    max_connections,
                    Arc::clone(secrets),
                ))
            });
        Arc::clone(connection)
    }
}

/// `name` as an identifier of SQL, quoted: a schema, a table, an index.
pub(crate) fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `statement` as a query is made of it: one built of SQL and of
/// identifiers quoted by `quoted` alone, every value bound as a parameter -
/// which is what was audited.
pub(crate) fn sql(statement: String) -> AssertSqlSafe<String> {
    AssertSqlSafe(statement)
}
