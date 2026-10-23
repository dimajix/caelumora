use caelumora_model::security::SecurityContext;
use caelumora_model::type_map::TypeKey;
use crate::runtime::Services;
use std::sync::Arc;
use super::container::{ComponentImpl, Graph};
use super::registry::Registration;
use super::{require_declared, ComponentError, ComponentResult};
use std::fmt;
use std::fmt::Debug;

/// The service `T` of `services`, for what is made with them: a component
/// (`ComponentContext::service`), a backend (`Backends`).
///
/// # Errors
/// `ComponentError::NoService` if there is none.
pub fn service_of<T: ?Sized + Send + Sync + 'static>(services: &Services) -> ComponentResult<&Arc<T>> {
    services.get::<T>().ok_or(ComponentError::NoService(TypeKey::of::<T>().name()))
}

/// What a component is built with: the environment of the whole graph - the
/// security context it is built under, the services of the runtime it uses
/// (`service`) - and, while it is built, the other components of its graph,
/// asked for by type (`all`, `one`). This is all a component is given: not its
/// actor's runtime, so that it cannot store its actor's state past the `Turn`
/// it is called with (issue 128); of the services, only those its kind
/// declares it uses (`ComponentKind::uses`, issue 296); and of the other
/// components, only the facets it declares it needs (`ComponentKind::needs`,
/// issue 337).
#[derive(Clone)]
pub struct ComponentContext {
    /// The name the graph is built in, that of its owner (issue 321): an
    /// agent's own, a channel's owner's. What a component asks while it is
    /// built - a `ref` resolved, a secret - it asks in this name. It does not
    /// keep it: what it does later it does in the context of the act it
    /// serves, as a tool does in its turn's.
    pub security_context: SecurityContext,
    /// The services of the runtime: all of them in the context a graph is
    /// built with, those its kind uses in the one a component is given.
    pub(super) services: Services,
    /// The facets of other components it may ask for: none in the context a
    /// graph is built with, those its kind needs in the one a component is
    /// given.
    pub(super) needs: Vec<TypeKey>,
    /// The graph being built and the component this context was given to;
    /// set by `ComponentContainer::from_config`.
    pub(super) building: Option<(Arc<Graph>, String)>,
}

impl Debug for ComponentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentContext")
            .field("security_context", &self.security_context)
            .field("services", &self.services.names())
            .field("needs", &self.needs)
            .field("building", &self.building.as_ref().map(|(_, component)| component))
            .finish()
    }
}

impl ComponentContext {
    /// The environment a component graph is built in, with the services of
    /// the runtime: `ComponentContainer::from_config` gives each component
    /// this context, with the services its kind uses, through which it
    /// reaches the others.
    pub fn new(security_context: SecurityContext, services: Services) -> Self {
        Self { security_context, services, needs: Vec::new(), building: None }
    }

    /// The service `T` of the runtime - the policy (`dyn AccessPolicy`),
    /// the secrets (`SecretManager`), the workspaces (`WorkspaceManager`) -,
    /// for a component whose kind declares it uses it.
    ///
    /// # Errors
    /// `ComponentError::NoService` if the kind does not declare it, or the
    /// runtime has none.
    pub fn service<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<&Arc<T>> {
        service_of(&self.services)
    }

    /// The context for a component of `kind`: this one, with the services it
    /// uses alone, and the facets it needs.
    pub(super) fn for_kind(&self, kind: &Registration) -> Self {
        Self { services: self.services.only(&kind.uses), needs: kind.needs.clone(), ..self.clone() }
    }

