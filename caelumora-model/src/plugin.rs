//! How the models are joined to the kernel (issues 520, 525): the section `plugins.model`, the
//! service that builds an agent its clients, the kind `language_model` with the language model's
//! facets and the kind `image_model` with an image model's.

use crate::manager::ModelManager;
use crate::{image_model, language_model, ImageModelProvider, LanguageModelProvider};
use anyhow::Result;
use caelumora_domain::config::{AppConfig, BackendConfig};
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The name of the plugin, and of its section of the configuration: the models by their kind -
/// `LANGUAGE`, `IMAGE` - and their names, each a backend.
pub const MODEL: &str = "model";

/// The key of the language models in the section.
pub const LANGUAGE: &str = "language";

/// The key of the image models in the section (issue 525).
pub const IMAGE: &str = "image";

/// The section `plugins.model`: the models the platform offers, by their kind and their names.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ModelConfig {
    /// The language models; left out, none.
    language: BTreeMap<String, BackendConfig>,
    /// The image models; left out, none.
    image: BTreeMap<String, BackendConfig>,
}

/// The models, as a plugin.
#[derive(Debug, Default)]
pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    fn name(&self) -> &'static str {
        MODEL
    }

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let section: ModelConfig = kernel.config(MODEL)?;
        let language =
            kernel.make_named::<dyn LanguageModelProvider>("language model", &section.language)?;
        let image = kernel.make_named::<dyn ImageModelProvider>("image model", &section.image)?;
        kernel.provide(Arc::new(ModelManager::new(language, image)));
        language_model::register_all(kernel.components());
        image_model::register_all(kernel.components());
        Ok(())
    }
}

/// `config`, with `model` as the language model of the name `name` where its section names none -
/// the section or its `LANGUAGE` left out, empty or null -, for a program that offers a model of
/// its own (issues 329, 520). A section that is anything else is left as it is, for `ModelPlugin`
/// to read or refuse.
pub fn with_default_language_model(
    mut config: AppConfig,
    name: &str,
    model: BackendConfig,
) -> AppConfig {
    let section = config.plugins.entry(MODEL.to_string()).or_insert(Value::Null);
    if section.is_null() {
        *section = Value::Object(Map::new());
    }
    let Value::Object(kinds) = section else { return config };
    let language = kinds.entry(LANGUAGE).or_insert(Value::Null);
    let configures_none = match language {
        Value::Null => true,
        Value::Object(models) => models.is_empty(),
        _ => false,
    };
    if configures_none {
        let models = BTreeMap::from([(name.to_string(), model)]);
        // Never fails: names to configurations of backends.
        *language = serde_json::to_value(models).unwrap_or(Value::Null);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{configuring, kernel_of, ANSWERING};
    use serde_json::json;

    /// Issue 520: a program's model is configured where the section names no language model, and
    /// only there.
    #[test]
    fn a_default_language_model_fills_a_section_that_names_none() {
        let local = || BackendConfig::new("local", Value::Null);
        let filled = with_default_language_model(AppConfig::default(), "local", local());
        assert_eq!(filled.plugins[MODEL], json!({ "language": { "local": { "type": "local" } } }));
        let other = BackendConfig::new("other", Value::Null);
        let kept = with_default_language_model(filled.clone(), "other", other);
        assert_eq!(kept.plugins, filled.plugins, "one configured is kept");
        for none in [json!(null), json!({}), json!({ "language": null }), json!({ "language": {} })]
        {
            let config = with_default_language_model(configuring(none.clone()), "local", local());
            assert_eq!(config.plugins, filled.plugins, "{none}");
        }
        for wrong in [json!("gemma"), json!({ "language": "gemma" })] {
            let left = with_default_language_model(configuring(wrong.clone()), "local", local());
            assert_eq!(left.plugins, configuring(wrong).plugins, "for the plugin to refuse");
        }
    }

    /// The section holds the models by their kind: one that names a model where a kind belongs
    /// keeps the kernel from being built.
    #[test]
    fn a_section_holds_the_models_by_their_kind() {
        let section = json!({ "gemma": { "type": ANSWERING, "answer": "ok" } });
        let refused = kernel_of(configuring(section)).unwrap_err();
        assert!(format!("{refused:#}").contains("unknown field `gemma`"), "{refused:#}");
    }
}
