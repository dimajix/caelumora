//! How the models are joined to the kernel (issue 520): the section `plugins.model`, the service
//! that builds an agent its client, and the kind `language_model` with the model's facets.

use crate::language_model;
use crate::manager::ModelManager;
use crate::LanguageModelProvider;
use anyhow::Result;
use caelumora_domain::config::{AppConfig, BackendConfig};
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The name of the plugin, and of its section of the configuration: the models by their names,
/// each a backend.
pub const MODEL: &str = "model";

/// The models, as a plugin.
#[derive(Debug, Default)]
pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    fn name(&self) -> &'static str {
        MODEL
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let configs: BTreeMap<String, BackendConfig> = kernel.config(MODEL)?;
        let models = kernel.make_named::<dyn LanguageModelProvider>("model", &configs)?;
        kernel.provide(Arc::new(ModelManager::new(models)));
        language_model::register_all(kernel.components());
        Ok(())
    }
}

/// `config`, with `model` as the model of the name `name` where its section names none - left out,
/// empty or null -, for a program that offers a model of its own (issues 329, 520). A section that
/// is anything else is left as it is, for `ModelPlugin` to read or refuse.
pub fn with_default_model(mut config: AppConfig, name: &str, model: BackendConfig) -> AppConfig {
    let section = config.plugins.entry(MODEL.to_string()).or_insert(Value::Null);
    let configures_none = match section {
        Value::Null => true,
        Value::Object(models) => models.is_empty(),
        _ => false,
    };
    if configures_none {
        let models = BTreeMap::from([(name.to_string(), model)]);
        // Never fails: names to configurations of backends.
        *section = serde_json::to_value(models).unwrap_or(Value::Null);
    }
    config
}
