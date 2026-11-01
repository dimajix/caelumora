//! How the Postgres store is joined to the kernel (issue 03): as a backend of the stores alone.

use crate::connection::Connections;
use crate::{PostgresStore, PostgresStoreConfig};
use anyhow::Result;
use caelumora_api::component::service_of;
use caelumora_api::component::Params;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_security::secret::SecretManager;
use caelumora_store::PersistenceBackend;
use std::sync::Arc;

/// The backend `postgres` of the stores (`PostgresStore`), as a plugin. Its backends share a pool
/// per secret and schema, among the backends of one kernel.
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

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}
