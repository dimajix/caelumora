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

use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::security::SecurityContext;
use caelumora_model::store::{ComponentStore, StoreError};
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

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
}

impl ComponentError {
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

/// A type, by its id and - for messages - its name: of a facet a component
/// provides, or of a service it uses.
#[derive(Debug, Clone, Copy)]
struct TypeKey {
    id: TypeId,
    name: &'static str,
}

impl TypeKey {
    fn of<T: ?Sized + 'static>() -> Self {
        // The trait's name without its path: `dyn crate::context::PromptProvider`
        // is shown as `PromptProvider`.
        let full = std::any::type_name::<T>();
        let name = full.rfind("::").map_or(full, |at| &full[at + 2..]);
        Self { id: TypeId::of::<T>(), name }
    }
}

/// What a component is built with: the environment of the whole graph - the
/// security context it is built under, the services of the runtime it uses
/// (`service`) - and, while it is built, the other components of its graph,
/// asked for by type (`all`, `one`). This is all a component is given: not its
/// actor's runtime, so that it cannot store its actor's state past the `Turn`
/// it is called with (issue 128); and of the services, only those its kind
/// declares it uses (`ComponentKind::uses`, issue 296).
#[derive(Clone)]
pub struct ComponentContext {
    pub security_context: SecurityContext,
    /// The services of the runtime: all of them in the context a graph is
    /// built with, those its kind uses in the one a component is given.
    services: ComponentServices,
    /// The graph being built and the component this context was given to;
    /// set by `ComponentContainer::from_config`.
    building: Option<(Arc<Graph>, String)>,
}

impl std::fmt::Debug for ComponentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentContext")
            .field("security_context", &self.security_context)
            .field("services", &self.services.names())
            .field("building", &self.building.as_ref().map(|(_, component)| component))
            .finish()
    }
}

impl ComponentContext {
    /// The environment a component graph is built in, with the services of
    /// the runtime: `ComponentContainer::from_config` gives each component
    /// this context, with the services its kind uses, through which it
    /// reaches the others.
    pub fn new(security_context: SecurityContext, services: ComponentServices) -> Self {
        Self { security_context, services, building: None }
    }

    /// The service `T` of the runtime - the policy (`dyn SecurityManager`),
    /// the secrets (`SecretManager`), the workspaces (`WorkspaceManager`) -,
    /// for a component whose kind declares it uses it.
    ///
    /// # Errors
    /// `ComponentError::NoService` if the kind does not declare it, or the
    /// runtime has none.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<&Arc<T>> {
        self.services.get::<T>().ok_or(ComponentError::NoService(TypeKey::of::<T>().name))
    }

    /// The context for a component of a kind that uses `services`: this one,
    /// with those of its services alone.
    fn for_kind(&self, services: &[TypeKey]) -> Self {
        Self { services: self.services.only(services), ..self.clone() }
    }

    /// Every other component of the graph that provides `T`, in the order of
    /// the configuration - built first if they are not yet.
    ///
    /// # Errors
    /// Whatever building one of them reports; `ComponentError::Cycle` if one
    /// of them is being built and waits for this one.
    pub async fn all<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<Vec<ComponentImpl<T>>> {
        let (graph, component) = self.building()?;
        graph.all::<T>(Some(component)).await
    }

    /// The one component that provides `T`, a facet of which there is
    /// exactly one (`ComponentRegistry::single`): the graph's, or the
    /// default - built first if it is not yet.
    ///
    /// # Errors
    /// As [`all`](Self::all).
    pub async fn one<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<ComponentImpl<T>> {
        let (graph, component) = self.building()?;
        graph.one::<T>(Some(component)).await
    }

    fn building(&self) -> ComponentResult<(&Arc<Graph>, &str)> {
        self.building.as_ref()
            .map(|(graph, component)| (graph, component.as_str()))
            .ok_or_else(|| ComponentError::InvalidConfig("other components can only be asked for while a component is built".to_string()))
    }
}

/// Values by their type, each an `Arc<T>` of a trait object or a type: the
/// facets a component provides (`ComponentFacets`), the services of the
/// runtime (`ComponentServices`).
#[derive(Default, Debug, Clone)]
pub struct TypeMap {
    map: HashMap<TypeId, (&'static str, Arc<dyn Any + Send + Sync>)>,
}

/// The facets a component provides, by type.
pub type ComponentFacets = TypeMap;

/// The services of the runtime a component may use, by type (issue 296).
pub type ComponentServices = TypeMap;

impl TypeMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, value: Arc<T>) {
        let key = TypeKey::of::<T>();
        self.map.insert(key.id, (key.name, Arc::new(value) as Arc<dyn Any + Send + Sync>));
    }

    /// It with `value` besides.
    pub fn with<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.insert(value);
        self
    }

    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|(_, boxed)| boxed.downcast_ref::<Arc<T>>())
    }

    /// It with the values of `keys` alone.
    fn only(&self, keys: &[TypeKey]) -> Self {
        let map = keys.iter().filter_map(|key| self.map.get(&key.id).map(|value| (key.id, value.clone()))).collect();
        Self { map }
    }

    /// The names of the values' types, sorted - for messages.
    fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.map.values().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names
    }
}

/// A component kind as it is registered: its name and the facets it
/// provides, whatever its parameters - which is how components that provide
/// a facet are found before they are built. The container checks that the
/// factory kept to it.
#[derive(Debug, Clone)]
pub struct ComponentKind {
    name: String,
    provides: Vec<TypeKey>,
    uses: Vec<TypeKey>,
}

impl ComponentKind {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), provides: Vec::new(), uses: Vec::new() }
    }

    /// Declares that components of this kind provide `T`.
    pub fn provides<T: ?Sized + 'static>(mut self) -> Self {
        self.provides.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind use the service `T` of the
    /// runtime: their factory is given it (`ComponentContext::service`), and
    /// none it does not declare (issue 296).
    pub fn uses<T: ?Sized + 'static>(mut self) -> Self {
        self.uses.push(TypeKey::of::<T>());
        self
    }
}

