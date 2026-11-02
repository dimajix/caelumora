//! The graph of a configuration's components as the registry builds it (`Graph`): each component
//! when it is asked for, the rest in the order of the configuration, and the defaults of the
//! singles.

use super::{BuildContext, ComponentRegistry};
use async_trait::async_trait;
use caelumora_api::component::{
    BuiltComponent, ComponentContainer, ComponentError, ComponentGraph, ComponentResult,
};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::type_map::TypeKey;
use futures::future::BoxFuture;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A component of a graph, as far as it has been built.
#[derive(Debug)]
enum State {
    Pending,
    Building,
    Built(Arc<BuiltComponent>),
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
    /// `cause`, as what went wrong with the component, wrapped with it and with the `ref` that
    /// brought it, if one did - so that the entry of the configuration it came from is named.
    pub(super) fn wrapped(&self, cause: ComponentError) -> ComponentError {
        let failed = ComponentError::of_component(&self.descriptor, cause);
        match &self.brought_by {
            Some(reference) => ComponentError::of_component(reference, failed),
            None => failed,
        }
    }
}

/// A component graph, built on demand: each component when it is asked for,
/// the rest in the order of the configuration.
#[derive(Debug)]
pub(super) struct Graph {
    registry: Arc<ComponentRegistry>,
    /// The environment every component is built in: its owner's name and the services.
    context: BuildContext,
    /// The facets its owner asks it for (`ComponentContainer::one`,
    /// `get_all`).
    asked: Vec<TypeKey>,
    planned: Vec<Planned>,
    states: Mutex<Vec<State>>,
    /// The defaults of singles for this graph alone, in place of the
    /// registry's (`ComponentRegistry::build_with_defaults`).
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
    ) -> BoxFuture<'static, ComponentResult<Arc<BuiltComponent>>> {
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
    ) -> ComponentResult<Arc<BuiltComponent>> {
        let failed = |cause| ComponentError::of_component(descriptor, cause);
        let facets = self
            .registry
            .dispatch(descriptor.clone(), &self.context.services, Arc::<Graph>::clone(self))
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
        Ok(Arc::new(BuiltComponent {
            id: descriptor.id.clone(),
            fail_policy: descriptor.fail_policy,
            tags: descriptor.tags.clone(),
            facets,
        }))
    }

    /// Every planned component that provides `facet`, except `asking` itself.
    async fn all(
        self: &Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Vec<Arc<BuiltComponent>>> {
        let mut found = Vec::new();
        for (index, planned) in self.planned.iter().enumerate() {
            if Some(planned.descriptor.id.as_str()) == asking
                || !planned.provides.iter().any(|f| f.id == facet.id)
            {
                continue;
            }
            found.push(self.ensure(index, asking.map(|component| (component, facet))).await?);
        }
        Ok(found)
    }

    /// The one component that provides the single `facet`: the planned one, or
    /// the default.
    async fn one(
        self: &Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Arc<BuiltComponent>> {
        if !self.registry.is_single(facet.id) {
            return Err(ComponentError::not_single(facet));
        }
        let asked = asking.map(|component| (component, facet));
        let providers: Vec<usize> = self
            .planned
            .iter()
            .enumerate()
            .filter(|(_, planned)| planned.provides.iter().any(|f| f.id == facet.id))
            .map(|(index, _)| index)
            .collect();
        match providers.as_slice() {
            [index] => self.ensure(*index, asked).await,
            // `build` refuses a graph with two.
            [] => self.default(facet, asked).await,
            _ => Err(ComponentError::not_single(facet)),
        }
    }

    /// The default for the single `facet`, built if it is not yet.
    async fn default(
        self: &Arc<Self>,
        facet: TypeKey,
        asked: Option<(&str, TypeKey)>,
    ) -> ComponentResult<Arc<BuiltComponent>> {
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

impl ComponentRegistry {
    /// Builds the components of `config` with the factories of the registry,
    /// each in `context` - a component asked for by another first, the
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
    /// `ComponentError::DuplicateId` if two entries of `config` share an id, or two of the
    /// components they stand for do - wrapped with the `ref` that brings the second -;
    /// `ComponentError::InvalidConfig` if two provide a facet of which there is one, or - wrapped
    /// with its entry - one provides no facet of the closure or has params its kind does not take
    /// or finds wrong, refused before anything is built; whatever resolving a `ref` or building a
    /// component reports, wrapped with its id and kind.
    pub async fn build(
        self: &Arc<Self>,
        config: &ComponentConfig,
        asked: &[TypeKey],
        context: &BuildContext,
    ) -> ComponentResult<ComponentContainer> {
        self.build_with_defaults(config, &ComponentConfig::default(), asked, context).await
    }

    /// Builds the components of `config` as `build` does, with the
    /// components of `defaults` as the defaults of the singles they provide,
    /// in place of the registry's, for this container alone: for a kind of
    /// owner whose singles want another default, such as an external agent's
    /// `ContextProvider` (issue 214).
    ///
    /// # Errors
    /// As `build`; and as `ComponentRegistry::set_defaults` for
    /// `defaults`.
    pub async fn build_with_defaults(
        self: &Arc<Self>,
        config: &ComponentConfig,
        defaults: &ComponentConfig,
        asked: &[TypeKey],
        context: &BuildContext,
    ) -> ComponentResult<ComponentContainer> {
        let own_defaults = self.defaults_of(defaults)?;
        let planned = self.plan(config, &own_defaults, &context.acting.context()).await?;
        // Refused before anything is built: params a kind does not take or
        // finds wrong (issue 515), and what nobody asks for, which would read
        // its owner's secrets for nothing (issue 337).
        self.require_buildable(&planned, &own_defaults, asked)?;
        let graph = Arc::new(Graph {
            registry: Arc::clone(self),
            context: context.clone(),
            asked: asked.to_vec(),
            states: Mutex::new(planned.iter().map(|_| State::Pending).collect()),
            planned,
            own_defaults,
            defaults: Mutex::new(HashMap::new()),
        });
        for index in 0..graph.planned.len() {
            graph.ensure(index, None).await?;
        }
        Ok(ComponentContainer::new(graph))
    }
}

#[async_trait]
impl ComponentGraph for Graph {
    async fn all_of(
        self: Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Vec<Arc<BuiltComponent>>> {
        self.all(facet, asking).await
    }

    async fn one_of(
        self: Arc<Self>,
        facet: TypeKey,
        asking: Option<&str>,
    ) -> ComponentResult<Arc<BuiltComponent>> {
        self.one(facet, asking).await
    }

    fn built(&self) -> Vec<Arc<BuiltComponent>> {
        let states = self.states.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        states
            .iter()
            .filter_map(|state| match state {
                State::Built(node) => Some(Arc::clone(node)),
                _ => None,
            })
            .collect()
    }

    fn components(&self) -> ComponentConfig {
        ComponentConfig {
            component_nodes: self
                .planned
                .iter()
                .map(|planned| planned.descriptor.clone())
                .collect(),
        }
    }

    fn asked(&self) -> &[TypeKey] {
        &self.asked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::*;
    use caelumora_api::component::ComponentFacets;
    use caelumora_api::component::ComponentKind;
    use caelumora_security::Policies;
    use caelumora_store::{EntityStores, Persistence};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn components_that_need_each_other_are_refused_naming_both() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
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

        let error = registry
            .build(&config(&[("hen", "chicken"), ("egg", "egg")]), ASKED, &test_context())
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
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        registry.register(ComponentKind::new("liar").provides::<dyn Named>(), |_, _| {
            Ok(ComponentFacets::empty())
        });
        let registry = Arc::new(registry);

        let error = registry
            .build(&config(&[("liar", "liar")]), ASKED, &test_context())
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

        let without =
            registry.build(&config(&[("a", "part")]), ASKED, &test_context()).await.unwrap();
        assert_eq!(built.load(Ordering::SeqCst), 0, "nobody asked for it yet");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "declared");
        assert_eq!(without.one::<dyn Choice>().await.unwrap().id, "declared");
        assert_eq!(built.load(Ordering::SeqCst), 1, "built once");

        let with =
            registry.build(&config(&[("mine", "other")]), ASKED, &test_context()).await.unwrap();
        assert_eq!(
            with.one::<dyn Choice>().await.unwrap().id,
            "mine",
            "the configured one replaces the default"
        );

        let two = registry
            .build(&config(&[("mine", "other"), ("yours", "other")]), ASKED, &test_context())
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

        let own = registry
            .build_with_defaults(
                &config(&[]),
                &config(&[("its_own", "other")]),
                ASKED,
                &test_context(),
            )
            .await
            .unwrap();
        assert_eq!(
            own.one::<dyn Choice>().await.unwrap().id,
            "its_own",
            "in place of the configuration's too"
        );
        let mine = registry
            .build_with_defaults(
                &config(&[("mine", "other")]),
                &config(&[("its_own", "other")]),
                ASKED,
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

        let refused = registry
            .build_with_defaults(&config(&[]), &config(&[("x", "part")]), ASKED, &test_context())
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
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
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

        let graph = registry.build(&config(&[]), asked, &test_context()).await.unwrap();
        graph.one::<dyn Choice>().await.unwrap();
        graph.one::<dyn Other>().await.unwrap();
        assert_eq!(built.load(Ordering::SeqCst), 1, "one component for both");

        let configured = registry
            .build_with_defaults(
                &config(&[]),
                &config(&[("model", "pair")]),
                asked,
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
        let built = registry
            .build(&config(&[]), ASKED, &test_context())
            .await
            .expect("nobody asked for it");
        let message = built.one::<dyn Named>().await.expect_err("there is none").to_string();
        assert!(
            message.contains("no component provides Named, and it has no default"),
            "{message}"
        );
        assert_eq!(registry.kind_of::<dyn Named>(&config(&[])), None, "no kind to tell");
        let provided =
            registry.build(&config(&[("a", "part")]), ASKED, &test_context()).await.unwrap();
        assert_eq!(provided.one::<dyn Named>().await.expect("the container's own").id, "a");

        let (mut registry, _) = choices();
        registry.single::<dyn Named>(Some("missing"));
        let registry = Arc::new(registry);
        let built = registry.build(&config(&[]), ASKED, &test_context()).await.unwrap();
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
