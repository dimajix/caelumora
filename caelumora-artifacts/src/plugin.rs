//! How artifacts are joined to the runtime (issue 328): the store of their
//! kind, the `ArtifactManager` with the storages of content (issue 282) and
//! the section `artifacts` of the configuration, as a service - what the
//! component, the views and the routes reach it by - and as a member of a
//! scope, and the `artifact_tool` component.

use crate::artifact::Artifact;
use crate::component;
use crate::manager::{ArtifactManager, ArtifactsConfig};
use crate::{NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use std::sync::Arc;

/// The section of the configuration and the name of the plugin.
pub const ARTIFACTS: &str = "artifacts";

/// Artifacts, as a plugin.
#[derive(Debug, Default)]
pub struct ArtifactsPlugin;

impl Plugin for ArtifactsPlugin {
    fn name(&self) -> &'static str {
        ARTIFACTS
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        let config: ArtifactsConfig = runtime.config(ARTIFACTS)?;
        let artifacts = runtime.entity_store::<Artifact>()?;
        let manager = ArtifactManager::new(
            runtime.entity_stores(),
            artifacts,
            Arc::clone(runtime.event_bus()),
            Arc::clone(runtime.access_policy()),
            Arc::clone(runtime.confidentiality()),
            Arc::clone(runtime.content_storages()),
            &config,
        )?;
        let manager = Arc::new(manager);
        runtime.provide(Arc::clone(&manager));
        runtime.scope_member(manager);
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        component::register(runtime.components());
        Ok(())
    }
}