type ComponentFactory = Box<dyn Fn(ComponentDescriptor, ComponentContext) -> BoxFuture<'static, ComponentResult<ComponentFacets>> + Send + Sync>;

/// A registered kind.
struct Registration {
    provides: Vec<TypeKey>,
    uses: Vec<TypeKey>,
    factory: ComponentFactory,
}

/// A facet of which there is exactly one per container, with the kind that
/// is built if no component of the container provides it - if there is one
/// that makes sense.
#[derive(Debug, Clone)]
struct Single {
    facet: TypeKey,
    default_kind: Option<String>,
}

/// Reserved `kind` that resolves to a preconfigured component stored in the `ComponentStore`
/// instead of invoking a factory. `params` must deserialize to `{ "id": "<preconfigured id>" }`.
pub const PRECONFIGURED_REF_KIND: &str = "ref";

#[derive(Deserialize)]
struct PreconfiguredRef {
    id: String,
}

pub struct ComponentRegistry {
    kinds: HashMap<String, Registration>,
    singles: HashMap<TypeId, Single>,
    /// The configuration's defaults for singles (`AppConfig::component_defaults`),
    /// by the single they provide.
    defaults: HashMap<TypeId, ComponentDescriptor>,
    component_store: Arc<dyn ComponentStore>,
}

impl std::fmt::Debug for ComponentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("kind_count", &self.kinds.len())
            .field("singles", &self.singles.values().map(|s| s.facet.name).collect::<Vec<_>>())
            .finish()
    }
}

impl ComponentRegistry {
    pub fn new(component_store: Arc<dyn ComponentStore>) -> Self {
        Self {
            kinds: HashMap::new(),
            singles: HashMap::new(),
            defaults: HashMap::new(),
            component_store,
        }
    }

    /// A registry with every built-in component kind (the `register_all` of
    /// `component_impl`, `tool`, `command`, `llm`,
    /// `external`, `moderator` and `compaction`), resolving references to
    /// preconfigured components in `component_store`. A binary passes it to
    /// the `RuntimeManager` as it is; a test may register kinds of its own
    /// first, such as a stand-in for the model.
    pub fn built_in(component_store: Arc<dyn ComponentStore>) -> Self {
        let mut registry = Self::new(component_store);
        crate::component_impl::register_all(&mut registry);
        crate::tool::register_all(&mut registry);
        crate::command::register_all(&mut registry);
        crate::llm::register_all(&mut registry);
        crate::external::register_all(&mut registry);
        crate::moderator::register_all(&mut registry);
        crate::compaction::register_all(&mut registry);
        registry
    }

    /// Registers `kind` with a factory that builds without waiting.
    pub fn register<F>(&mut self, kind: ComponentKind, factory: F)
    where
        F: Fn(&ComponentDescriptor, &ComponentContext) -> ComponentResult<ComponentFacets> + Send + Sync + 'static,
    {
        let factory: ComponentFactory = Box::new(move |descriptor, context| {
            let built = factory(&descriptor, &context);
            Box::pin(std::future::ready(built))
        });
        self.kinds.insert(kind.name, Registration { provides: kind.provides, uses: kind.uses, factory });
    }

    /// Registers `kind` with a factory that waits: for a remote system, or
    /// for other components (`ComponentContext::all`, `one`).
    pub fn register_async<F, Fut>(&mut self, kind: ComponentKind, factory: F)
    where
        F: Fn(ComponentDescriptor, ComponentContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ComponentResult<ComponentFacets>> + Send + 'static,
    {
        let factory: ComponentFactory = Box::new(move |descriptor, context| Box::pin(factory(descriptor, context)));
        self.kinds.insert(kind.name, Registration { provides: kind.provides, uses: kind.uses, factory });
    }

    /// Makes `T` a facet of which there is exactly one per container, built
    /// from `default_kind` - unless the configuration names another default
    /// (`set_defaults`) - if no component of the container provides it. With
    /// no default, a container that is asked for `T` and provides none fails
    /// to build it (issue 179: a remote agent has no default).
    pub fn single<T: ?Sized + 'static>(&mut self, default_kind: Option<&str>) {
        let facet = TypeKey::of::<T>();
        self.singles.insert(facet.id, Single { facet, default_kind: default_kind.map(str::to_string) });
    }

    /// Takes the configuration's defaults (`AppConfig::component_defaults`):
    /// each of its components replaces the default kind of the one single it
    /// provides, for every container.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` naming the component if one is a
    /// `ref`, has an unknown kind, does not provide exactly one single, or
    /// provides one that another of them provides too; nothing is taken then.
    pub fn set_defaults(&mut self, config: &ComponentConfig) -> ComponentResult<()> {
        self.defaults = self.defaults_of(config)?;
        Ok(())
    }

    /// The components of `config` as defaults of singles, by the single each
    /// provides - for the whole registry (`set_defaults`) or one container
    /// (`ComponentContainer::with_defaults`).
    ///
    /// # Errors
    /// As `set_defaults`.
    fn defaults_of(&self, config: &ComponentConfig) -> ComponentResult<HashMap<TypeId, ComponentDescriptor>> {
        let mut defaults: HashMap<TypeId, ComponentDescriptor> = HashMap::new();
        for node in &config.component_nodes {
            if node.kind == PRECONFIGURED_REF_KIND {
                return Err(ComponentError::InvalidConfig(format!("component default '{}' cannot be a ref", node.id)));
            }
            let registration = self.kinds.get(&node.kind)
                .ok_or_else(|| ComponentError::InvalidConfig(format!("component default '{}' has the unknown kind '{}'", node.id, node.kind)))?;
            let singles: Vec<&Single> = registration.provides.iter().filter_map(|f| self.singles.get(&f.id)).collect();
            let [single] = singles.as_slice() else {
                return Err(ComponentError::InvalidConfig(format!(
                    "component default '{}' ({}) must provide exactly one facet of which there is one per agent, not {}",
                    node.id, node.kind, singles.len()
                )));
            };
            if let Some(other) = defaults.insert(single.facet.id, node.clone()) {
                return Err(ComponentError::InvalidConfig(format!(
                    "component defaults '{}' and '{}' both provide {}", other.id, node.id, single.facet.name
                )));
            }
        }
        Ok(defaults)
    }

