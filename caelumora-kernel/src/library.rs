//! How the kernel serves a library's registration (`caelumora_api::library`, issue 516): the part
//! of the `KernelBuilder` a library may use.

use crate::kernel::KernelBuilder;
use anyhow::Result;
use caelumora_api::actor::AgentFactory;
use caelumora_api::component::LibraryComponents;
use caelumora_api::library::LibraryBuilder;
use caelumora_domain::agent::PersonalityKind;
use caelumora_domain::prompt_catalogue::PromptError;
use caelumora_domain::type_map::{ErasedArc, TypeKey};
use serde_json::Value;

/// The part of the `KernelBuilder` a library registers with.
pub(crate) struct ForLibrary<'a>(pub(crate) &'a mut KernelBuilder);

impl LibraryBuilder for ForLibrary<'_> {
    fn components(&mut self) -> LibraryComponents<'_> {
        self.0.components().library_components()
    }

    fn register_agent_kind(
        &mut self,
        kind: PersonalityKind,
        facets: &'static [TypeKey],
        factory: AgentFactory,
    ) -> Result<()> {
        self.0.agent_kinds().register(kind, facets, factory)
    }

    fn add_prompts(&mut self, namespace: &str, yaml: &str) -> Result<(), PromptError> {
        self.0.prompts().add(namespace, yaml)
    }

    fn section(&mut self, name: &str) -> Result<Option<Value>> {
        self.0.library_section(name)
    }

    fn provide_of(&mut self, key: TypeKey, service: ErasedArc) -> Result<()> {
        self.0.provide_of_library(key, service)
    }
}
