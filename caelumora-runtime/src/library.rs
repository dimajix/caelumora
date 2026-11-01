//! Libraries (issue 516): the crates of the agent side - what runs an agent or brings components
//! for its facets -, joined to the core as plugins are, when the runtime is built, but trusted for
//! nothing. A library registers with a `LibraryBuilder`, which offers what a library registers and
//! nothing of the core: no store, no policy, no secret, no controller. Its component kinds are
//! marked as a library's (`LibraryComponents`), and the runtime is not built where one provides a
//! facet the core builds in its own name or uses a service of the core but the texts
//! (`RuntimeBuilder::build`). Which libraries are in is the program's choice, as which plugins are.

use crate::actor::AgentKinds;
use crate::component::LibraryComponents;
use crate::runtime::RuntimeBuilder;
use anyhow::Result;
use caelumora_model::prompt_catalogue::PromptError;
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// A crate of the agent side, joined to the core as a plugin is (`Plugin`), trusted for nothing.
pub trait Library: Send + Sync {
    /// Its name, as messages name it.
    fn name(&self) -> &'static str;

    /// Registers what it brings with `library`.
    ///
    /// # Errors
    /// If its section of the configuration cannot be read, or what it brings cannot be made - the
    /// process does not start then.
    fn register(&self, library: &mut LibraryBuilder<'_>) -> Result<()>;
}

/// What a library registers with (issue 516): the part of the `RuntimeBuilder` a library may use -
/// its component kinds, its kinds of agent, its texts, its section of the configuration and the
/// services it offers its own kinds.
#[derive(Debug)]
pub struct LibraryBuilder<'a> {
    runtime: &'a mut RuntimeBuilder,
}

impl<'a> LibraryBuilder<'a> {
    /// The part of `runtime` a library registers with.
    pub(crate) fn new(runtime: &'a mut RuntimeBuilder) -> Self {
        Self { runtime }
    }

    /// The component kinds, to register the library's with, marked as a library's.
    pub fn components(&mut self) -> LibraryComponents<'_> {
        LibraryComponents::new(self.runtime.components())
    }

    /// The kinds of agent the program runs (issue 370), to register the library's with.
    pub fn agent_kinds(&mut self) -> &mut AgentKinds {
        self.runtime.agent_kinds()
    }

    /// Adds the library's texts given to models (issue 57), `yaml`, under its `namespace`
    /// (`PromptCatalogue::add`) - and nothing else of the catalogue: the variants are the
    /// configuration's.
    ///
    /// # Errors
    /// As `PromptCatalogue::add`: a namespace added before among them.
    pub fn add_prompts(&mut self, namespace: &str, yaml: &str) -> Result<(), PromptError> {
        self.runtime.prompts().add(namespace, yaml)
    }

    /// Claims the section `name` of the libraries in the configuration (`libraries`) and reads it
    /// as `C`; left out, it is `C::default()`.
    ///
    /// # Errors
    /// If another library claimed it, or it is not a `C`.
    pub fn config<C: DeserializeOwned + Default>(&mut self, name: &str) -> Result<C> {
        self.runtime.library_config(name)
    }

    /// Offers `service`, the library's own - which decides nothing of the core's, as
    /// `caelumora-comfyui`'s workflows -, to the kinds of the libraries that use it.
    ///
    /// # Errors
    /// If a service of its type is offered already - by the core, a plugin or a library: a library
    /// replaces none.
    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) -> Result<()> {
        self.runtime.provide_of_library(service)
    }
}
