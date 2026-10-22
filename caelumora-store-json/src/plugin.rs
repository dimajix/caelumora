//! How the JSON store is joined to the runtime (issue 446): as a backend of
//! the stores alone.

use crate::JsonStoreConfig;
use caelumora_model::store::Persistence;
use caelumora_runtime::backend::Backends;
use caelumora_runtime::component::{config_of_params, ComponentError};
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use anyhow::Result;
use std::sync::Arc;

/// The backend `json` of the stores (`JsonFileStore`), as a plugin.
#[derive(Debug, Default)]
pub struct JsonStorePlugin;

impl Plugin for JsonStorePlugin {
    fn name(&self) -> &'static str {
        "store_json"
    }

    fn backends(&self, backends: &mut Backends) {
        backends.register::<dyn Persistence, _>("json", |_, params, _| {
            let config: JsonStoreConfig = config_of_params(params)?;
            let store = config.store().map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            Ok(Arc::new(store))
        });
    }

    fn register(&self, _runtime: &mut RuntimeBuilder) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::config::BackendConfig;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::security::AllowAllPolicy;
    use caelumora_model::store::{EntityStoreFactory, StoreBackend};
    use caelumora_model::user::User;
    use caelumora_runtime::runtime::Services;
    use serde_json::json;

    /// The stores keep their data in JSON files once the plugin is there,
    /// made from the parameters and refused naming the store.
    #[test]
    fn the_stores_keep_their_data_in_json_files() {
        let backends = Backends::with_plugins(&[&JsonStorePlugin]);
        let dir = tempfile::tempdir().unwrap();
        let json = BackendConfig::new("json", json!({ "path": dir.path(), "journal_floor": 1024 }));
        let files = backends.make_one::<dyn Persistence>("entity store", "entity_store", &json, &Services::empty()).unwrap();
        let stores = StoreBackend::new(files, Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()));
        stores.entity_store::<User>().expect("its kinds are opened in its files");
        assert!(dir.path().join("entities/user").is_dir(), "below its path");

        let misspelt = BackendConfig::new("json", json!({ "pth": dir.path() }));
        let refused = backends.make_one::<dyn Persistence>("entity store", "entity_store", &misspelt, &Services::empty()).map(|_| ()).unwrap_err().to_string();
        assert!(refused.contains("the entity store (json)") && refused.contains("pth"), "{refused}");
    }
}
