//! How artifacts are joined to the runtime (issue 328): the storage `local`
//! as a backend of `dyn ArtifactStorage`, the store of their kind, the
//! storages the section `artifacts` of the configuration names, the
//! `ArtifactManager` as a service - what the component, the views and the
//! routes reach it by - and as a member of a scope, and the `artifact_tool`
//! component.

use crate::artifact::Artifact;
use crate::component;
use crate::local;
use crate::manager::{ArtifactManager, ArtifactStorage, ArtifactsConfig};
use caelumora_runtime::backend::Backends;
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use std::sync::Arc;
use anyhow::Result;

/// The section of the configuration and the name of the plugin.
pub const ARTIFACTS: &str = "artifacts";

/// Artifacts, as a plugin.
#[derive(Debug, Default)]
pub struct ArtifactsPlugin;

impl Plugin for ArtifactsPlugin {
    fn name(&self) -> &'static str {
        ARTIFACTS
    }

    fn backends(&self, backends: &mut Backends) {
        local::register_backend(backends);
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        let config: ArtifactsConfig = runtime.config(ARTIFACTS)?;
        let storages = runtime.make_named::<dyn ArtifactStorage>("storage of artifacts", &config.storage)?;
        let artifacts = runtime.entity_store::<Artifact>()?;
        let groups = Arc::clone(&runtime.entity_stores().groups);
        let manager = ArtifactManager::new(artifacts, groups, Arc::clone(runtime.event_bus()), Arc::clone(runtime.access_policy()), Arc::clone(runtime.confidentiality()), storages, &config)?;
        let manager = Arc::new(manager);
        runtime.provide(Arc::clone(&manager));
        runtime.scope_member(manager);
        component::register(runtime.components());
        Ok(())
    }
}
