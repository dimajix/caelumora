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

mod container;
mod context;
mod registry;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use container::{ComponentContainer, ComponentImpl};
pub use context::{service_of, ComponentContext};
pub use registry::{
    ComponentFacets, ComponentKind, ComponentRegistry, BUNDLE_REF_KIND, COMPONENT_DEFAULT_KIND,
};

use caelumora_model::component::ComponentDescriptor;
use caelumora_model::prompt_catalogue::PromptError;
use caelumora_model::store::StoreError;
use caelumora_model::type_map::TypeKey;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

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
    /// A text the component gives its model cannot be had from the prompt
    /// catalogue (issue 57) - a fault of the texts or of the component.
    #[error("{0}")]
    Prompt(#[from] PromptError),
    /// A component asked for a service of the runtime its kind does not
    /// declare it uses, or one the runtime does not have (issue 296).
    #[error("the {0} is not a service offered to the component")]
    NoService(&'static str),
    /// A facet was asked for that was not declared: by a component, one its
    /// kind does not declare it needs (`ComponentKind::needs`); by the owner
    /// of a container, one it was not built for (issue 337).
    #[error("{0} is not among the facets declared to be asked for")]
    Undeclared(&'static str),
    /// The component bundle a `ref` names cannot be read (issues 270, 455, 457): gone, not the
    /// reader's to read - a share withdrawn -, or its store failed, as `cause` says. Gone or not
    /// the reader's, it keeps an agent from being run, as its personality's would
    /// (`ComponentError::is_unavailable`).
    #[error("component bundle '{name}' cannot be read: {cause}")]
    Bundle { name: String, cause: StoreError },
    /// A component of a kind only an operator configures was written in a
    /// request rather than named through a `ref` (issue 403,
    /// `ComponentKind::operator_only`).
    #[error(
        "components of kind '{0}' are configured by an operator: name a bundle that holds one"
    )]
    OperatorOnly(String),
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
        Self::Build {
            component: entry.id.clone(),
            kind: entry.kind.clone(),
            cause: Box::new(cause),
        }
    }

    /// Whether it says that a component bundle is gone or not the reader's
    /// (`Bundle`, `StoreError::is_absent_or_refused`) - itself, or as the
    /// cause of a component that could not be built.
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Bundle { cause, .. } => cause.is_absent_or_refused(),
            Self::Build { cause, .. } => cause.is_unavailable(),
            _ => false,
        }
    }
}

pub type ComponentResult<T> = Result<T, ComponentError>;

/// A component's `params` read as its configuration `T`; left out, they are
/// read as an empty object, so that a `T` whose fields all have a default
/// is built from them.
///
/// A parameter is given a value or left out, never `null`: written without
/// a value in YAML, it is more likely a mistake than meant as left out, and
/// a field of `T` that may be left out (`Option`) would take it as left out
/// where another field refuses it.
///
/// # Errors
/// `ComponentError::InvalidConfig` if they are no object - a list would be
/// read by the order of `T`'s fields -, a parameter is `null`, or they are
/// not a `T`, naming the parameter that is not what it should be, where it
/// is one.
pub fn config_of_params<T: DeserializeOwned>(params: &Value) -> ComponentResult<T> {
    let empty = Value::Object(Map::new());
    let params = match params {
        Value::Null => &empty,
        Value::Object(given) => {
            if let Some((name, _)) = given.iter().find(|(_, value)| value.is_null()) {
                return Err(ComponentError::InvalidConfig(format!(
                    "invalid parameter '{name}': it has no value"
                )));
            }
            params
        }
        _ => {
            return Err(ComponentError::InvalidConfig(
                "invalid parameters: they must be an object".to_string(),
            ))
        }
    };
    serde_path_to_error::deserialize(params).map_err(|e| {
        let path = e.path().to_string();
        let message = match path.as_str() {
            "." => format!("invalid parameters: {}", e.inner()),
            parameter => format!("invalid parameter '{parameter}': {}", e.inner()),
        };
        ComponentError::InvalidConfig(message)
    })
}

/// Whether `T` is among the facets `declared` to be asked for.
///
/// # Errors
/// `ComponentError::Undeclared` if it is not.
fn require_declared<T: ?Sized + 'static>(declared: &[TypeKey]) -> ComponentResult<()> {
    let facet = TypeKey::of::<T>();
    if declared.iter().any(|f| f.id == facet.id) {
        Ok(())
    } else {
        Err(ComponentError::Undeclared(facet.name()))
    }
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
        assert_eq!(
            config_of_params::<Config>(&Value::Null).unwrap().limit,
            0,
            "left out: all defaults"
        );
        assert_eq!(config_of_params::<Config>(&json!({ "limit": 3 })).unwrap().limit, 3);
        let message = config_of_params::<Config>(&json!({ "limti": 3 })).unwrap_err().to_string();
        assert!(
            message.starts_with(
                "invalid configuration: invalid parameter 'limti': unknown field `limti`"
            ),
            "{message}"
        );
        let message = config_of_params::<Config>(&json!({ "limit": -1 })).unwrap_err().to_string();
        assert!(
            message.starts_with("invalid configuration: invalid parameter 'limit': invalid value"),
            "the parameter named: {message}"
        );
        assert!(
            config_of_params::<Config>(&json!([3])).is_err(),
            "a list is not read by the order of the fields"
        );
    }

    #[test]
    fn a_parameter_without_a_value_is_refused_whether_it_may_be_left_out_or_not() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            #[serde(default)]
            title: Option<String>,
        }
        assert!(config_of_params::<Config>(&json!({})).unwrap().title.is_none());
        let message =
            config_of_params::<Config>(&json!({ "title": null })).unwrap_err().to_string();
        assert!(
            message.contains("invalid parameter 'title': it has no value"),
            "not taken as left out: {message}"
        );
    }
}
