//! The documents of one kind in its table.

use crate::access::{entries, Admitted};
use crate::connection::{quoted, sql, Connection};
use crate::filter::Condition;
use async_trait::async_trait;
use caelumora_model::store::documents::{Document, DocumentStorage};
use caelumora_model::security::Admission;
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
const KIND_TABLE_VERSION: i32 = 3;

/// The number of the first parameter of a scan after its own three - the
/// mark, the cursor and the limit -, which the filter and the admission
/// bind from.
const FIRST_SCANNED: usize = 4;

/// The longest identifier Postgres keeps; a longer one would be cut short,
/// and two kinds could meet in one table.
const MAX_IDENTIFIER: usize = 63;

/// The documents of one kind in its table (issue 03), `entity_<kind>`: the
/// id, a `uuid` - ordered bytewise, as `Uuid` orders it -, the key of the name
/// (`ScopedName::key`), whether it is marked deleted, and the body. A name is
/// kept unique among what is not deleted by a partial unique index, the body
/// searched by a GIN index.
///
/// Beside it, the access entries of its documents (issue 332),
/// `entity_<kind>_access`: a row `(subject, relation, entity_id)` for the
/// owner and each entry of the access list, written in the transaction that
/// writes the document and removed with it, which a list is narrowed by -
/// joined, for an entry naming a group, with the groups an actor belongs to.
#[derive(Debug)]
pub(crate) struct PgDocuments {
    connection: Arc<Connection>,
    kind: EntityKind,
    /// The table, quoted.
    table: String,
    /// The table of the access entries, quoted.
    access: String,
    /// The index that keeps names unique, as an error names it.
    name_index: String,
    /// The storage of the groups, whose members an entry for a group
    /// admits; `None` where these documents are taken for them, as the
    /// groups' own are.
    groups: Option<Arc<PgDocuments>>,
    /// Whether the tables are there, made if they were not - once.
    ready: OnceCell<()>,
}

impl PgDocuments {
    /// The documents of `kind` over `connection`, whose tables are made when
    /// they are first used; a list narrowed by the groups of `groups`, or by
    /// these documents themselves if `None`, as for the groups.
    ///
    /// # Errors
    /// `StoreError::Backend` if the name of the kind is too long for a
    /// table.
    pub(crate) fn new(connection: Arc<Connection>, kind: EntityKind, groups: Option<Arc<PgDocuments>>) -> StoreResult<Self> {
        let table = format!("entity_{kind}");
        let name_index = format!("{table}_name");
        if format!("{table}_access_entity").len() > MAX_IDENTIFIER {
            return Err(StoreError::Backend(format!("the kind {kind} has too long a name for a table")));
        }
        let access = quoted(&format!("{table}_access"));
        Ok(Self { connection, kind, table: quoted(&table), access, name_index, groups, ready: OnceCell::new() })
    }

    /// The storage of the groups.
    fn groups(&self) -> &PgDocuments {
        self.groups.as_deref().unwrap_or(self)
    }

    /// The pool, with the tables there.
    ///
    /// # Errors
    /// If the pool cannot be made, or the tables not - or they are of a form
    /// this program does not know.
    pub(crate) async fn ready(&self) -> StoreResult<&PgPool> {
        let pool = self.connection.pool().await?;
        self.ready.get_or_try_init(|| self.make_table(pool)).await?;
        Ok(pool)
    }

