//! What a library declares and is given of components (issues 99, 516): a kind as it is registered
//! (`ComponentKind`) with its factory and the parameters it takes (`Params`), what a component is
//! built with (`ComponentContext`), the graph the kernel builds (`ComponentGraph`) and what its
//! owner holds of it (`ComponentContainer`), a built component as one of its facets
//! (`ComponentImpl`), and what building one fails with (`ComponentError`). How kinds are
//! registered, a configuration is planned and a graph is built is the kernel's
//! (`caelumora_kernel::component`).

use crate::remote::describe_chain;
use async_trait::async_trait;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::prompt_catalogue::PromptError;
use caelumora_model::secret::file::FileSecretConfig;
use caelumora_model::store::StoreError;
use caelumora_model::type_map::{TypeKey, TypeMap};
use caelumora_model::user_provider::accept_any::AcceptAnyConfig;
use caelumora_model::user_provider::file::FileUserConfig;
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::error::Error;
use std::fmt::{self, Debug};
use std::future::{self, Future};
use std::ops::Deref;
use std::sync::Arc;

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
    /// A component asked for a service of the kernel its kind does not declare it uses, or one the
    /// kernel does not have (issue 296).
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
    /// The error for `facet`, asked for as one of which there is one, which it is not.
    pub fn not_single(facet: TypeKey) -> Self {
        Self::InvalidConfig(format!("{} is not a facet of which there is one", facet.name()))
    }

    /// `cause`, as what went wrong with the component `entry` (`Build`).
    pub fn of_component(entry: &ComponentDescriptor, cause: ComponentError) -> Self {
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

/// What a component kind or a backend type takes as its parameters (issue 515): a struct
/// `#[serde(deny_unknown_fields)]`, read from its params by `config_of_params` - which refuses a
/// parameter it does not know - and checked there beyond the types of its fields.
pub trait Params: DeserializeOwned {
    /// Whether the values go together - a share within its bounds, a number at least 1 -, beyond
    /// what the types of the fields say; nothing to check unless its kind or type says otherwise.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` naming the parameter that is not what it should be.
    fn check(&self) -> ComponentResult<()> {
        Ok(())
    }
}

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
/// read by the order of `T`'s fields -, a parameter is `null`, they are
/// not a `T`, naming the parameter that is not what it should be, where it
/// is one, or `T` finds them wrong (`Params::check`).
pub fn config_of_params<T: Params>(params: &Value) -> ComponentResult<T> {
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
    let config: T = serde_path_to_error::deserialize(params).map_err(|e| {
        let path = e.path().to_string();
        let message = match path.as_str() {
            "." => format!("invalid parameters: {}", e.inner()),
            parameter => format!("invalid parameter '{parameter}': {}", e.inner()),
        };
        ComponentError::InvalidConfig(message)
    })?;
    config.check()?;
    Ok(config)
}

/// The parameters of what takes none - a component kind that declares none
/// (`ComponentKind::params`), a backend: an object without fields, so that a misspelt one is
/// refused rather than ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

impl Params for NoParams {}

// The parameters of the kernel's backends whose types the model defines, which knows nothing of
// `Params`: implemented here, where the trait is, as neither lies in the kernel.
impl Params for FileSecretConfig {}
impl Params for FileUserConfig {}
impl Params for AcceptAnyConfig {}

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
    /// says: a failure fails the caller if the component fails closed - the
    /// caller names it with the component, in the error type of the facet's
    /// caller (`ContextError::Component`); if it fails open, it is logged
    /// and the component's part left out - `None`.
    ///
    /// # Errors
    /// The failure of a component that fails closed.
    pub fn apply_fail_policy<R, E: Error + 'static>(
        &self,
        result: Result<R, E>,
    ) -> Result<Option<R>, E> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(e) => match self.fail_policy {
                ComponentFailPolicy::FailOpen => {
                    tracing::warn!(
                        "Component '{}' failed and is left out, as it fails open: {}",
                        self.id,
                        describe_chain(&e)
                    );
                    Ok(None)
                }
                ComponentFailPolicy::FailClosed => Err(e),
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

/// The facets a component provides, by type.
pub type ComponentFacets = TypeMap;

/// The services of the kernel, by type (issues 296, 323): the policy, the secrets, the managers of
/// the kernel that components use and what the plugins and the libraries provide. A component is
/// given those its kind uses (`ComponentKind::uses`), the views and a plugin's routes reach them
/// all.
pub type Services = TypeMap;

/// A component kind as it is registered: its name, the parameters it takes, the facets it provides,
/// the services it uses, the facets of others it needs and the extensions it reads and writes by -
/// which is how a component's params are checked, components that provide a facet are found, what a
/// container accepts is known, and a kernel that lacks an extension is refused, before anything is
/// built. The container checks that the factory kept to it.
#[derive(Debug, Clone)]
pub struct ComponentKind {
    name: String,
    params: ParamsCheck,
    provides: Vec<TypeKey>,
    uses: Vec<TypeKey>,
    needs: Vec<TypeKey>,
    extensions: Vec<(Face, TypeKey)>,
    operator_only: bool,
}

/// How the params of a component of a kind are checked: read as the configuration the kind
/// declared (`ComponentKind::params`), which checks its values (`Params::check`).
type ParamsCheck = fn(&Value) -> ComponentResult<()>;

/// Whether `params` are a `C` whose values it finds right.
fn check_params<C: Params>(params: &Value) -> ComponentResult<()> {
    config_of_params::<C>(params).map(drop)
}

impl ComponentKind {
    /// What the configuration names it by.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether `params` are those of one of its components, read as it declared them and checked
    /// (`params`).
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig`, naming the parameter, if they are not.
    pub fn check_params(&self, params: &Value) -> ComponentResult<()> {
        (self.params)(params)
    }

    /// The facets it provides (`provides`).
    pub fn provided(&self) -> &[TypeKey] {
        &self.provides
    }

    /// The services of the kernel it uses (`uses`).
    pub fn used(&self) -> &[TypeKey] {
        &self.uses
    }

    /// The facets of other components it needs (`needs`).
    pub fn needed(&self) -> &[TypeKey] {
        &self.needs
    }

    /// The extensions it reads and writes by (`reads`, `writes`).
    pub fn extensions(&self) -> &[(Face, TypeKey)] {
        &self.extensions
    }

    /// Whether only an operator configures it (`operator_only`).
    pub fn is_operator_only(&self) -> bool {
        self.operator_only
    }

    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            params: check_params::<NoParams>,
            provides: Vec::new(),
            uses: Vec::new(),
            needs: Vec::new(),
            extensions: Vec::new(),
            operator_only: false,
        }
    }

    /// Declares that components of this kind are configured by an operator
    /// alone (issue 403): their params are trusted - where they send what
    /// they are given, what that is cleared for -, so they are written only
    /// where an operator writes, in the templates, personalities and
    /// component bundles of the seed. A request names one only
    /// through a `ref` (`require_acceptable`).
    pub fn operator_only(mut self) -> Self {
        self.operator_only = true;
        self
    }

    /// Declares that components of this kind take the parameters `C` (`Params`), which their
    /// factory reads with `config_of_params`. A kind that declares none takes none. The registry
    /// checks a component's params before anything is built.
    pub fn params<C: Params>(mut self) -> Self {
        self.params = check_params::<C>;
        self
    }

    /// Declares that components of this kind provide `T`.
    pub fn provides<T: ?Sized + 'static>(mut self) -> Self {
        self.provides.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind use the service `T` of the kernel: their factory is
    /// given it (`ComponentContext::service`), and none it does not declare (issue 296).
    pub fn uses<T: ?Sized + 'static>(mut self) -> Self {
        self.uses.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind need the facet `T` of others:
    /// their factory may ask for it (`ComponentContext::all`, `one`), and
    /// for none it does not declare; and a container that accepts them
    /// accepts components that provide it (issue 337).
    pub fn needs<T: ?Sized + 'static>(mut self) -> Self {
        self.needs.push(TypeKey::of::<T>());
        self
    }

    /// Declares that components of this kind read by the reader `T` a plugin binds to the work they
    /// serve (`KernelBuilder::extend_readers`, issue 516): the kernel is not built where no plugin
    /// binds it.
    pub fn reads<T: ?Sized + 'static>(mut self) -> Self {
        self.extensions.push((Face::Reader, TypeKey::of::<T>()));
        self
    }

    /// Declares that components of this kind act by the writer `T` a plugin binds to the work they
    /// serve (`KernelBuilder::extend_writers`), as `reads` declares a reader.
    pub fn writes<T: ?Sized + 'static>(mut self) -> Self {
        self.extensions.push((Face::Writer, TypeKey::of::<T>()));
        self
    }
}

/// What builds a component of a kind: from its entry of the configuration and the context it is
/// given, its facets - waiting, if it needs to, for a remote system or for other components.
pub type ComponentFactory = Box<
    dyn Fn(
            ComponentDescriptor,
            ComponentContext,
        ) -> BoxFuture<'static, ComponentResult<ComponentFacets>>
        + Send
        + Sync,
>;

/// The factory of `build`, which builds without waiting.
pub fn factory<F>(build: F) -> ComponentFactory
where
    F: Fn(&ComponentDescriptor, &ComponentContext) -> ComponentResult<ComponentFacets>
        + Send
        + Sync
        + 'static,
{
    Box::new(move |descriptor, context| {
        let built = build(&descriptor, &context);
        Box::pin(future::ready(built))
    })
}

/// The factory of `build`, which waits: for a remote system, or for other components
/// (`ComponentContext::all`, `one`).
pub fn async_factory<F, Fut>(build: F) -> ComponentFactory
where
    F: Fn(ComponentDescriptor, ComponentContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ComponentResult<ComponentFacets>> + Send + 'static,
{
    Box::new(move |descriptor, context| Box::pin(build(descriptor, context)))
}

/// The kinds of components a library registers with (issue 516): the kernel's registry, which
/// marks what is registered through it as a library's, so that the kernel is not built where one
/// provides a facet the kernel builds in its own name or uses a service of the kernel.
pub trait ComponentKinds {
    /// Registers `kind`, built by `factory`, as a library's.
    fn register(&mut self, kind: ComponentKind, factory: ComponentFactory);

    /// Makes `facet` one of which there is exactly one per container, built from `default_kind`
    /// where no component provides it, as a library declares it.
    fn single_of(&mut self, facet: TypeKey, default_kind: Option<&str>);
}

/// The component kinds a library registers (issue 516), typed over the kernel's registration
/// (`ComponentKinds`). A library is handed it, never the registry (`LibraryBuilder::components`).
pub struct LibraryComponents<'a> {
    kinds: Box<dyn ComponentKinds + 'a>,
}

impl<'a> LibraryComponents<'a> {
    /// The kinds a library registers through `kinds`.
    pub fn new(kinds: Box<dyn ComponentKinds + 'a>) -> Self {
        Self { kinds }
    }

    /// Registers `kind`, a library's, with a factory that builds without waiting.
    pub fn register<F>(&mut self, kind: ComponentKind, build: F)
    where
        F: Fn(&ComponentDescriptor, &ComponentContext) -> ComponentResult<ComponentFacets>
            + Send
            + Sync
            + 'static,
    {
        self.kinds.register(kind, factory(build));
    }

    /// Registers `kind`, a library's, with a factory that waits.
    pub fn register_async<F, Fut>(&mut self, kind: ComponentKind, build: F)
    where
        F: Fn(ComponentDescriptor, ComponentContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ComponentResult<ComponentFacets>> + Send + 'static,
    {
        self.kinds.register(kind, async_factory(build));
    }

    /// Makes `T` a facet of which there is exactly one per container, declared by a library.
    pub fn single<T: ?Sized + 'static>(&mut self, default_kind: Option<&str>) {
        self.kinds.single_of(TypeKey::of::<T>(), default_kind);
    }
}

/// Which face of a service an extension is: what a work reads by, handed out where it reads
/// (`Situation`), or what it acts by, handed out where it acts (`Turn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Reader,
    Writer,
}

impl fmt::Display for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        })
    }
}

