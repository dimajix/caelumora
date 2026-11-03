//! Libraries (issue 516): the crates of the agent side - what runs an agent or brings components
//! for its facets -, joined to the kernel when it is built, but trusted for nothing. A library
//! registers with a `LibraryBuilder`, which offers what a library registers and nothing of the
//! kernel: no store, no policy, no secret, no controller. Its component kinds are marked as a
//! library's (`LibraryComponents`), and the kernel is not built where one provides a facet the
//! kernel builds in its own name or uses a service of the kernel but the texts. Which libraries are
//! in is the program's choice, as which plugins are.

use crate::actor::AgentFactory;
use crate::component::LibraryComponents;
use anyhow::{Context, Result};
use caelumora_domain::agent::PersonalityKind;
use caelumora_domain::prompt_catalogue::PromptError;
use caelumora_domain::type_map::{ErasedArc, TypeKey};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;

/// A crate of the agent side, joined to the kernel as a plugin is, trusted for nothing.
pub trait Library: Send + Sync {
    /// Its name, as messages name it.
    fn name(&self) -> &'static str;

    /// Registers what it brings with `library`.
    ///
    /// # Errors
    /// If its section of the configuration cannot be read, or what it brings cannot be made - the
    /// process does not start then.
    fn register(&self, library: &mut dyn LibraryBuilder) -> Result<()>;
}

/// What a library registers with (issue 516), as the kernel serves it: its component kinds, its
/// kinds of agent, its texts, its section of the configuration and the services it offers its own
/// kinds. Typed on `dyn LibraryBuilder` (`config`, `provide`), as `ActorRuntime::reader` is.
pub trait LibraryBuilder {
    /// The component kinds, to register the library's with, marked as a library's.
    fn components(&mut self) -> LibraryComponents<'_>;

    /// Registers the agents of personalities of `kind` (issue 370): their actor is built by
    /// `factory` from the agent's runtime, their components are asked for `facets` (issue 337).
    ///
    /// # Errors
    /// If `kind` is registered already, by this library or another: none replaces it (issue 521).
    fn register_agent_kind(
        &mut self,
        kind: PersonalityKind,
        facets: &'static [TypeKey],
        factory: AgentFactory,
    ) -> Result<()>;

    /// Adds the library's texts given to models (issue 57), `yaml`, under its `namespace` - and
    /// nothing else of the catalogue: the variants are the configuration's.
    ///
    /// # Errors
    /// As `PromptCatalogue::add`: a namespace added before among them.
    fn add_prompts(&mut self, namespace: &str, yaml: &str) -> Result<(), PromptError>;

    /// Claims the section `name` of the libraries in the configuration (`libraries`), as it
    /// stands; `None` if it is left out.
    ///
    /// # Errors
    /// If another library claimed it.
    fn section(&mut self, name: &str) -> Result<Option<Value>>;

    /// Offers `service`, an `Arc` of the type `key` names, to the kinds of the libraries that use
    /// it.
    ///
    /// # Errors
    /// If a service of its type is offered already - by the kernel, a plugin or a library: a
    /// library replaces none.
    fn provide_of(&mut self, key: TypeKey, service: ErasedArc) -> Result<()>;
}

impl dyn LibraryBuilder + '_ {
    /// Claims the section `name` of the libraries in the configuration (`section`) and reads it as
    /// `C`; left out, it is `C::default()`.
    ///
    /// # Errors
    /// If another library claimed it, or it is not a `C`.
    pub fn config<C: DeserializeOwned + Default>(&mut self, name: &str) -> Result<C> {
        match self.section(name)? {
            Some(section) => serde_json::from_value(section).with_context(|| {
                format!("Cannot read the section 'libraries.{name}' of the configuration")
            }),
            None => Ok(C::default()),
        }
    }

    /// Offers `service`, the library's own - which decides nothing of the kernel's -, to the kinds
    /// of the libraries that use it (`provide_of`).
    ///
    /// # Errors
    /// As `provide_of`.
    pub fn provide<T: ?Sized + Send + Sync + 'static>(&mut self, service: Arc<T>) -> Result<()> {
        self.provide_of(TypeKey::of::<T>(), ErasedArc::of(service))
    }
}