    /// Makes the tables unless `kinds` records them, under a lock of the kind,
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
            format!("CREATE TABLE {} (subject uuid NOT NULL, relation text NOT NULL, entity_id uuid NOT NULL REFERENCES {table} (id) ON DELETE CASCADE, \
                PRIMARY KEY (subject, relation, entity_id))", self.access),
            format!("CREATE INDEX {} ON {} (entity_id)", quoted(&format!("entity_{}_access_entity", self.kind)), self.access),
        ];
        for statement in statements {
            sqlx::query(sql(statement)).execute(&mut **tx).await.map_err(|e| self.failed(e))?;
        }
        sqlx::query("INSERT INTO kinds (name, table_name, schema_version) VALUES ($1, $2, $3)")
            .bind(self.kind.name()).bind(format!("entity_{}", self.kind)).bind(KIND_TABLE_VERSION)
            .execute(&mut **tx).await.map_err(|e| self.failed(e))?;
        Ok(())
    }

    /// Writes `document` by `statement`, which binds its id, the key of its
    /// name, its mark and its body, and its access entries in place of those
    /// it had - in one transaction.
    async fn write(&self, statement: String, document: Document) -> StoreResult<()> {
        let pool = self.ready().await?;
        let mut tx = pool.begin().await.map_err(|e| self.failed(e))?;
        sqlx::query(sql(statement))
            .bind(document.id).bind(document.name.as_ref().map(ScopedName::key)).bind(document.deleted).bind(&document.body)
            .execute(&mut *tx).await.map_err(|e| self.refused(e, document.id))?;
        sqlx::query(sql(format!("DELETE FROM {} WHERE entity_id = $1", self.access))).bind(document.id).execute(&mut *tx).await.map_err(|e| self.failed(e))?;
        let (subjects, relations) = entries(&document.security_attributes);
        sqlx::query(sql(format!("INSERT INTO {} (subject, relation, entity_id) SELECT subject, relation, $3 FROM UNNEST($1::uuid[], $2::text[]) AS entry(subject, relation) \
            ON CONFLICT DO NOTHING", self.access)))
            .bind(subjects).bind(relations).bind(document.id)
            .execute(&mut *tx).await.map_err(|e| self.failed(e))?;
        tx.commit().await.map_err(|e| self.failed(e))
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
    /// (`Condition`), the rest by the store; the admission by the access
    /// entries (`Admitted`).
    async fn scan(&self, filter: Option<&Filter<FieldPath>>, admission: Option<&Admission>, deleted: bool, after: Option<Uuid>, limit: usize) -> StoreResult<Vec<(Uuid, Value)>> {
        let pool = self.ready().await?;
        let groups = self.groups();
        if admission.is_some() {
            groups.ready().await?;
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let condition = filter.map(|filter| Condition::of(filter, "body", FIRST_SCANNED));
        let meeting = condition.as_ref().map_or("true", |condition| condition.sql.as_str());
        let first = condition.as_ref().map_or(FIRST_SCANNED, Condition::next);
        let admitted = admission.map(|admission| Admitted::of(admission, &self.access, &groups.table, first));
        let admitting = admitted.as_ref().map_or("true", |admitted| admitted.sql.as_str());
        let statement = format!("SELECT id, body FROM {} WHERE deleted = $1 AND ($2::uuid IS NULL OR id > $2) AND {meeting} AND {admitting} ORDER BY id LIMIT $3", self.table);
        let query = sqlx::query_as(sql(statement)).bind(deleted).bind(after).bind(limit);
        let query = match &condition {
            Some(condition) => condition.bound(query),
            None => query,
        };
        let query = match &admitted {
            Some(admitted) => admitted.bound(query),
            None => query,
        };
        query.fetch_all(pool).await.map_err(|e| self.failed(e))
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        let pool = self.ready().await?;
        sqlx::query_scalar(sql(format!("SELECT body FROM {} WHERE name_key = $1 AND NOT deleted", self.table)))
            .bind(name.key()).fetch_optional(pool).await.map_err(|e| self.failed(e))
    }

    /// A taken id is answered before a taken name because Postgres checks the unique indexes in
    /// the order they were made, the primary key's with the table first - asked by the store
    /// suite (`lifecycle!`), since `seed::create_unless_taken` relies on it.
    async fn insert(&self, document: Document) -> StoreResult<()> {
        self.write(format!("INSERT INTO {} (id, name_key, deleted, body) VALUES ($1, $2, $3, $4)", self.table), document).await
    }

    async fn replace(&self, document: Document) -> StoreResult<()> {
        let upsert = format!("INSERT INTO {} (id, name_key, deleted, body) VALUES ($1, $2, $3, $4) \
            ON CONFLICT (id) DO UPDATE SET name_key = EXCLUDED.name_key, deleted = EXCLUDED.deleted, body = EXCLUDED.body", self.table);
        self.write(upsert, document).await
    }

    /// Its access entries go with it.
    async fn remove_deleted(&self, id: Uuid) -> StoreResult<bool> {
        let pool = self.ready().await?;
        let done = sqlx::query(sql(format!("DELETE FROM {} WHERE id = $1 AND deleted", self.table))).bind(id).execute(pool).await.map_err(|e| self.failed(e))?;
        Ok(done.rows_affected() > 0)
    }
}