/// The service `T` of `services`, for what is made with them: a component
/// (`ComponentContext::service`), a backend (`Backends`).
///
/// # Errors
/// `ComponentError::NoService` if there is none.
pub fn service_of<T: ?Sized + Send + Sync + 'static>(
    services: &Services,
) -> ComponentResult<&Arc<T>> {
    services.get::<T>().ok_or(ComponentError::NoService(TypeKey::of::<T>().name()))
}

/// A graph of components as the kernel builds it (issue 516): each component when it is asked
/// for, the rest in the order of the configuration, in the name of the graph's owner - which the
/// graph keeps, not the components. What a component is given to reach the others while it is
/// built (`ComponentContext`), and what an owner holds of the built graph (`ComponentContainer`).
#[async_trait]
pub trait ComponentGraph: Send + Sync + Debug {
    /// Every planned component that provides `facet`, but `asking` itself, each built if it is
    /// not yet, in the order of the configuration.
    ///
    /// # Errors
    /// Whatever building one of them reports; `ComponentError::Cycle` if one of them is being built
    /// and waits for `asking`.
    async fn all_of(
        self: Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Vec<Arc<BuiltComponent>>>;

    /// The one component that provides the single `facet`: the planned one, or the default -
    /// built if it is not yet.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if `facet` is not a facet of which there is one, or nothing
    /// provides it; as [`all_of`](Self::all_of).
    async fn one_of(
        self: Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Arc<BuiltComponent>>;

    /// The components built of the configuration, in its order; those not built yet - none, once
    /// the graph is built - are left out, as are the defaults of singles.
    fn built(&self) -> Vec<Arc<BuiltComponent>>;

    /// What it was built from: its configuration with every `ref` resolved to the components of
    /// the bundle it stood for (issue 270), in their order.
    fn components(&self) -> ComponentConfig;

    /// The facets its owner asks it for (issue 337).
    fn asked(&self) -> &[TypeKey];
}

/// What a component is built with: the services of the kernel its kind uses (`service`) and, while
/// it is built, the other components of its graph, asked for by type (`all`, `one`). This is all a
/// component is given: not its actor's runtime, so that it cannot store its actor's state past the
/// `Turn` it is called with (issue 128); of the services, only those its kind declares it uses
/// (`ComponentKind::uses`, issue 296); of the other components, only the facets it declares it
/// needs (`ComponentKind::needs`, issue 337); and not the name its graph is built in, which the
/// graph keeps (issue 516).
#[derive(Clone)]
pub struct ComponentContext {
    /// The services of the kernel its kind uses.
    services: Services,
    /// The facets of other components it may ask for, those its kind needs.
    needs: Vec<TypeKey>,
    /// The graph being built.
    graph: Arc<dyn ComponentGraph>,
    /// The component this context was given to.
    component: String,
}

impl Debug for ComponentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentContext")
            .field("services", &self.services.names())
            .field("needs", &self.needs)
            .field("component", &self.component)
            .finish()
    }
}

