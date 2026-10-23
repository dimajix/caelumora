//! The documents of one kind in its table.

use crate::connection::{quoted, sql, Connection};
use crate::filter::Condition;
use async_trait::async_trait;
use caelumora_model::store::documents::{Document, DocumentStorage};
use caelumora_model::store::query::{FieldPath, Filter};
use caelumora_model::store::{EntityKind, StoreError, StoreResult};
use caelumora_model::value::name::ScopedName;
use serde_json::Value;
use sqlx::postgres::PgPool;
use sqlx::{Postgres, Transaction};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::info;
use uuid::Uuid;

/// The form of the tables of the kinds this program makes and reads, as
/// `kinds.schema_version` records it: a change of the form that all of them
/// share counts it up, and a table of an older form is refused - it is
/// deleted, as stored data that is no longer read is.
const KIND_TABLE_VERSION: i32 = 2;

/// The longest identifier Postgres keeps; a longer one would be cut short,
/// and two kinds could meet in one table.
const MAX_IDENTIFIER: usize = 63;

/// The documents of one kind in its table (issue 03), `entity_<kind>`: the
/// id, a `uuid` - ordered bytewise, as `Uuid` orders it -, the key of the name
/// (`ScopedName::key`), whether it is marked deleted, and the body. A name is
/// kept unique among what is not deleted by a partial unique index, the body
/// searched by a GIN index.
#[derive(Debug)]
pub(crate) struct PgDocuments {
    connection: Arc<Connection>,
    kind: EntityKind,
    /// The table, quoted.
    table: String,
    /// The index that keeps names unique, as an error names it.
    name_index: String,
    /// Whether the table is there, made if it was not - once.
    ready: OnceCell<()>,
}

impl PgDocuments {
    /// The documents of `kind` over `connection`, whose table is made when
    /// it is first used.
    ///
    /// # Errors
    /// `StoreError::Backend` if the name of the kind is too long for a
    /// table.
    pub(crate) fn new(connection: Arc<Connection>, kind: EntityKind) -> StoreResult<Self> {
        let table = format!("entity_{kind}");
        let name_index = format!("{table}_name");
        if format!("{table}_body").len() > MAX_IDENTIFIER {
            return Err(StoreError::Backend(format!("the kind {kind} has too long a name for a table")));
        }
        Ok(Self { connection, kind, table: quoted(&table), name_index, ready: OnceCell::new() })
    }

    /// The pool, with the table there.
    ///
    /// # Errors
    /// If the pool cannot be made, or the table not - or it is of a form
    /// this program does not know.
    pub(crate) async fn ready(&self) -> StoreResult<&PgPool> {
        let pool = self.connection.pool().await?;
        self.ready.get_or_try_init(|| self.make_table(pool)).await?;
        Ok(pool)
    }

    /// Makes the table unless `kinds` records it, under a lock of the kind,
    /// so that two processes that open it at once make it once.
    async fn make_table(&self, pool: &PgPool) -> StoreResult<()> {
        let kind = self.kind.name();
        let mut tx = pool.begin().await.map_err(|e| self.failed(e))?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('caelumora.kind.' || $1))").bind(kind).execute(&mut *tx).await.map_err(|e| self.failed(e))?;
        let version: Option<i32> = sqlx::query_scalar("SELECT schema_version FROM kinds WHERE name = $1").bind(kind).fetch_optional(&mut *tx).await.map_err(|e| self.failed(e))?;
        match version {
            Some(KIND_TABLE_VERSION) => return Ok(()),
            Some(other) => return Err(StoreError::Backend(format!("the table of the kind {kind} has the form {other}, this program knows {KIND_TABLE_VERSION}"))),
            None => self.create_table(&mut tx).await?,
        }
        tx.commit().await.map_err(|e| self.failed(e))?;
        info!("Made the table of the kind {kind}");
        Ok(())
    }

    async fn create_table(&self, tx: &mut Transaction<'_, Postgres>) -> StoreResult<()> {
        let table = &self.table;
        let statements = [
            format!("CREATE TABLE {table} (id uuid PRIMARY KEY, name_key text, deleted boolean NOT NULL, body jsonb NOT NULL)"),
            format!("CREATE UNIQUE INDEX {} ON {table} (name_key) WHERE NOT deleted", quoted(&self.name_index)),
            format!("CREATE INDEX {} ON {table} USING gin (body jsonb_path_ops)", quoted(&format!("entity_{}_body", self.kind))),
        ];
        for statement in statements {
            sqlx::query(sql(statement)).execute(&mut **tx).await.map_err(|e| self.failed(e))?;
        }
        sqlx::query("INSERT INTO kinds (name, table_name, schema_version) VALUES ($1, $2, $3)")
            .bind(self.kind.name()).bind(format!("entity_{}", self.kind)).bind(KIND_TABLE_VERSION)
            .execute(&mut **tx).await.map_err(|e| self.failed(e))?;
        Ok(())
    }

    /// `err` as the failure of the storage of the kind.
    fn failed(&self, err: sqlx::Error) -> StoreError {
        StoreError::Backend(format!("the table of the kind {}: {err}", self.kind))
    }

    /// `err` of a write of the document `id`: a name taken if the index of
    /// the names refused it, a conflict if the id is taken.
    fn refused(&self, err: sqlx::Error, id: Uuid) -> StoreError {
        if let sqlx::Error::Database(database) = &err {
            if database.is_unique_violation() {
                return match database.constraint() == Some(self.name_index.as_str()) {
                    true => StoreError::NameTaken { entity: self.kind.name() },
                    false => StoreError::Conflict(format!("{} {id} already exists", self.kind)),
                };
            }
        }
        self.failed(err)
    }
}