    /// The kind of the component of `config` that provides `T`, a facet of
    /// which there is one - or else of the default built for it - as
    /// configured, without building anything (issue 174). `None` if `T` is
    /// no such facet, or if `config` refers to a preconfigured component
    /// (`ref`) and none of its own provides `T`: what that one provides only
    /// its descriptor in the store tells.
    pub fn kind_of<T: ?Sized + 'static>(&self, config: &ComponentConfig) -> Option<String> {
        let facet = TypeKey::of::<T>();
        if !self.singles.contains_key(&facet.id) {
            return None;
        }
        let provides = |kind: &str| self.provides(kind).is_some_and(|facets| facets.iter().any(|f| f.id == facet.id));
        if let Some(node) = config.component_nodes.iter().find(|node| provides(&node.kind)) {
            return Some(node.kind.clone());
        }
        if config.component_nodes.iter().any(|node| node.kind == PRECONFIGURED_REF_KIND) {
            return None;
        }
        self.default_of(facet.id).map(|descriptor| descriptor.kind)
    }

    /// The facets `kind` provides; `None` for a kind that is not registered.
    fn provides(&self, kind: &str) -> Option<&[TypeKey]> {
        self.kinds.get(kind).map(|registration| registration.provides.as_slice())
    }

    /// What is built for the single `facet` if no component of a container
    /// provides it: the configuration's default, or else the default kind,
    /// if it has one.
    fn default_of(&self, facet: TypeId) -> Option<ComponentDescriptor> {
        if let Some(descriptor) = self.defaults.get(&facet) {
            return Some(descriptor.clone());
        }
        let kind = self.singles.get(&facet)?.default_kind.as_deref()?;
        Some(ComponentDescriptor::new(kind, kind))
    }

    /// `node` itself, or - for `kind: "ref"` - the preconfigured components
    /// it names, in their order (issue 270), fetched from the ComponentStore
    /// fresh on every call, using the ComponentContext of whichever component
    /// graph is referencing it. This ensures things like secret resolution
    /// run under the actual caller's ComponentContext rather than a synthetic
    /// one, at the cost of not sharing a single instance across references
    /// (e.g. a preconfigured `mcp_tool` re-runs discovery for every
    /// personality that references it).
    async fn resolve(&self, node: &ComponentDescriptor, context: &ComponentContext) -> ComponentResult<Vec<ComponentDescriptor>> {
        if node.kind != PRECONFIGURED_REF_KIND {
            return Ok(vec![node.clone()]);
        }
        let target: PreconfiguredRef = serde_json::from_value(node.params.clone())
            .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
        let components = match self.component_store.get_components(&context.security_context, &target.id).await {
            Ok(stored) => stored.components,
            Err(cause @ (StoreError::NotFound { .. } | StoreError::PermissionDenied)) => return Err(ComponentError::Unavailable { id: target.id, cause }),
            Err(e) => return Err(ComponentError::InvalidConfig(format!("no preconfigured component '{}': {}", target.id, e))),
        };
        if let Some(inner) = components.component_nodes.iter().find(|inner| inner.kind == PRECONFIGURED_REF_KIND) {
            return Err(ComponentError::InvalidConfig(format!(
                "preconfigured component '{}' cannot include a ref ('{}')",
                target.id, inner.id
            )));
        }
        Ok(components.component_nodes)
    }

    fn dispatch(&self, node: ComponentDescriptor, context: ComponentContext) -> ComponentResult<BoxFuture<'static, ComponentResult<ComponentFacets>>> {
        let Some(registration) = self.kinds.get(&node.kind) else {
            return Err(ComponentError::UnsupportedKind(node.kind.clone()));
        };
        Ok((registration.factory)(node, context.for_kind(&registration.uses)))
    }
}

/// A built component.
#[derive(Debug)]
struct ComponentNode {
    id: String,
    fail_policy: ComponentFailPolicy,
    tags: Vec<String>,
    facets: ComponentFacets,
}

impl ComponentNode {
    fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<ComponentImpl<T>> {
        self.facets.get::<T>().map(|facet| ComponentImpl { id: self.id.clone(), fail_policy: self.fail_policy, facet: Arc::clone(facet) })
    }
}

/// A built component as one of its facets: the facet with the component's id
/// and fail policy. It dereferences to the facet, so that the facet's methods
/// are called on it directly; `facet` is needed only for the `Arc` itself.
#[derive(Debug, Clone)]
pub struct ComponentImpl<T>
where
    T: ?Sized + Send + Sync,
{
    pub id: String,
    pub fail_policy: ComponentFailPolicy,
    pub facet: Arc<T>,
}

impl<T> ComponentImpl<T>
where
    T: ?Sized + Send + Sync,
{
    /// `result` of a call of the facet, as the component's fail policy
    /// says: a failure fails the caller if the component fails closed,
    /// named with the component; if it fails open, it is logged and the
    /// component's part left out - `None`.
    ///
    /// # Errors
    /// The failure of a component that fails closed.
    pub fn apply_fail_policy<R>(&self, result: anyhow::Result<R>) -> anyhow::Result<Option<R>> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(e) => match self.fail_policy {
                ComponentFailPolicy::FailOpen => {
                    tracing::warn!("Component '{}' failed and is left out, as it fails open: {:#}", self.id, e);
                    Ok(None)
                }
                ComponentFailPolicy::FailClosed => Err(e.context(format!("component '{}' failed", self.id))),
            },
        }
    }
}

impl<T> Deref for ComponentImpl<T>
where
    T: ?Sized + Send + Sync,
{
    type Target = T;

    fn deref(&self) -> &T {
        &self.facet
    }
}

/// A component of a graph, as far as it has been built.
#[derive(Debug)]
enum State {
    Pending,
    Building,
    Built(Arc<ComponentNode>),
}

