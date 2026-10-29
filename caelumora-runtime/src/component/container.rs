use super::{
    require_declared, ComponentContext, ComponentError, ComponentFacets, ComponentRegistry,
    ComponentResult,
};
use crate::remote::describe_chain;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::type_map::TypeKey;
use futures::future::BoxFuture;
use std::any::TypeId;
use std::collections::HashMap;
use std::error::Error;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

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
        self.facets.get::<T>().map(|facet| ComponentImpl {
            id: self.id.clone(),
            fail_policy: self.fail_policy,
            facet: Arc::clone(facet),
        })
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

/// A component of a graph, as far as it has been built.
#[derive(Debug)]
enum State {
    Pending,
    Building,
    Built(Arc<ComponentNode>),
}

/// A component of a graph to be built: its descriptor, `ref`s resolved, the
/// facets its kind provides, and the `ref` that brought it, if one did.
#[derive(Debug)]
pub(super) struct Planned {
    pub(super) descriptor: ComponentDescriptor,
    pub(super) provides: Vec<TypeKey>,
    pub(super) brought_by: Option<ComponentDescriptor>,
}

impl Planned {
    /// `cause`, as what went wrong with the entry of the configuration that
    /// brought it - the `ref`, naming it within, or itself.
    pub(super) fn failed(&self, cause: &str) -> ComponentError {
        let descriptor = &self.descriptor;
        let (entry, cause) = match &self.brought_by {
            Some(reference) => (
                reference,
                format!("its component '{}' ({}) {cause}", descriptor.id, descriptor.kind),
            ),
            None => (descriptor, format!("it {cause}")),
        };
        ComponentError::of_component(entry, ComponentError::InvalidConfig(cause))
    }
}

/// A component graph, built on demand: each component when it is asked for,
/// the rest in the order of the configuration.
#[derive(Debug)]
pub(super) struct Graph {
    registry: Arc<ComponentRegistry>,
    /// The environment every component is built in.
    context: ComponentContext,
    /// The facets its owner asks it for (`ComponentContainer::one`,
    /// `get_all`).
    asked: Vec<TypeKey>,
    planned: Vec<Planned>,
    states: Mutex<Vec<State>>,
    /// The defaults of singles for this graph alone, in place of the
    /// registry's (`ComponentContainer::with_defaults`).
    own_defaults: HashMap<TypeId, ComponentDescriptor>,
    /// The defaults built for singles no component of the graph provides,
    /// by their ids: a default that provides several singles is built once
    /// and serves all of them.
    defaults: Mutex<HashMap<String, State>>,
}

