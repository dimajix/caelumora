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
//! The parts: the kinds are registered, and a configuration is planned, in
//! the `ComponentRegistry` (`registry`); a factory builds its component with
//! the `ComponentContext` it is given (`context`); the `ComponentContainer`
//! builds the components of one configuration and hands them out as
//! `ComponentImpl`s (`container`).

use caelumora_model::component::ComponentDescriptor;
use caelumora_model::store::StoreError;
use caelumora_model::type_map::TypeKey;
use serde::de::DeserializeOwned;

mod container;
mod context;
mod registry;
#[cfg(test)]
mod testing;

pub use container::{ComponentContainer, ComponentImpl};
pub use context::{service_of, ComponentContext};
pub use registry::{ComponentFacets, ComponentKind, ComponentRegistry, PRECONFIGURED_REF_KIND};

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("unsupported component kind: {0}")]
    UnsupportedKind(String),
    #[error("duplicate component id: {0}")]
    DuplicateId(String),
    /// Two components need each other: `component` asked for `facet`,
    /// which `other` provides - and `other` is still being built, waiting
    /// for `component` or for something that waits for it.
    #[error("component '{component}' needs the {facet} of component '{other}', which is being built and needs it in turn")]
    Cycle { component: String, facet: &'static str, other: String },
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// A component asked for a service of the runtime its kind does not
    /// declare it uses, or one the runtime does not have (issue 296).
    #[error("the {0} is not a service offered to the component")]
    NoService(&'static str),
    /// A facet was asked for that was not declared: by a component, one its
    /// kind does not declare it needs (`ComponentKind::needs`); by the owner
    /// of a container, one it was not built for (issue 337).
    #[error("{0} is not among the facets declared to be asked for")]
    Undeclared(&'static str),
    /// The preconfigured components `id` a `ref` names cannot be read -
    /// gone, or not the reader's to read, a share withdrawn (issue 270):
    /// what keeps an agent from being run, as its personality's would
    /// (`ComponentError::is_unavailable`).
    #[error("preconfigured component '{id}' cannot be read: {cause}")]
    Unavailable { id: String, cause: StoreError },
    /// A component of a graph could not be built. Wraps whatever went wrong
    /// with the component's id and kind, so that the message names the
    /// offending component on its own (issue 02).
    #[error("component '{component}' ({kind}) cannot be built: {cause}")]
    Build { component: String, kind: String, cause: Box<ComponentError> },
    /// A backend of a service could not be made (issue 324): `backend`
    /// names it - "the storage of workspaces 'main'" -, `kind` is its
    /// type, as `Build` names a component.
    #[error("{backend} ({kind}) cannot be made: {cause}")]
    Backend { backend: String, kind: String, cause: Box<ComponentError> },
}

impl ComponentError {
    /// `cause`, as what went wrong with the component `entry` (`Build`).
    fn of_component(entry: &ComponentDescriptor, cause: ComponentError) -> Self {
        Self::Build { component: entry.id.clone(), kind: entry.kind.clone(), cause: Box::new(cause) }
    }

    /// Whether it says that preconfigured components cannot be read
    /// (`Unavailable`) - itself, or as the cause of a component that could
    /// not be built.
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Unavailable { .. } => true,
            Self::Build { cause, .. } => cause.is_unavailable(),
            _ => false,
        }
    }
}

pub type ComponentResult<T> = Result<T, ComponentError>;

/// The switch `name` of a component's `params`, `false` if it is left out.
///
/// # Errors
/// `ComponentError::InvalidConfig` if it is neither `true` nor `false`.
pub fn flag_of_params(params: &serde_json::Value, name: &str) -> ComponentResult<bool> {
    match params.get(name) {
        None => Ok(false),
        Some(value) => value.as_bool()
            .ok_or_else(|| ComponentError::InvalidConfig(format!("parameter '{name}' must be true or false"))),
    }
}

/// A component's `params` read as its configuration `T`; left out, they are
/// read as an empty object, so that a `T` whose fields all have a default
/// is built from them.
///
/// # Errors
/// `ComponentError::InvalidConfig` if they are not a `T`.
pub fn config_of_params<T: DeserializeOwned>(params: &serde_json::Value) -> ComponentResult<T> {
    let result = match params {
        serde_json::Value::Null => T::deserialize(serde_json::json!({})),
        params => T::deserialize(params),
    };
    result.map_err(|e| ComponentError::InvalidConfig(format!("invalid parameters: {e}")))
}

/// Whether `T` is among the facets `declared` to be asked for.
///
/// # Errors
/// `ComponentError::Undeclared` if it is not.
fn require_declared<T: ?Sized + 'static>(declared: &[TypeKey]) -> ComponentResult<()> {
    let facet = TypeKey::of::<T>();
    if declared.iter().any(|f| f.id == facet.id) { Ok(()) } else { Err(ComponentError::Undeclared(facet.name())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

        #[test]
        fn params_left_out_give_the_defaults() {
            #[derive(Debug, Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Config {
                #[serde(default)]
                limit: u32,
            }
            assert_eq!(config_of_params::<Config>(&serde_json::Value::Null).unwrap().limit, 0, "left out: all defaults");
            assert_eq!(config_of_params::<Config>(&json!({ "limit": 3 })).unwrap().limit, 3);
            let message = config_of_params::<Config>(&json!({ "limti": 3 })).unwrap_err().to_string();
            assert!(message.starts_with("invalid configuration: invalid parameters: unknown field `limti`"), "{message}");
        }
}
