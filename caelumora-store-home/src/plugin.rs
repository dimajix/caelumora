//! How the stores of a program of one person are joined to the kernel (issue 554): as a backend of
//! the stores alone.

use crate::{HomeStore, HomeStoreConfig};
use anyhow::Result;
use caelumora_api::component::{ComponentError, Params};
use caelumora_kernel::backend::Backends;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use caelumora_store::PersistenceBackend;
use std::sync::Arc;

/// The backend `home` of the stores (`HomeStore`), as a plugin.
#[derive(Debug, Default)]
pub struct HomeStorePlugin;

impl Params for HomeStoreConfig {}

impl Plugin for HomeStorePlugin {
    fn name(&self) -> &'static str {
        "store_home"
    }

    fn backends(&self, backends: &mut Backends) {
        backends.register::<dyn PersistenceBackend, _, _>(
            "home",
            |_, config: HomeStoreConfig, _| {
                let store = HomeStore::new(&config)
                    .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
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

    /// The stores keep their data below the data directory once the plugin is there, made from
    /// the parameters and refused naming the store.
    #[test]
    fn the_stores_keep_their_data_below_the_data_directory() {
        let backends = Backends::with_plugins(&[&HomeStorePlugin]);
        let dir = tempfile::tempdir().unwrap();
        let home = BackendConfig::new(
            "home",
            json!({ "data_dir": dir.path(), "config_dir": dir.path().join("config") }),
        );
        let files = backends
            .make_one::<dyn PersistenceBackend>(
                "entity store",
                "entity_store",
                &home,
                &Services::empty(),
            )
            .unwrap();
        let stores = Persistence::over(files).expect("its states, too");
        stores.entity_store::<User>(&Policies::permitting_all()).expect("its kinds are opened");
        assert!(dir.path().join("locks").is_dir(), "below its data directory");

        let misspelt = BackendConfig::new("home", json!({ "data": dir.path() }));
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
            refused.contains("the entity store (home)") && refused.contains("data"),
            "{refused}"
        );
    }
}