impl Graph {
    /// The planned component `index`, built if it is not yet. `asked` is who
    /// asked for which facet, for the message if it is being built.
    fn ensure(
        self: &Arc<Self>,
        index: usize,
        asked: Option<(&str, TypeKey)>,
    ) -> BoxFuture<'static, ComponentResult<Arc<ComponentNode>>> {
        let graph = Arc::clone(self);
        let asked = asked.map(|(component, facet)| (component.to_string(), facet));
        Box::pin(async move {
            let planned = &graph.planned[index];
            {
                let mut states =
                    graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                match &states[index] {
                    State::Built(node) => return Ok(Arc::clone(node)),
                    State::Building => return Err(cycle(asked, &planned.descriptor.id)),
                    State::Pending => states[index] = State::Building,
                }
            }
            let node = graph.construct(&planned.descriptor, &planned.provides).await?;
            graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[index] =
                State::Built(Arc::clone(&node));
            Ok(node)
        })
    }

    /// Builds `descriptor` with a context through which it reaches the rest
    /// of the graph, and checks that it provides exactly `provides`.
    async fn construct(
        self: &Arc<Self>,
        descriptor: &ComponentDescriptor,
        provides: &[TypeKey],
    ) -> ComponentResult<Arc<ComponentNode>> {
        let failed = |cause| ComponentError::of_component(descriptor, cause);
        let context = ComponentContext {
            building: Some((Arc::clone(self), descriptor.id.clone())),
            ..self.context.clone()
        };
        let facets = self
            .registry
            .dispatch(descriptor.clone(), context)
            .map_err(failed)?
            .await
            .map_err(failed)?;

        let mut declared: Vec<&'static str> = provides.iter().map(|f| f.name()).collect();
        declared.sort_unstable();
        if facets.names() != declared {
            return Err(failed(ComponentError::InvalidConfig(format!(
                "provides [{}] instead of the declared [{}]",
                facets.names().join(", "),
                declared.join(", ")
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
    pub(super) async fn all<T: ?Sized + Send + Sync + 'static>(
        self: &Arc<Self>,
        asking: Option<&str>,
    ) -> ComponentResult<Vec<ComponentImpl<T>>> {
        let facet = TypeKey::of::<T>();
        let mut found = Vec::new();
        for (index, planned) in self.planned.iter().enumerate() {
            if Some(planned.descriptor.id.as_str()) == asking
                || !planned.provides.iter().any(|f| f.id == facet.id)
            {
                continue;
            }
            let node = self.ensure(index, asking.map(|component| (component, facet))).await?;
            found.extend(node.get::<T>());
        }
        Ok(found)
    }

    /// The one component that provides the single `T`: the planned one, or
    /// the default.
    pub(super) async fn one<T: ?Sized + Send + Sync + 'static>(
        self: &Arc<Self>,
        asking: Option<&str>,
    ) -> ComponentResult<ComponentImpl<T>> {
        let facet = TypeKey::of::<T>();
        let not_single = || {
            ComponentError::InvalidConfig(format!(
                "{} is not a facet of which there is one",
                facet.name()
            ))
        };
        if !self.registry.is_single(facet.id) {
            return Err(not_single());
        }
        let asked = asking.map(|component| (component, facet));
        let providers: Vec<usize> = self
            .planned
            .iter()
            .enumerate()
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
    async fn default(
        self: &Arc<Self>,
        facet: TypeKey,
        asked: Option<(&str, TypeKey)>,
    ) -> ComponentResult<Arc<ComponentNode>> {
        let descriptor =
            self.registry.default_for(facet.id, &self.own_defaults).ok_or_else(|| {
                ComponentError::InvalidConfig(format!(
                    "no component provides {}, and it has no default",
                    facet.name()
                ))
            })?;
        {
            let mut defaults =
                self.defaults.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match defaults.get(&descriptor.id) {
                Some(State::Built(node)) => return Ok(Arc::clone(node)),
                Some(State::Building) => {
                    return Err(cycle(asked.map(|(c, f)| (c.to_string(), f)), &descriptor.id))
                }
                Some(State::Pending) | None => {
                    defaults.insert(descriptor.id.clone(), State::Building);
                }
            }
        }
        let provides = self.registry.provides(&descriptor.kind).unwrap_or_default().to_vec();
        if !provides.iter().any(|f| f.id == facet.id) {
            // A kind registered in code under a name that is wrong, or
            // registered without the facet: a mistake no configuration can
            // make, found the first time the default is needed.
            return Err(ComponentError::InvalidConfig(format!(
                "the default kind '{}' of {} is not registered as providing it",
                descriptor.kind,
                facet.name()
            )));
        }
        let node = self.construct(&descriptor, &provides).await?;
        self.defaults
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(descriptor.id.clone(), State::Built(Arc::clone(&node)));
        Ok(node)
    }
}

/// The error for asking for a component that is being built.
fn cycle(asked: Option<(String, TypeKey)>, other: &str) -> ComponentError {
    match asked {
        Some((component, facet)) => {
            ComponentError::Cycle { component, facet: facet.name(), other: other.to_string() }
        }
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
    /// rest in the order of the configuration -, for an owner that asks it
    /// for the facets `asked` and no other (issue 337).
    ///
    /// A `ref` stands for the components of the bundle it names, in their
    /// order, where it is written (issue 270).
    ///
    /// What it accepts is the closure of `asked`: the components that
    /// provide one of those facets, and those that provide a facet one of
    /// them needs (`ComponentKind::needs`), in turn. A component that
    /// provides none would be built for nothing - reading its owner's
    /// secrets, connecting - and is refused before anything is built.
    ///
    /// # Errors
    /// `ComponentError::DuplicateId` if two entries of `config` share an id,
    /// or two of the components they stand for do - wrapped with the `ref`
    /// that brings the second -; `ComponentError::InvalidConfig` if two
    /// provide a facet of which there is one, or one provides no facet of
    /// the closure, wrapped with its entry; whatever resolving a `ref` or
    /// building a component reports, wrapped with its id and kind.
    pub async fn from_config(
        config: &ComponentConfig,
        asked: &[TypeKey],
        registry: &Arc<ComponentRegistry>,
        context: &ComponentContext,
    ) -> ComponentResult<Self> {
        Self::with_defaults(config, &ComponentConfig::default(), asked, registry, context).await
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
        asked: &[TypeKey],
        registry: &Arc<ComponentRegistry>,
        context: &ComponentContext,
    ) -> ComponentResult<Self> {
        let own_defaults = registry.defaults_of(defaults)?;
        let planned = registry.plan(config, &own_defaults, &context.security_context).await?;
        // Refused before anything is built: what nobody asks for would read
        // its owner's secrets for nothing (issue 337).
        registry.require_asked(&planned, &own_defaults, asked)?;
        let graph = Arc::new(Graph {
            registry: Arc::clone(registry),
            context: ComponentContext { building: None, ..context.clone() },
            asked: asked.to_vec(),
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
    /// to the components of the bundle it stood for (issue 270), in their
    /// order. The defaults built for singles it does not configure are not
    /// among them.
    pub fn components(&self) -> ComponentConfig {
        ComponentConfig {
            component_nodes: self
                .graph
                .planned
                .iter()
                .map(|planned| planned.descriptor.clone())
                .collect(),
        }
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
        require_declared::<T>(&self.graph.asked)?;
        let states = self.graph.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(states
            .iter()
            .filter_map(|state| match state {
                State::Built(node) => Some(node),
                _ => None,
            })
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
        require_declared::<T>(&self.graph.asked)?;
        self.graph.one::<T>(None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::*;
    use crate::component::ComponentKind;
    use caelumora_model::store::Persistence;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn components_that_need_each_other_are_refused_naming_both() {
        let mut registry = ComponentRegistry::new(
            Persistence::in_memory().entity_stores().unwrap().component_bundles,
        );
        registry.register_async(
            ComponentKind::new("chicken").provides::<dyn Named>().needs::<dyn Other>(),
            |descriptor, context| async move {
                context.all::<dyn Other>().await?;
                Ok(named(&descriptor.id))
            },
        );
        registry.register_async(
            ComponentKind::new("egg").provides::<dyn Other>().needs::<dyn Named>(),
            |descriptor, context| async move {
                context.all::<dyn Named>().await?;
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Other>(Arc::new(Name(descriptor.id.clone())));
                Ok(facets)
            },
        );
        let registry = Arc::new(registry);

        let error = ComponentContainer::from_config(
            &config(&[("hen", "chicken"), ("egg", "egg")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .expect_err("they need each other");

        let innermost = causes(&error).last().copied().unwrap();
        assert!(
            matches!(innermost, ComponentError::Cycle { component, facet: "Named", other } if component == "egg" && other == "hen"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_component_must_provide_what_its_kind_declares() {
        let mut registry = ComponentRegistry::new(
            Persistence::in_memory().entity_stores().unwrap().component_bundles,
        );
        registry.register(ComponentKind::new("liar").provides::<dyn Named>(), |_, _| {
            Ok(ComponentFacets::empty())
        });
        let registry = Arc::new(registry);

        let error = ComponentContainer::from_config(
            &config(&[("liar", "liar")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .expect_err("provides nothing");

        let message = error.to_string();
        assert!(
            message.contains("'liar'") && message.contains("instead of the declared [Named]"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn of_a_single_facet_there_is_the_configured_one_or_the_default_built_when_first_asked_for(
    ) {
        let (registry, built) = choices();
        let registry = Arc::new(registry);

        let without = ComponentContainer::from_config(
            &config(&[("a", "part")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(built.load(Ordering::SeqCst), 0, "nobody asked for it yet");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "declared");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "declared");
        assert_eq!(built.load(Ordering::SeqCst), 1, "built once");

        let with = ComponentContainer::from_config(
            &config(&[("mine", "other")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(
            with.one::<dyn Choice>().await.unwrap().id,
            "mine",
            "the configured one replaces the default"
        );

        let two = ComponentContainer::from_config(
            &config(&[("mine", "other"), ("yours", "other")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await;
        let message = two.expect_err("there is one").to_string();
        assert!(message.contains("'mine' and 'yours' both provide Choice"), "{message}");

        let not_single =
            without.one::<dyn Named>().await.expect_err("of Named there may be any number");
        assert!(matches!(not_single, ComponentError::InvalidConfig(_)), "{not_single}");
    }

    #[tokio::test]
    async fn a_container_may_have_defaults_of_its_own_in_place_of_the_registrys() {
        let (mut registry, built) = choices();
        registry.set_defaults(&config(&[("configured", "declared")])).unwrap();
        let registry = Arc::new(registry);

        let own = ComponentContainer::with_defaults(
            &config(&[]),
            &config(&[("its_own", "other")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(
            own.one::<dyn Choice>().await.unwrap().id,
            "its_own",
            "in place of the configuration's too"
        );
        let mine = ComponentContainer::with_defaults(
            &config(&[("mine", "other")]),
            &config(&[("its_own", "other")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(
            mine.one::<dyn Choice>().await.unwrap().id,
            "mine",
            "what the container configures still comes first"
        );
        assert_eq!(built.load(Ordering::SeqCst), 0, "no other default is built");

        let refused = ComponentContainer::with_defaults(
            &config(&[]),
            &config(&[("x", "part")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await;
        assert!(
            matches!(refused, Err(ComponentError::InvalidConfig(_))),
            "a default must provide a single"
        );
    }

    /// A default that provides two singles - a model, its client and its
    /// profile - is built once and serves both, whether it is the kind
    /// declared or the configuration's.
    #[tokio::test]
    async fn a_default_of_two_singles_is_built_once_for_both() {
        let mut registry = ComponentRegistry::new(
            Persistence::in_memory().entity_stores().unwrap().component_bundles,
        );
        let built = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&built);
        registry.register(
            ComponentKind::new("pair").provides::<dyn Choice>().provides::<dyn Other>(),
            move |descriptor, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                let name = Arc::new(Name(descriptor.id.clone()));
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Choice>(Arc::<Name>::clone(&name));
                facets.insert::<dyn Other>(name);
                Ok(facets)
            },
        );
        registry.single::<dyn Choice>(Some("pair"));
        registry.single::<dyn Other>(Some("pair"));
        let registry = Arc::new(registry);
        let asked = &[TypeKey::of::<dyn Choice>(), TypeKey::of::<dyn Other>()];

        let graph =
            ComponentContainer::from_config(&config(&[]), asked, &registry, &test_context())
                .await
                .unwrap();
        graph.one::<dyn Choice>().await.unwrap();
        graph.one::<dyn Other>().await.unwrap();
        assert_eq!(built.load(Ordering::SeqCst), 1, "one component for both");

        let configured = ComponentContainer::with_defaults(
            &config(&[]),
            &config(&[("model", "pair")]),
            asked,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(configured.one::<dyn Choice>().await.unwrap().id, "model");
        assert_eq!(configured.one::<dyn Other>().await.unwrap().id, "model");
        assert_eq!(built.load(Ordering::SeqCst), 2, "the configuration's, once as well");
    }

    #[tokio::test]
    async fn a_single_without_a_usable_default_fails_where_it_is_asked_for() {
        let (mut registry, _) = choices();
        registry.single::<dyn Named>(None);
        let registry = Arc::new(registry);
        let built =
            ComponentContainer::from_config(&config(&[]), ASKED, &registry, &test_context())
                .await
                .expect("nobody asked for it");
        let message = built.one::<dyn Named>().await.expect_err("there is none").to_string();
        assert!(
            message.contains("no component provides Named, and it has no default"),
            "{message}"
        );
        assert_eq!(registry.kind_of::<dyn Named>(&config(&[])), None, "no kind to tell");
        let provided = ComponentContainer::from_config(
            &config(&[("a", "part")]),
            ASKED,
            &registry,
            &test_context(),
        )
        .await
        .unwrap();
        assert_eq!(provided.one::<dyn Named>().await.expect("the container's own").id, "a");

        let (mut registry, _) = choices();
        registry.single::<dyn Named>(Some("missing"));
        let registry = Arc::new(registry);
        let built =
            ComponentContainer::from_config(&config(&[]), ASKED, &registry, &test_context())
                .await
                .unwrap();
        let message = built
            .one::<dyn Named>()
            .await
            .expect_err("a default kind that is not registered")
            .to_string();
        assert!(
            message
                .contains("the default kind 'missing' of Named is not registered as providing it"),
            "{message}"
        );
    }
}