impl ComponentContext {
    /// The context of the component `component` of `graph`, while it is built: with `services`,
    /// those its kind uses, and the facets `needs` of others, those its kind needs.
    pub fn new(
        services: Services,
        needs: Vec<TypeKey>,
        graph: Arc<dyn ComponentGraph>,
        component: String,
    ) -> Self {
        Self { services, needs, graph, component }
    }

    /// The service `T` of the kernel - the texts (`PromptCatalogue`), the services of a library -,
    /// for a component whose kind declares it uses it.
    ///
    /// # Errors
    /// `ComponentError::NoService` if the kind does not declare it, or the kernel has none.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<&Arc<T>> {
        service_of(&self.services)
    }

    /// Every other component of the graph that provides `T`, in the order of
    /// the configuration - built first if they are not yet.
    ///
    /// # Errors
    /// `ComponentError::Undeclared` if the kind does not declare it needs
    /// `T`; whatever building one of them reports; `ComponentError::Cycle`
    /// if one of them is being built and waits for this one.
    pub async fn all<T: ?Sized + Send + Sync + 'static>(
        &self,
    ) -> ComponentResult<Vec<ComponentImpl<T>>> {
        require_declared::<T>(&self.needs)?;
        let graph = Arc::clone(&self.graph);
        let built = graph.all_of(TypeKey::of::<T>(), Some(&self.component)).await?;
        Ok(built.iter().filter_map(|node| node.get::<T>()).collect())
    }

    /// The one component that provides `T`, a facet of which there is
    /// exactly one (`ComponentRegistry::single`): the graph's, or the
    /// default - built first if it is not yet.
    ///
    /// # Errors
    /// As [`all`](Self::all).
    pub async fn one<T: ?Sized + Send + Sync + 'static>(
        &self,
    ) -> ComponentResult<ComponentImpl<T>> {
        require_declared::<T>(&self.needs)?;
        let graph = Arc::clone(&self.graph);
        let built = graph.one_of(TypeKey::of::<T>(), Some(&self.component)).await?;
        built.get::<T>().ok_or_else(|| ComponentError::not_single(TypeKey::of::<T>()))
    }
}