#[async_trait]
impl DocumentStorage for PgDocuments {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        let pool = self.ready().await?;
        sqlx::query_scalar(sql(format!("SELECT body FROM {} WHERE id = $1", self.table))).bind(id).fetch_optional(pool).await.map_err(|e| self.failed(e))
    }

    /// What of `filter` SQL can judge is judged by the database
    /// (`Condition`), the rest by the store.
    async fn scan(&self, filter: Option<&Filter<FieldPath>>, deleted: bool, after: Option<Uuid>, limit: usize) -> StoreResult<Vec<(Uuid, Value)>> {
        let pool = self.ready().await?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let condition = filter.map(|filter| Condition::of(filter, "body", 4));
        let meeting = condition.as_ref().map_or("true", |condition| condition.sql.as_str());
        let statement = format!("SELECT id, body FROM {} WHERE deleted = $1 AND ($2::uuid IS NULL OR id > $2) AND {meeting} ORDER BY id LIMIT $3", self.table);
        let query = sqlx::query_as(sql(statement)).bind(deleted).bind(after).bind(limit);
        let query = match condition {
            Some(condition) => condition.bound(query),
            None => query,
        };
        query.fetch_all(pool).await.map_err(|e| self.failed(e))
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        let pool = self.ready().await?;
        sqlx::query_scalar(sql(format!("SELECT body FROM {} WHERE name_key = $1 AND NOT deleted", self.table)))
            .bind(name.key()).fetch_optional(pool).await.map_err(|e| self.failed(e))
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        let pool = self.ready().await?;
        sqlx::query(sql(format!("INSERT INTO {} (id, name_key, deleted, body) VALUES ($1, $2, $3, $4)", self.table)))
            .bind(document.id).bind(document.name.as_ref().map(ScopedName::key)).bind(document.deleted).bind(&document.body)
            .execute(pool).await.map_err(|e| self.refused(e, document.id))?;
        Ok(())
    }

    async fn replace(&self, document: Document) -> StoreResult<()> {
        let pool = self.ready().await?;
        let upsert = format!("INSERT INTO {} (id, name_key, deleted, body) VALUES ($1, $2, $3, $4) \
            ON CONFLICT (id) DO UPDATE SET name_key = EXCLUDED.name_key, deleted = EXCLUDED.deleted, body = EXCLUDED.body", self.table);
        sqlx::query(sql(upsert))
            .bind(document.id).bind(document.name.as_ref().map(ScopedName::key)).bind(document.deleted).bind(&document.body)
            .execute(pool).await.map_err(|e| self.refused(e, document.id))?;
        Ok(())
    }

    async fn remove_deleted(&self, id: Uuid) -> StoreResult<bool> {
        let pool = self.ready().await?;
        let done = sqlx::query(sql(format!("DELETE FROM {} WHERE id = $1 AND deleted", self.table))).bind(id).execute(pool).await.map_err(|e| self.failed(e))?;
        Ok(done.rows_affected() > 0)
    }
}
