//! Components and how they are wired (issue 99), the way Spring Boot wires
//! beans: a component kind declares, when it is registered, which facets it
//! provides; what a component needs, its factory asks its context for by
//! type while it is built - `context.all::<dyn PromptProvider>()` - which
//! builds the components that provide it on demand. Nothing is wired by
//! hand, so nothing can be forgotten.
//!
//! Of most facets there may be any number. Of some there is exactly one per
//! container (`ComponentRegistry::single`, e.g. what assembles the model's
//! context): two components providing one are an error, and if none does,
//! a default is built when it is first asked for - the configuration's
//! (`AppConfig::component_defaults`), or else the kind registered in code.
//!
//! The parts: the kinds are registered, and a configuration is planned, in the `ComponentRegistry`
//! (`registry`); the registry builds the components of one configuration in a `BuildContext` - its
//! owner's name and the kernel's services (`context`) - as a graph (`container`), which its owner
//! holds as a `ComponentContainer`, handing out the facets as `ComponentImpl`s. What a library
//! declares and is given - the kind, the factory, the `ComponentContext` a component is built with,
//! the container - is the API's (`api::component`, issue 516).

mod container;
mod context;
mod registry;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use context::BuildContext;
pub use registry::{ComponentRegistry, BUNDLE_REF_KIND, COMPONENT_DEFAULT_KIND};
