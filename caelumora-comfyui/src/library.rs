//! How ComfyUI is joined to the runtime (issue 512): the section `comfyui` of the configuration,
//! the workflows it names as a service, the texts of its tool, and the component kind
//! `comfyui_tool`.

use crate::workflow::Workflows;
use crate::{tool, NAMESPACE, PROMPTS};
use anyhow::{Context, Result};
use caelumora_runtime::library::{Library, LibraryBuilder};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// The section of the configuration and the name of the library.
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

/// ComfyUI, as a library: its section of the configuration, the workflows it names as a service,
/// and the tool that generates images with them.
#[derive(Debug, Default)]
pub struct ComfyUiLibrary;

impl Library for ComfyUiLibrary {
    fn name(&self) -> &'static str {
        COMFYUI
    }

    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()> {
        let config: ComfyUiConfig = library.config(COMFYUI)?;
        let workflows = match &config.workflows {
            Some(dir) => Workflows::read(dir).context("Cannot read libraries.comfyui.workflows")?,
            None => Workflows::default(),
        };
        library.provide(Arc::new(workflows))?;
        library.add_prompts(NAMESPACE, PROMPTS)?;
        tool::register(&mut library.components());
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

    /// The workflows the section names, of the example configuration here, are what the library
    /// offers; a section that names none offers none, and one whose workflows cannot be read keeps
    /// the runtime from being built, naming the parameter.
    #[test]
    fn the_workflows_the_section_names_are_offered_as_a_service() {
        let built = |section: Option<Value>| {
            let mut config = AppConfig::default();
            if let Some(section) = section {
                config.libraries.insert(COMFYUI.to_string(), section);
            }
            TestRuntime { config, libraries: vec![&ComfyUiLibrary], ..Default::default() }
                .try_build()
        };
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/comfyui/workflows");
        let runtime = built(Some(json!({ "workflows": examples }))).expect("the example's");
        let workflows = runtime.services().get::<Workflows>().expect("offered");
        let example = workflows.named("z_image_turbo").expect("the example's workflow");
        assert!(example.description().starts_with("fast"));

        let none = built(None).expect("no section, no workflows");
        assert!(none.services().get::<Workflows>().unwrap().named("z_image_turbo").is_none());
        let unreadable = built(Some(json!({ "workflows": "/none" }))).unwrap_err();
        assert!(
            format!("{unreadable:#}").contains("libraries.comfyui.workflows"),
            "{unreadable:#}"
        );
        assert!(built(Some(json!({ "workflow": "x" }))).is_err(), "a key it does not know");
    }

    /// Issue 516: a library's section is read from the libraries' sections alone - among the
    /// plugins' it is claimed by none -, and one no library claims keeps the runtime from being
    /// built.
    #[test]
    fn a_library_s_section_lies_among_the_libraries_sections() {
        let built = |yaml: &str| {
            let config = AppConfig::from_yaml(yaml).unwrap();
            TestRuntime { config, libraries: vec![&ComfyUiLibrary], ..Default::default() }
                .try_build()
                .map(drop)
        };
        let among_plugins = built("plugins:\n  comfyui: {}\n").expect_err("no plugin claims it");
        let message = format!("{among_plugins:#}");
        assert!(message.contains("'plugins.comfyui' that no plugin claims"), "{message}");
        let misspelt = built("libraries:\n  comfyuy: {}\n").expect_err("no library claims it");
        let message = format!("{misspelt:#}");
        assert!(message.contains("'libraries.comfyuy' that no library claims"), "{message}");
        built("libraries:\n  comfyui: {}\n").expect("its own");
    }
}
