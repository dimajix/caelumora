//! How ComfyUI is joined to the runtime (issue 512): the section `comfyui` of the configuration,
//! the workflows it names as a service, the texts of its tool, and the component kind
//! `comfyui_tool`.

use crate::workflow::Workflows;
use crate::{tool, NAMESPACE, PROMPTS};
use anyhow::{Context, Result};
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// The section of the configuration and the name of the plugin.
pub const COMFYUI: &str = "comfyui";

/// The section `comfyui` of the configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComfyUiConfig {
    /// The directory of the workflows, read as the seed files are, from where the program runs;
    /// left out, there are none.
    #[serde(default)]
    workflows: Option<PathBuf>,
}

/// ComfyUI, as a plugin: its section of the configuration, the workflows it names as a service,
/// and the tool that generates images with them.
#[derive(Debug, Default)]
pub struct ComfyUiPlugin;

impl Plugin for ComfyUiPlugin {
    fn name(&self) -> &'static str {
        COMFYUI
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        let config: ComfyUiConfig = runtime.config(COMFYUI)?;
        let workflows = match &config.workflows {
            Some(dir) => Workflows::read(dir).context("Cannot read comfyui.workflows")?,
            None => Workflows::default(),
        };
        runtime.provide(Arc::new(workflows));
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        tool::register(runtime.components());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::config::AppConfig;
    use caelumora_runtime::testing::TestRuntime;
    use serde_json::{json, Value};
    use std::path::Path;

    /// The workflows the section names, of the example configuration here, are what the plugin
    /// offers; a section that names none offers none, and one whose workflows cannot be read keeps
    /// the runtime from being built, naming the parameter.
    #[test]
    fn the_workflows_the_section_names_are_offered_as_a_service() {
        let built = |section: Option<Value>| {
            let mut config = AppConfig::default();
            if let Some(section) = section {
                config.plugins.insert(COMFYUI.to_string(), section);
            }
            TestRuntime { config, plugins: vec![&ComfyUiPlugin], ..Default::default() }.try_build()
        };
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/comfyui/workflows");
        let runtime = built(Some(json!({ "workflows": examples }))).expect("the example's");
        let workflows = runtime.services().get::<Workflows>().expect("offered");
        let example = workflows.named("z_image_turbo").expect("the example's workflow");
        assert!(example.description().starts_with("fast"));

        let none = built(None).expect("no section, no workflows");
        assert!(none.services().get::<Workflows>().unwrap().named("z_image_turbo").is_none());
        let unreadable = built(Some(json!({ "workflows": "/none" }))).unwrap_err();
        assert!(format!("{unreadable:#}").contains("comfyui.workflows"), "{unreadable:#}");
        assert!(built(Some(json!({ "workflow": "x" }))).is_err(), "a key it does not know");
    }
}