    /// Every other component of the graph that provides `T`, in the order of
    /// the configuration - built first if they are not yet.
    ///
    /// # Errors
    /// `ComponentError::Undeclared` if the kind does not declare it needs
    /// `T`; whatever building one of them reports; `ComponentError::Cycle`
    /// if one of them is being built and waits for this one.
    pub async fn all<T: ?Sized + Send + Sync + 'static>(&self) -> ComponentResult<Vec<ComponentImpl<T>>> {
        require_declared::<T>(&self.needs)?;
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
        require_declared::<T>(&self.needs)?;
        let (graph, component) = self.building()?;
        graph.one::<T>(Some(component)).await
    }

    fn building(&self) -> ComponentResult<(&Arc<Graph>, &str)> {
        self.building.as_ref()
            .map(|(graph, component)| (graph, component.as_str()))
            .ok_or_else(|| ComponentError::InvalidConfig("other components can only be asked for while a component is built".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::testing::*;
    use crate::component::{ComponentContainer, ComponentKind, ComponentRegistry};
    use caelumora_model::security::AccessPolicy;
    use caelumora_model::store::Persistence;
    use std::sync::Mutex;

        #[tokio::test]
        async fn a_component_gets_every_other_that_provides_what_it_asks_for_in_the_order_of_the_configuration() {
            let mut registry = ComponentRegistry::new(Persistence::in_memory().entity_stores().unwrap().component_bundles);
            registry.register(ComponentKind::new("part").provides::<dyn Named>(), |descriptor, _| Ok(named(&descriptor.id)));
            let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&seen);
            // Provides what it collects, so that it has to leave itself out.
            registry.register_async(ComponentKind::new("collector").provides::<dyn Named>().needs::<dyn Named>(), move |descriptor, context| {
                let recorded = Arc::clone(&recorded);
                async move {
                    let parts = context.all::<dyn Named>().await?;
                    recorded.lock().unwrap().extend(parts.iter().map(|part| part.name()));
                    Ok(named(&descriptor.id))
                }
            });
            let registry = Arc::new(registry);

            // Configured before what it collects, which is built for it first.
            let container = ComponentContainer::from_config(&config(&[("collector", "collector"), ("a", "part"), ("b", "part")]), ASKED, &registry, &test_context())
                .await.expect("builds");

            assert_eq!(*seen.lock().unwrap(), ["a", "b"], "every other component with the facet, in the order of the configuration");
            let all: Vec<_> = container.get_all::<dyn Named>(None).unwrap().iter().map(|c| c.id.clone()).collect();
            assert_eq!(all, ["collector", "a", "b"], "the container keeps the order of the configuration");
        }

        /// A kind is given the services it declares it uses (issue 296), and
        /// none it does not: the context of the graph has the policy, the one of
        /// `blind` does not.
        #[tokio::test]
        async fn a_component_is_given_the_services_its_kind_uses_alone() {
            let mut registry = ComponentRegistry::new(Persistence::in_memory().entity_stores().unwrap().component_bundles);
            registry.register(ComponentKind::new("seeing").provides::<dyn Named>().uses::<dyn AccessPolicy>(), |node, context| {
                context.service::<dyn AccessPolicy>()?;
                Ok(named(&node.id))
            });
            registry.register(ComponentKind::new("blind").provides::<dyn Named>(), |node, context| {
                context.service::<dyn AccessPolicy>()?;
                Ok(named(&node.id))
            });
            let registry = Arc::new(registry);

            ComponentContainer::from_config(&config(&[("seeing", "seeing")]), ASKED, &registry, &test_context()).await.expect("given what it uses");
            let error = ComponentContainer::from_config(&config(&[("blind", "blind")]), ASKED, &registry, &test_context())
                .await.expect_err("not what it does not declare");
            assert!(causes(&error).iter().any(|e| matches!(e, ComponentError::NoService("AccessPolicy"))), "{error}");
        }

        /// A component may ask for the facets its kind needs and no other, an
        /// owner for those it built its container for (issue 337).
        #[tokio::test]
        async fn a_facet_that_was_not_declared_is_refused() {
            let mut registry = ComponentRegistry::new(Persistence::in_memory().entity_stores().unwrap().component_bundles);
            registry.register_async(ComponentKind::new("greedy").provides::<dyn Named>(), |descriptor, context| async move {
                context.all::<dyn Named>().await?;
                Ok(named(&descriptor.id))
            });
            registry.register(ComponentKind::new("part").provides::<dyn Named>().provides::<dyn Other>(), |descriptor, _| {
                Ok(named(&descriptor.id).with(Arc::new(Name(descriptor.id.clone())) as Arc<dyn Other>))
            });
            let registry = Arc::new(registry);
            let error = ComponentContainer::from_config(&config(&[("greedy", "greedy")]), NAMED, &registry, &test_context())
                .await.expect_err("its kind does not need Named");
            assert!(causes(&error).iter().any(|e| matches!(e, ComponentError::Undeclared("Named"))), "{error}");

            let container = ComponentContainer::from_config(&config(&[("a", "part")]), NAMED, &registry, &test_context()).await.expect("provides Named");
            assert!(matches!(container.get_all::<dyn Other>(None), Err(ComponentError::Undeclared("Other"))), "not built for Other");
            assert!(matches!(container.one::<dyn Choice>().await, Err(ComponentError::Undeclared("Choice"))), "not built for Choice");
        }
}
