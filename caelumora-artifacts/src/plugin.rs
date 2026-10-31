//! How artifacts are joined to the runtime (issue 328): the store of their kind, the
//! `ArtifactManager` with the storages of content (issue 282) and the section `artifacts` of the
//! configuration, as a service - what the views and the routes reach it by - and as a member of a
//! scope, the `ArtifactPublisher` bound to the views of an agent's work where a storage of content
//! is configured - what the component publishes through (issue 516) -, and the `artifact_tool`
//! component.

use crate::artifact::Artifact;
use crate::component;
use crate::extension::ArtifactPublisher;
use crate::manager::{ArtifactManager, ArtifactsConfig};
use crate::{NAMESPACE, PROMPTS};
use anyhow::Result;
use caelumora_runtime::component::LibraryComponents;
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
        let manager = ArtifactManager::new(runtime, artifacts, &config)?;
        // Bound only where something can be published: a storage of content is configured.
        let configured = manager.is_configured();
        let manager = Arc::new(manager);
        runtime.provide(Arc::clone(&manager));
        runtime.scope_member(manager);
        runtime.extend_writers::<dyn ArtifactPublisher, _, _>(move |session| async move {
            configured.then(|| Arc::new(session.views().clone()) as Arc<dyn ArtifactPublisher>)
        })?;
        runtime.prompts().add(NAMESPACE, PROMPTS)?;
        // Its component kind is a library's (issue 516), though the crate registers as a plugin
        // until it is cut in two (step 5d of issue 516).
        component::register(&mut LibraryComponents::new(runtime.components()));
        Ok(())
    }
}
