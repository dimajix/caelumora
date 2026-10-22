//! The store of the entities in Postgres (issue 03): a backend of the
//! stores (`PersistenceBackend`) that keeps the documents of every kind in a
//! table of its own, `entity_<kind>`, and no states (issue 04). What is
//! asked and in which order is the model's, as for every backend
//! (`CheckedStore`); the database keeps what is kept unique - an id, a name
//! in its scope - by its indexes, so that it holds against every process
//! that writes it.
//!
//! The connection is named by a secret (`PostgresStoreConfig::url_secret`),
//! resolved when the backend is opened at start; then the pool is made, the
//! crate's migrations are applied (`migrations/`) and the tables of the kinds
//! opened are made. A kind opened later gets its table when it is first
//! used.

mod connection;
mod documents;
mod filter;
mod plugin;

pub use plugin::PostgresStorePlugin;

use crate::connection::Connection;
use crate::documents::PgDocuments;
use async_trait::async_trait;
use caelumora_model::store::documents::{DocumentOf, DocumentStorage};
use caelumora_model::store::history::HistoryStore;
use caelumora_model::store::transcript::TranscriptStore;
use caelumora_model::store::{EntityKind, PersistenceBackend, StateStore, StoreError, StoreResult};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

/// The parameters of the backend `postgres`: the name of the secret that
/// holds the URL of the database (`postgres://user:password@host/db`), the
/// schema its tables are kept in - `public` unless named -, and how many
/// connections its pool opens at most - 10 unless named. The pool is shared
/// by every backend named by the same secret and schema; the first one made
/// says how large it is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresStoreConfig {
    pub url_secret: String,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub max_connections: Option<u32>,
}

/// The schema of a configuration that names none.
const DEFAULT_SCHEMA: &str = "public";

/// The size of the pool of a configuration that names none.
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// The entities in Postgres: the storage of each kind asked for, over one
/// connection.
#[derive(Debug)]
pub struct PostgresStore {
    connection: Arc<Connection>,
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<PgDocuments>>>,
}

impl PostgresStore {
    fn new(connection: Arc<Connection>) -> Self {
        Self { connection, documents: Default::default() }
    }

    /// Why it refuses `what` of the states.
    fn keeps_no(what: &str) -> StoreError {
        StoreError::Backend(format!("the Postgres store keeps no {what}"))
    }
}

/// Opens a kind in its table, `entity_<kind>`; keeps no states.
#[async_trait]
impl PersistenceBackend for PostgresStore {
    fn documents(&self, kind: EntityKind, _document_of: DocumentOf) -> StoreResult<Arc<dyn DocumentStorage>> {
        let mut all = self.documents.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(storage) = all.get(&kind) {
            return Ok(Arc::<PgDocuments>::clone(storage));
        }
        let storage = Arc::new(PgDocuments::new(Arc::clone(&self.connection), kind)?);
        all.insert(kind, Arc::clone(&storage));
        Ok(storage)
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Err(Self::keeps_no("states"))
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Err(Self::keeps_no("histories"))
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Err(Self::keeps_no("transcripts"))
    }

    /// Connects, applies the migrations and makes the tables of the kinds
    /// opened, so that a wrong secret or a database out of reach fails the
    /// start.
    async fn open(&self) -> StoreResult<()> {
        self.connection.pool().await?;
        let opened: Vec<Arc<PgDocuments>> = self.documents.read().unwrap_or_else(PoisonError::into_inner).values().cloned().collect();
        for documents in opened {
            documents.ready().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
