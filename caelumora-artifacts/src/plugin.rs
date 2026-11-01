//! How artifacts are joined to the kernel (issue 328): the store of their kind, the
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
use caelumora_kernel::component::LibraryComponents;
use caelumora_kernel::kernel::KernelBuilder;
use caelumora_kernel::plugin::Plugin;
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

    fn register(&self, kernel: &mut KernelBuilder) -> Result<()> {
        let config: ArtifactsConfig = kernel.config(ARTIFACTS)?;
        let artifacts = kernel.entity_store::<Artifact>()?;
        let manager = ArtifactManager::new(kernel, artifacts, &config)?;
        // Bound only where something can be published: a storage of content is configured.
        let configured = manager.is_configured();
        let manager = Arc::new(manager);
        kernel.provide(Arc::clone(&manager));
        kernel.scope_member(manager);
        kernel.extend_writers::<dyn ArtifactPublisher, _, _>(move |session| async move {
            configured.then(|| Arc::new(session.views().clone()) as Arc<dyn ArtifactPublisher>)
        })?;
        kernel.prompts().add(NAMESPACE, PROMPTS)?;
        // Its component kind is a library's (issue 516), though the crate registers as a plugin
        // until it is cut in two (step 5d of issue 516).
        component::register(&mut LibraryComponents::new(kernel.components()));
        Ok(())
    }
}
