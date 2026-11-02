//! How the JSON store is joined to the kernel (issue 446): as a backend of the stores alone.

use crate::JsonStoreConfig;
use anyhow::Result;
use caelumora_api::component::ComponentError;
use caelumora_api::component::Params;
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_store::PersistenceBackend;
use std::sync::Arc;

/// The backend `json` of the stores (`JsonFileStore`), as a plugin.
#[derive(Debug, Default)]
pub struct JsonStorePlugin;

impl Params for JsonStoreConfig {}

impl Plugin for JsonStorePlugin {
    fn name(&self) -> &'static str {
        "store_json"
    }

    fn backends(&self, backends: &mut Backends) {
        backends.register::<dyn PersistenceBackend, _, _>(
            "json",
            |_, config: JsonStoreConfig, _| {
                let store =
                    config.store().map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
                Ok(Arc::new(store))
            },
        );
    }

    fn register(&self, _kernel: &mut KernelBuilder) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_api::component::Services;
    use caelumora_domain::config::BackendConfig;
    use caelumora_domain::user::User;
    use caelumora_security::Policies;
    use caelumora_store::Persistence;
    use serde_json::json;

    /// The stores keep their data in JSON files once the plugin is there,
    /// made from the parameters and refused naming the store.
    #[test]
    fn the_stores_keep_their_data_in_json_files() {
        let backends = Backends::with_plugins(&[&JsonStorePlugin]);
        let dir = tempfile::tempdir().unwrap();
        let json = BackendConfig::new("json", json!({ "path": dir.path(), "journal_floor": 1024 }));
        let files = backends
            .make_one::<dyn PersistenceBackend>(
                "entity store",
                "entity_store",
                &json,
                &Services::empty(),
            )
            .unwrap();
        let stores = Persistence::over(files).expect("its states, too");
        stores
            .entity_store::<User>(&Policies::permitting_all())
            .expect("its kinds are opened in its files");
        assert!(dir.path().join("entities/user").is_dir(), "below its path");

        let misspelt = BackendConfig::new("json", json!({ "pth": dir.path() }));
        let refused = backends
            .make_one::<dyn PersistenceBackend>(
                "entity store",
                "entity_store",
                &misspelt,
                &Services::empty(),
            )
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("the entity store (json)") && refused.contains("pth"),
            "{refused}"
        );
    }
}