/// A built component: its id, its fail policy and tags from its entry, and its facets.
#[derive(Debug)]
pub struct BuiltComponent {
    pub id: String,
    pub fail_policy: ComponentFailPolicy,
    pub tags: Vec<String>,
    pub facets: ComponentFacets,
}

impl BuiltComponent {
    /// It as its facet `T`, if it provides it.
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<ComponentImpl<T>> {
        self.facets.get::<T>().map(|facet| ComponentImpl {
            id: self.id.clone(),
            fail_policy: self.fail_policy,
            facet: Arc::clone(facet),
        })
    }
}

/// The components of one configuration, built (`ComponentRegistry::build`): what their owner asks
/// for of them.
#[derive(Debug)]
pub struct ComponentContainer {
    graph: Arc<dyn ComponentGraph>,
}

impl ComponentContainer {
    /// The container of the built `graph`.
    pub fn new(graph: Arc<dyn ComponentGraph>) -> Self {
        Self { graph }
    }

    /// What it was built from: its configuration with every `ref` resolved
    /// to the components of the bundle it stood for (issue 270), in their
    /// order. The defaults built for singles it does not configure are not
    /// among them.
    pub fn components(&self) -> ComponentConfig {
        self.graph.components()
    }

    /// Every component that provides `T` - with `tag`, if one is given - in
    /// the order of the configuration.
    ///
    /// # Errors
    /// `ComponentError::Undeclared` if `T` is not among the facets it was
    /// built for.
    pub fn get_all<T>(&self, tag: Option<&str>) -> ComponentResult<Vec<ComponentImpl<T>>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        require_declared::<T>(self.graph.asked())?;
        Ok(self
            .graph
            .built()
            .iter()
            .filter(|node| tag.is_none_or(|tag| node.tags.iter().any(|t| t == tag)))
            .filter_map(|node| node.get::<T>())
            .collect())
    }

    /// The one component that provides `T`, a facet of which there is
    /// exactly one: the container's, or the default - built when it is first
    /// asked for.
    ///
    /// # Errors
    /// `ComponentError::Undeclared` if `T` is not among the facets it was
    /// built for; whatever building the default reports;
    /// `ComponentError::InvalidConfig` if `T` is not a facet of which there
    /// is one.
    pub async fn one<T: ?Sized + Send + Sync + 'static>(
        &self,
    ) -> ComponentResult<ComponentImpl<T>> {
        require_declared::<T>(self.graph.asked())?;
        Arc::clone(&self.graph)
            .one_of(TypeKey::of::<T>(), None)
            .await?
            .get::<T>()
            .ok_or_else(|| ComponentError::not_single(TypeKey::of::<T>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_left_out_give_the_defaults() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Config {
            #[serde(default)]
            limit: u32,
        }
        impl Params for Config {}
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
        impl Params for Config {}
        assert!(config_of_params::<Config>(&json!({})).unwrap().title.is_none());
        let message =
            config_of_params::<Config>(&json!({ "title": null })).unwrap_err().to_string();
        assert!(
            message.contains("invalid parameter 'title': it has no value"),
            "not taken as left out: {message}"
        );
    }
}