/// A component of a graph to be built: its descriptor, `ref`s resolved, and
/// the facets its kind provides.
#[derive(Debug)]
struct Planned {
    descriptor: ComponentDescriptor,
    provides: Vec<TypeKey>,
}

/// A component graph, built on demand: each component when it is asked for,
/// the rest in the order of the configuration.
#[derive(Debug)]
struct Graph {
    registry: Arc<ComponentRegistry>,
    /// The environment every component is built in.
    context: ComponentContext,
    planned: Vec<Planned>,
    states: Mutex<Vec<State>>,
    /// The defaults of singles for this graph alone, in place of the
    /// registry's (`ComponentContainer::with_defaults`).
    own_defaults: HashMap<TypeId, ComponentDescriptor>,
    /// The defaults built for singles no component of the graph provides.
    defaults: Mutex<HashMap<TypeId, State>>,
}

impl Graph {
    /// The planned component `index`, built if it is not yet. `asked` is who
    /// asked for which facet, for the message if it is being built.
    fn ensure(self: &Arc<Self>, index: usize, asked: Option<(&str, TypeKey)>) -> BoxFuture<'static, ComponentResult<Arc<ComponentNode>>> {
        let graph = Arc::clone(self);
        let asked = asked.map(|(component, facet)| (component.to_string(), facet));
        Box::pin(async move {
            let planned = &graph.planned[index];
            {
                let mut states = graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                match &states[index] {
                    State::Built(node) => return Ok(Arc::clone(node)),
                    State::Building => return Err(cycle(asked, &planned.descriptor.id)),
                    State::Pending => states[index] = State::Building,
                }
            }
            let node = graph.construct(&planned.descriptor, &planned.provides).await?;
            graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[index] = State::Built(Arc::clone(&node));
            Ok(node)
        })
    }

    /// Builds `descriptor` with a context through which it reaches the rest
    /// of the graph, and checks that it provides exactly `provides`.
    async fn construct(self: &Arc<Self>, descriptor: &ComponentDescriptor, provides: &[TypeKey]) -> ComponentResult<Arc<ComponentNode>> {
        let failed = |cause| ComponentError::Build { component: descriptor.id.clone(), kind: descriptor.kind.clone(), cause: Box::new(cause) };
        let context = ComponentContext { building: Some((Arc::clone(self), descriptor.id.clone())), ..self.context.clone() };
        let facets = self.registry.dispatch(descriptor.clone(), context).map_err(failed)?.await.map_err(failed)?;

        let mut declared: Vec<&'static str> = provides.iter().map(|f| f.name).collect();
        declared.sort_unstable();
        if facets.names() != declared {
            return Err(failed(ComponentError::InvalidConfig(format!(
                "provides [{}] instead of the declared [{}]", facets.names().join(", "), declared.join(", ")
            ))));
        }
        Ok(Arc::new(ComponentNode {
            id: descriptor.id.clone(),
            fail_policy: descriptor.fail_policy,
            tags: descriptor.tags.clone(),
            facets,
        }))
    }

    /// Every planned component that provides `T`, except `asking` itself.
    async fn all<T: ?Sized + Send + Sync + 'static>(self: &Arc<Self>, asking: Option<&str>) -> ComponentResult<Vec<ComponentImpl<T>>> {
        let facet = TypeKey::of::<T>();
        let mut found = Vec::new();
        for (index, planned) in self.planned.iter().enumerate() {
            if Some(planned.descriptor.id.as_str()) == asking || !planned.provides.iter().any(|f| f.id == facet.id) {
                continue;
            }
            let node = self.ensure(index, asking.map(|component| (component, facet))).await?;
            found.extend(node.get::<T>());
        }
        Ok(found)
    }

    /// The one component that provides the single `T`: the planned one, or
    /// the default.
    async fn one<T: ?Sized + Send + Sync + 'static>(self: &Arc<Self>, asking: Option<&str>) -> ComponentResult<ComponentImpl<T>> {
        let facet = TypeKey::of::<T>();
        let not_single = || ComponentError::InvalidConfig(format!("{} is not a facet of which there is one", facet.name));
        if !self.registry.singles.contains_key(&facet.id) {
            return Err(not_single());
        }
        let asked = asking.map(|component| (component, facet));
        let providers: Vec<usize> = self.planned.iter().enumerate()
            .filter(|(_, planned)| planned.provides.iter().any(|f| f.id == facet.id))
            .map(|(index, _)| index)
            .collect();
        let node = match providers.as_slice() {
            [index] => self.ensure(*index, asked).await?,
            // `from_config` refuses a graph with two.
            [] => self.default(facet, asked).await?,
            _ => return Err(not_single()),
        };
        node.get::<T>().ok_or_else(not_single)
    }

    /// The default for the single `facet`, built if it is not yet.
    async fn default(self: &Arc<Self>, facet: TypeKey, asked: Option<(&str, TypeKey)>) -> ComponentResult<Arc<ComponentNode>> {
        let descriptor = self.own_defaults.get(&facet.id).cloned()
            .or_else(|| self.registry.default_of(facet.id))
            .ok_or_else(|| ComponentError::InvalidConfig(format!("no component provides {}, and it has no default", facet.name)))?;
        {
            let mut defaults = self.defaults.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match defaults.get(&facet.id) {
                Some(State::Built(node)) => return Ok(Arc::clone(node)),
                Some(State::Building) => return Err(cycle(asked.map(|(c, f)| (c.to_string(), f)), &descriptor.id)),
                Some(State::Pending) | None => { defaults.insert(facet.id, State::Building); }
            }
        }
        let provides = self.registry.provides(&descriptor.kind).unwrap_or_default().to_vec();
        if !provides.iter().any(|f| f.id == facet.id) {
            // A kind registered in code under a name that is wrong, or
            // registered without the facet: a mistake no configuration can
            // make, found the first time the default is needed.
            return Err(ComponentError::InvalidConfig(format!(
                "the default kind '{}' of {} is not registered as providing it", descriptor.kind, facet.name
            )));
        }
        let node = self.construct(&descriptor, &provides).await?;
        self.defaults.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(facet.id, State::Built(Arc::clone(&node)));
        Ok(node)
    }
}

