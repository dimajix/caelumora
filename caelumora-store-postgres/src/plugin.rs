//! How the Postgres store is joined to the runtime (issue 03): as a backend
//! of the stores alone.

use crate::connection::Connections;
use crate::{PostgresStore, PostgresStoreConfig};
use anyhow::Result;
use caelumora_model::secret::SecretManager;
use caelumora_model::store::PersistenceBackend;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::component::service_of;
use caelumora_runtime::component::Params;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use std::sync::Arc;

/// The backend `postgres` of the stores (`PostgresStore`), as a plugin. Its
/// backends share a pool per secret and schema, among the backends of one
/// runtime.
#[derive(Debug, Default)]
pub struct PostgresStorePlugin;

impl Params for PostgresStoreConfig {}

impl Plugin for PostgresStorePlugin {
    fn name(&self) -> &'static str {
        "store_postgres"
    }

    fn backends(&self, backends: &mut Backends) {
        let connections = Arc::new(Connections::default());
        backends.register::<dyn PersistenceBackend, _, _>(
            "postgres",
            move |_, config: PostgresStoreConfig, services| {
                let secrets = service_of::<SecretManager>(services)?;
                Ok(Arc::new(PostgresStore::new(connections.of(&config, secrets))))
            },
        );
    }

    fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
        Ok(())
    }
}