/// The error for asking for a component that is being built.
fn cycle(asked: Option<(String, TypeKey)>, other: &str) -> ComponentError {
    match asked {
        Some((component, facet)) => ComponentError::Cycle { component, facet: facet.name, other: other.to_string() },
        // Only a component being built asks for another while the graph is
        // built; anything else waits for it.
        None => ComponentError::InvalidConfig(format!("component '{other}' is being built")),
    }
}

/// The components of one configuration, built.
#[derive(Debug)]
pub struct ComponentContainer {
    graph: Arc<Graph>,
}

impl ComponentContainer {
    /// Builds the components of `config` with the factories of `registry`,
    /// each with `context` - a component asked for by another first, the
    /// rest in the order of the configuration.
    ///
    /// A `ref` stands for the preconfigured components it names, in their
    /// order, where it is written (issue 270).
    ///
    /// # Errors
    /// `ComponentError::DuplicateId` if two entries of `config` share an id,
    /// or two of the components they stand for do - wrapped with the `ref`
    /// that brings the second -; `ComponentError::InvalidConfig` if two
    /// provide a facet of which there is one; whatever resolving a `ref` or
    /// building a component reports, wrapped with its id and kind.
    pub async fn from_config(
        config: &ComponentConfig,
        registry: &Arc<ComponentRegistry>,
        context: &ComponentContext,
    ) -> ComponentResult<Self> {
        Self::with_defaults(config, &ComponentConfig::default(), registry, context).await
    }

    /// Builds the components of `config` as `from_config` does, with the
    /// components of `defaults` as the defaults of the singles they provide,
    /// in place of the registry's, for this container alone: for a kind of
    /// owner whose singles want another default, such as an external agent's
    /// `ContextProvider` (issue 214).
    ///
    /// # Errors
    /// As `from_config`; and as `ComponentRegistry::set_defaults` for
    /// `defaults`.
    pub async fn with_defaults(
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        registry: &Arc<ComponentRegistry>,
        context: &ComponentContext,
    ) -> ComponentResult<Self> {
        let own_defaults = registry.defaults_of(defaults)?;
        let mut entries = HashSet::new();
        // The entry of `config` each component's id came from.
        let mut ids: HashMap<String, &ComponentDescriptor> = HashMap::new();
        let mut planned = Vec::with_capacity(config.component_nodes.len());
        for node in &config.component_nodes {
            if !entries.insert(node.id.as_str()) {
                return Err(ComponentError::DuplicateId(node.id.clone()));
            }
            let failed = |entry: &ComponentDescriptor, cause| ComponentError::Build { component: entry.id.clone(), kind: entry.kind.clone(), cause: Box::new(cause) };
            for descriptor in registry.resolve(node, context).await.map_err(|cause| failed(node, cause))? {
                if let Some(earlier) = ids.insert(descriptor.id.clone(), node) {
                    // Two plain entries of one id are caught above: at least
                    // one of the two is a ref, and it is named.
                    let entry = if node.kind == PRECONFIGURED_REF_KIND { node } else { earlier };
                    return Err(failed(entry, ComponentError::DuplicateId(descriptor.id)));
                }
                let provides = registry.provides(&descriptor.kind)
                    .ok_or_else(|| failed(node, ComponentError::UnsupportedKind(descriptor.kind.clone())))?
                    .to_vec();
                planned.push(Planned { descriptor, provides });
            }
        }
        for single in registry.singles.values() {
            let providers: Vec<&str> = planned.iter()
                .filter(|p| p.provides.iter().any(|f| f.id == single.facet.id))
                .map(|p| p.descriptor.id.as_str())
                .collect();
            if let [first, second, ..] = providers.as_slice() {
                return Err(ComponentError::InvalidConfig(format!(
                    "components '{first}' and '{second}' both provide {}, of which there is one", single.facet.name
                )));
            }
        }

        let graph = Arc::new(Graph {
            registry: Arc::clone(registry),
            context: ComponentContext { building: None, ..context.clone() },
            states: Mutex::new(planned.iter().map(|_| State::Pending).collect()),
            planned,
            own_defaults,
            defaults: Mutex::new(HashMap::new()),
        });
        for index in 0..graph.planned.len() {
            graph.ensure(index, None).await?;
        }
        Ok(Self { graph })
    }

    /// What it was built from: its configuration with every `ref` resolved
    /// to the preconfigured components it stood for (issue 270), in their
    /// order. The defaults built for singles it does not configure are not
    /// among them.
    pub fn components(&self) -> ComponentConfig {
        ComponentConfig { component_nodes: self.graph.planned.iter().map(|planned| planned.descriptor.clone()).collect() }
    }

    /// Every component that provides `T` - with `tag`, if one is given - in
    /// the order of the configuration.
    pub fn get_all<T>(&self, tag: Option<&str>) -> Vec<ComponentImpl<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let states = self.graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        states.iter()
            .filter_map(|state| match state {
                State::Built(node) => Some(node),
                _ => None,
            })
            .filter(|node| tag.is_none_or(|tag| node.tags.iter().any(|t| t == tag)))
            .filter_map(|node| node.get::<T>())
            .collect()
    }

    /// The one component that provides `T`, a facet of which there is
    /// exactly one: the container's, or the default - built when it is first
    /// asked for.
    ///
    /// # Errors
    /// Whatever building the default reports; `ComponentError::InvalidConfig`
    /// if `T` is not a facet of which there is one.
    pub async fn one<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<ComponentImpl<T>> {
        self.graph.one::<T>(None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::component::StoredComponents;
    use caelumora_model::security::{DummySecurityManager, SecurityManager};
    use caelumora_model::session::SessionId;
    use caelumora_model::store::memory::MemoryStore;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    trait Marker: std::fmt::Debug + Send + Sync {}

    #[derive(Debug)]
    struct MarkerImpl;
    impl Marker for MarkerImpl {}

    fn test_context() -> ComponentContext {
        context_with_session(SessionId::default())
    }

    fn context_with_session(session_id: SessionId) -> ComponentContext {
        let services = ComponentServices::empty().with(Arc::new(DummySecurityManager::new()) as Arc<dyn SecurityManager>);
        ComponentContext::new(SecurityContext { session_id, ..SecurityContext::default() }, services)
    }

    fn descriptor(id: &str, kind: &str, params: serde_json::Value) -> ComponentDescriptor {
        ComponentDescriptor {
            id: id.to_string(),
            kind: kind.to_string(),
            params,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }
    }

    /// A store holding the preconfigured components `id`, the list `nodes`.
    async fn store_with(preconfigured: &[(&str, &[(&str, &str)])]) -> Arc<MemoryStore> {
        let store = Arc::new(MemoryStore::new());
        for (id, nodes) in preconfigured {
            store.create_components(&SecurityContext::default(), &StoredComponents::new(id, config(nodes), Default::default())).await.unwrap();
        }
        store
    }

    fn reference(id: &str, target: &str) -> ComponentDescriptor {
        descriptor(id, PRECONFIGURED_REF_KIND, json!({ "id": target }))
    }

    #[tokio::test]
    async fn ref_kind_builds_fresh_using_the_callers_context_each_time() {
        let build_count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&build_count);
        let seen_sessions: Arc<Mutex<Vec<SessionId>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen_sessions);

        let store = store_with(&[("shared-marker", &[("marker", "counter")])]).await;
        let mut registry = ComponentRegistry::new(store);
        registry.register(
            ComponentKind::new("counter").provides::<dyn Marker>(),
            move |_node, context| {
                counted.fetch_add(1, Ordering::SeqCst);
                recorded.lock().unwrap().push(context.security_context.session_id);
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Marker>(Arc::new(MarkerImpl));
                Ok(facets)
            },
        );
        let registry = Arc::new(registry);
        let config = ComponentConfig { component_nodes: vec![reference("ref", "shared-marker")] };

        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let container_a = ComponentContainer::from_config(&config, &registry, &context_with_session(session_a)).await.unwrap();
        let container_b = ComponentContainer::from_config(&config, &registry, &context_with_session(session_b)).await.unwrap();

        assert_eq!(build_count.load(Ordering::SeqCst), 2, "each ref must build fresh, not reuse a cached instance");
        assert_eq!(*seen_sessions.lock().unwrap(), vec![session_a, session_b], "the factory must see the caller's own context, not a synthetic one");
        assert!(!Arc::ptr_eq(&container_a.get_all::<dyn Marker>(None)[0].facet, &container_b.get_all::<dyn Marker>(None)[0].facet));
    }

    /// A registry of the kind `named`, whose components are named by their
    /// id, resolving references in `store`.
    fn named_registry(store: Arc<MemoryStore>) -> Arc<ComponentRegistry> {
        let mut registry = ComponentRegistry::new(store);
        registry.register(ComponentKind::new("named").provides::<dyn Named>(), |node, _| Ok(named(&node.id)));
        Arc::new(registry)
    }

    fn names(container: &ComponentContainer) -> Vec<String> {
        container.get_all::<dyn Named>(None).iter().map(|c| c.name()).collect()
    }

    #[tokio::test]
    async fn a_ref_stands_for_its_components_in_their_order_where_it_is_written() {
        let store = store_with(&[("basics", &[("stop", "named"), ("notes", "named")])]).await;
        let registry = named_registry(store);
        let config = ComponentConfig {
            component_nodes: vec![descriptor("prompt", "named", json!(null)), reference("basics", "basics"), descriptor("memory", "named", json!(null))],
        };

        let container = ComponentContainer::from_config(&config, &registry, &test_context()).await.unwrap();
        assert_eq!(names(&container), ["prompt", "stop", "notes", "memory"]);
        let built_from: Vec<String> = container.components().component_nodes.into_iter().map(|node| node.id).collect();
        assert_eq!(built_from, ["prompt", "stop", "notes", "memory"], "what it was built from, resolved");
    }

    #[tokio::test]
    async fn a_component_brought_twice_through_a_ref_is_refused_with_the_ref_named() {
        let store = store_with(&[("basics", &[("stop", "named"), ("notes", "named")]), ("more", &[("notes", "named")])]).await;
        let registry = named_registry(store);
        let notes = || descriptor("notes", "named", json!(null));

        for (nodes, named) in [
            (vec![reference("basics", "basics"), notes()], "basics"),
            (vec![notes(), reference("basics", "basics")], "basics"),
            (vec![reference("basics", "basics"), reference("more", "more")], "more"),
            (vec![reference("basics", "basics"), reference("again", "basics")], "again"),
        ] {
            let error = ComponentContainer::from_config(&ComponentConfig { component_nodes: nodes }, &registry, &test_context()).await.unwrap_err();
            assert!(
                matches!(&error, ComponentError::Build { component, cause, .. } if component == named && matches!(cause.as_ref(), ComponentError::DuplicateId(_))),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn a_ref_to_nothing_stored_is_refused_as_unavailable() {
        let registry = named_registry(Arc::new(MemoryStore::new()));
        let config = ComponentConfig { component_nodes: vec![reference("ref-a", "does-not-exist")] };
        let error = ComponentContainer::from_config(&config, &registry, &test_context()).await.unwrap_err();
        assert!(matches!(causes(&error)[..], [ComponentError::Build { .. }, ComponentError::Unavailable { .. }]), "{error}");
        assert!(error.is_unavailable(), "through the component it was building");
        assert!(error.to_string().contains("'does-not-exist'"), "names what it could not read: {error}");
    }

    #[tokio::test]
    async fn preconfigured_components_cannot_include_a_ref() {
        let store = Arc::new(MemoryStore::new());
        let nested = ComponentConfig { component_nodes: vec![descriptor("plain", "named", json!(null)), reference("inner", "b")] };
        store.create_components(&SecurityContext::default(), &StoredComponents::new("a", nested, Default::default())).await.unwrap();
        let registry = named_registry(store);
        let config = ComponentConfig { component_nodes: vec![reference("ref-a", "a")] };
        let error = ComponentContainer::from_config(&config, &registry, &test_context()).await.unwrap_err();
        assert!(matches!(causes(&error)[..], [ComponentError::Build { .. }, ComponentError::InvalidConfig(_)]), "{error}");
        assert!(error.to_string().contains("'inner'"), "names the ref within: {error}");
    }

    /// A facet that says which component it came from.
    trait Named: std::fmt::Debug + Send + Sync {
        fn name(&self) -> String;
    }

    #[derive(Debug)]
    struct Name(String);
    impl Named for Name {
        fn name(&self) -> String {
            self.0.clone()
        }
    }

    /// A second facet, for components that need each other.
    trait Other: std::fmt::Debug + Send + Sync {}
    impl Other for Name {}

    /// A facet of which there is one.
    trait Choice: std::fmt::Debug + Send + Sync {}
    impl Choice for Name {}

    fn named(id: &str) -> ComponentFacets {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn Named>(Arc::new(Name(id.to_string())));
        facets
    }

    fn config(nodes: &[(&str, &str)]) -> ComponentConfig {
        ComponentConfig { component_nodes: nodes.iter().map(|(id, kind)| descriptor(id, kind, json!(null))).collect() }
    }

    /// The component errors from the outermost to the innermost.
    fn causes(error: &ComponentError) -> Vec<&ComponentError> {
        let mut chain = vec![error];
        while let ComponentError::Build { cause, .. } = chain[chain.len() - 1] {
            chain.push(cause);
        }
        chain
    }

    #[tokio::test]
    async fn a_component_gets_every_other_that_provides_what_it_asks_for_in_the_order_of_the_configuration() {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        registry.register(ComponentKind::new("part").provides::<dyn Named>(), |descriptor, _| Ok(named(&descriptor.id)));
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        // Provides what it collects, so that it has to leave itself out.
        registry.register_async(ComponentKind::new("collector").provides::<dyn Named>(), move |descriptor, context| {
            let recorded = Arc::clone(&recorded);
            async move {
                let parts = context.all::<dyn Named>().await?;
                recorded.lock().unwrap().extend(parts.iter().map(|part| part.name()));
                Ok(named(&descriptor.id))
            }
        });
        let registry = Arc::new(registry);

        // Configured before what it collects, which is built for it first.
        let container = ComponentContainer::from_config(&config(&[("collector", "collector"), ("a", "part"), ("b", "part")]), &registry, &test_context())
            .await.expect("builds");

        assert_eq!(*seen.lock().unwrap(), ["a", "b"], "every other component with the facet, in the order of the configuration");
        let all: Vec<_> = container.get_all::<dyn Named>(None).iter().map(|c| c.id.clone()).collect();
        assert_eq!(all, ["collector", "a", "b"], "the container keeps the order of the configuration");
    }

    #[tokio::test]
    async fn components_that_need_each_other_are_refused_naming_both() {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        registry.register_async(ComponentKind::new("chicken").provides::<dyn Named>(), |descriptor, context| async move {
            context.all::<dyn Other>().await?;
            Ok(named(&descriptor.id))
        });
        registry.register_async(ComponentKind::new("egg").provides::<dyn Other>(), |descriptor, context| async move {
            context.all::<dyn Named>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn Other>(Arc::new(Name(descriptor.id.clone())));
            Ok(facets)
        });
        let registry = Arc::new(registry);

        let error = ComponentContainer::from_config(&config(&[("hen", "chicken"), ("egg", "egg")]), &registry, &test_context())
            .await.expect_err("they need each other");

        let innermost = causes(&error).last().copied().unwrap();
        assert!(matches!(innermost, ComponentError::Cycle { component, facet: "Named", other } if component == "egg" && other == "hen"), "{error}");
    }

    #[tokio::test]
    async fn a_component_must_provide_what_its_kind_declares() {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        registry.register(ComponentKind::new("liar").provides::<dyn Named>(), |_, _| Ok(ComponentFacets::empty()));
        let registry = Arc::new(registry);

        let error = ComponentContainer::from_config(&config(&[("liar", "liar")]), &registry, &test_context())
            .await.expect_err("provides nothing");

        let message = error.to_string();
        assert!(message.contains("'liar'") && message.contains("instead of the declared [Named]"), "{message}");
    }

    /// A kind is given the services it declares it uses (issue 296), and
    /// none it does not: the context of the graph has the policy, the one of
    /// `blind` does not.
    #[tokio::test]
    async fn a_component_is_given_the_services_its_kind_uses_alone() {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        registry.register(ComponentKind::new("seeing").provides::<dyn Named>().uses::<dyn SecurityManager>(), |node, context| {
            context.service::<dyn SecurityManager>()?;
            Ok(named(&node.id))
        });
        registry.register(ComponentKind::new("blind").provides::<dyn Named>(), |node, context| {
            context.service::<dyn SecurityManager>()?;
            Ok(named(&node.id))
        });
        let registry = Arc::new(registry);

        ComponentContainer::from_config(&config(&[("seeing", "seeing")]), &registry, &test_context()).await.expect("given what it uses");
        let error = ComponentContainer::from_config(&config(&[("blind", "blind")]), &registry, &test_context())
            .await.expect_err("not what it does not declare");
        assert!(causes(&error).iter().any(|e| matches!(e, ComponentError::NoService("SecurityManager"))), "{error}");
    }

    /// A registry with the single `Choice`, the kinds `default` and `other`
    /// that provide it, and how often `default` was built.
    fn choices() -> (ComponentRegistry, Arc<AtomicUsize>) {
        let mut registry = ComponentRegistry::new(Arc::new(MemoryStore::new()));
        let built = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&built);
        let choice = |id: &str| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn Choice>(Arc::new(Name(id.to_string())));
            facets
        };
        registry.register(ComponentKind::new("default").provides::<dyn Choice>(), move |descriptor, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(choice(&descriptor.id))
        });
        registry.register(ComponentKind::new("other").provides::<dyn Choice>(), move |descriptor, _| Ok(choice(&descriptor.id)));
        registry.register(ComponentKind::new("part").provides::<dyn Named>(), |descriptor, _| Ok(named(&descriptor.id)));
        registry.single::<dyn Choice>(Some("default"));
        (registry, built)
    }

    #[tokio::test]
    async fn of_a_single_facet_there_is_the_configured_one_or_the_default_built_when_first_asked_for() {
        let (registry, built) = choices();
        let registry = Arc::new(registry);

        let without = ComponentContainer::from_config(&config(&[("a", "part")]), &registry, &test_context()).await.unwrap();
        assert_eq!(built.load(Ordering::SeqCst), 0, "nobody asked for it yet");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "default");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "default");
        assert_eq!(built.load(Ordering::SeqCst), 1, "built once");

        let with = ComponentContainer::from_config(&config(&[("mine", "other")]), &registry, &test_context()).await.unwrap();
        assert_eq!(with.one::<dyn Choice>().await.unwrap().id, "mine", "the configured one replaces the default");

        let two = ComponentContainer::from_config(&config(&[("mine", "other"), ("yours", "other")]), &registry, &test_context()).await;
        let message = two.expect_err("there is one").to_string();
        assert!(message.contains("'mine' and 'yours' both provide Choice"), "{message}");

        let not_single = without.one::<dyn Named>().await.expect_err("of Named there may be any number");
        assert!(matches!(not_single, ComponentError::InvalidConfig(_)), "{not_single}");
    }

    #[tokio::test]
    async fn the_configuration_replaces_a_default_for_every_container() {
        let (mut registry, built) = choices();
        registry.set_defaults(&config(&[("configured", "other")])).expect("replaces the default of Choice");
        let registry = Arc::new(registry);

        let container = ComponentContainer::from_config(&config(&[]), &registry, &test_context()).await.unwrap();
        assert_eq!(container.one::<dyn Choice>().await.unwrap().id, "configured");
        assert_eq!(built.load(Ordering::SeqCst), 0, "the built-in default is not built");
    }

    #[tokio::test]
    async fn a_container_may_have_defaults_of_its_own_in_place_of_the_registrys() {
        let (mut registry, built) = choices();
        registry.set_defaults(&config(&[("configured", "default")])).unwrap();
        let registry = Arc::new(registry);

        let own = ComponentContainer::with_defaults(&config(&[]), &config(&[("its_own", "other")]), &registry, &test_context()).await.unwrap();
        assert_eq!(own.one::<dyn Choice>().await.unwrap().id, "its_own", "in place of the configuration's too");
        let mine = ComponentContainer::with_defaults(&config(&[("mine", "other")]), &config(&[("its_own", "other")]), &registry, &test_context()).await.unwrap();
        assert_eq!(mine.one::<dyn Choice>().await.unwrap().id, "mine", "what the container configures still comes first");
        assert_eq!(built.load(Ordering::SeqCst), 0, "no other default is built");

        let refused = ComponentContainer::with_defaults(&config(&[]), &config(&[("x", "part")]), &registry, &test_context()).await;
        assert!(matches!(refused, Err(ComponentError::InvalidConfig(_))), "a default must provide one single");
    }

    #[test]
    fn the_kind_of_a_single_is_told_from_the_configuration_without_building_it() {
        let (mut registry, built) = choices();
        assert_eq!(registry.kind_of::<dyn Choice>(&config(&[("a", "part"), ("mine", "other")])).as_deref(), Some("other"));
        assert_eq!(registry.kind_of::<dyn Choice>(&config(&[("a", "part")])).as_deref(), Some("default"), "the default if none provides it");
        assert_eq!(registry.kind_of::<dyn Choice>(&config(&[("shared", PRECONFIGURED_REF_KIND)])), None, "a preconfigured one might provide it");
        assert_eq!(registry.kind_of::<dyn Named>(&config(&[("a", "part")])), None, "of Named there may be any number");
        registry.set_defaults(&config(&[("configured", "other")])).unwrap();
        assert_eq!(registry.kind_of::<dyn Choice>(&config(&[])).as_deref(), Some("other"), "the configured default");
        assert_eq!(built.load(Ordering::SeqCst), 0, "nothing is built");
    }

    #[test]
    fn a_default_the_configuration_cannot_use_is_refused() {
        let (mut registry, _) = choices();
        for (nodes, expected) in [
            (config(&[("x", PRECONFIGURED_REF_KIND)]), "cannot be a ref"),
            (config(&[("x", "unknown")]), "unknown kind 'unknown'"),
            (config(&[("x", "part")]), "exactly one facet"),
            (config(&[("x", "other"), ("y", "default")]), "'x' and 'y' both provide Choice"),
        ] {
            let message = registry.set_defaults(&nodes).expect_err(expected).to_string();
            assert!(message.contains(expected), "{message}");
        }
    }

    #[tokio::test]
    async fn a_single_without_a_usable_default_fails_where_it_is_asked_for() {
        let (mut registry, _) = choices();
        registry.single::<dyn Named>(None);
        let registry = Arc::new(registry);
        let built = ComponentContainer::from_config(&config(&[]), &registry, &test_context()).await.expect("nobody asked for it");
        let message = built.one::<dyn Named>().await.expect_err("there is none").to_string();
        assert!(message.contains("no component provides Named, and it has no default"), "{message}");
        assert_eq!(registry.kind_of::<dyn Named>(&config(&[])), None, "no kind to tell");
        let provided = ComponentContainer::from_config(&config(&[("a", "part")]), &registry, &test_context()).await.unwrap();
        assert_eq!(provided.one::<dyn Named>().await.expect("the container's own").id, "a");

        let (mut registry, _) = choices();
        registry.single::<dyn Named>(Some("missing"));
        let registry = Arc::new(registry);
        let built = ComponentContainer::from_config(&config(&[]), &registry, &test_context()).await.unwrap();
        let message = built.one::<dyn Named>().await.expect_err("a default kind that is not registered").to_string();
        assert!(message.contains("the default kind 'missing' of Named is not registered as providing it"), "{message}");
    }

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

