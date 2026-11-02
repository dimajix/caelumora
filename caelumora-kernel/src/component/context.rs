//! The environment a configuration of components is built in (`BuildContext`).

use crate::kernel::{Acting, ActingKind};
use caelumora_api::component::Services;
use caelumora_security::SecurityContext;
use std::sync::Arc;

/// The environment a configuration of components is built in (issues 321, 516): in whose name - an
/// agent's own, a channel's owner's -, in which every `ref` is resolved and the graph keeps, and
/// the services of the kernel, of which each component is given those its kind uses
/// (`ComponentContext`). A component does not keep the name: what it does later it does in the
/// context of the act it serves, as a tool does in its turn's. The one exception is a plugin's
/// model (`caelumora-model`, issue 520), which is given the `Acting` as a service and keeps it - a
/// handle that builds the context of each call when it is made, never a context, and one no library
/// can name.
#[derive(Debug, Clone)]
pub struct BuildContext {
    pub(super) acting: Acting,
    pub(super) services: Services,
}

impl BuildContext {
    /// The environment of a graph built in `security_context`'s name, given once, with `services`.
    pub fn new(security_context: SecurityContext, services: Services) -> Self {
        Self::acting_as(Acting(ActingKind::Given(security_context)), services)
    }

    /// The environment of a graph built in the name of `acting` - an agent's -, as the kernel holds
    /// it when the graph is planned, with `services`.
    pub(crate) fn acting_as(acting: Acting, services: Services) -> Self {
        // A kind that acts in that name later - a plugin's model - takes it as a service.
        let services = services.with(Arc::new(acting.clone()));
        Self { acting, services }
    }
}

#[cfg(test)]
mod tests {

    use crate::component::testing::*;
    use crate::component::ComponentRegistry;
    use caelumora_api::component::ComponentError;
    use caelumora_api::component::ComponentKind;
    use caelumora_security::{AccessPolicy, Policies};
    use caelumora_store::{EntityStores, Persistence};
    use std::sync::Arc;
    use std::sync::Mutex;

    #[tokio::test]
    async fn a_component_gets_every_other_that_provides_what_it_asks_for_in_the_order_of_the_configuration(
    ) {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        registry.register(ComponentKind::new("part").provides::<dyn Named>(), |descriptor, _| {
            Ok(named(&descriptor.id))
        });
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        // Provides what it collects, so that it has to leave itself out.
        registry.register_async(
            ComponentKind::new("collector").provides::<dyn Named>().needs::<dyn Named>(),
            move |descriptor, context| {
                let recorded = Arc::clone(&recorded);
                async move {
                    let parts = context.all::<dyn Named>().await?;
                    recorded.lock().unwrap().extend(parts.iter().map(|part| part.name()));
                    Ok(named(&descriptor.id))
                }
            },
        );
        let registry = Arc::new(registry);

        // Configured before what it collects, which is built for it first.
        let container = registry
            .build(
                &config(&[("collector", "collector"), ("a", "part"), ("b", "part")]),
                ASKED,
                &test_context(),
            )
            .await
            .expect("builds");

        assert_eq!(
            *seen.lock().unwrap(),
            ["a", "b"],
            "every other component with the facet, in the order of the configuration"
        );
        let all: Vec<_> =
            container.get_all::<dyn Named>(None).unwrap().iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            all,
            ["collector", "a", "b"],
            "the container keeps the order of the configuration"
        );
    }

    /// A kind is given the services it declares it uses (issue 296), and
    /// none it does not: the context of the graph has the policy, the one of
    /// `blind` does not.
    #[tokio::test]
    async fn a_component_is_given_the_services_its_kind_uses_alone() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        registry.register(
            ComponentKind::new("seeing").provides::<dyn Named>().uses::<dyn AccessPolicy>(),
            |node, context| {
                context.service::<dyn AccessPolicy>()?;
                Ok(named(&node.id))
            },
        );
        registry.register(ComponentKind::new("blind").provides::<dyn Named>(), |node, context| {
            context.service::<dyn AccessPolicy>()?;
            Ok(named(&node.id))
        });
        let registry = Arc::new(registry);

        registry
            .build(&config(&[("seeing", "seeing")]), ASKED, &test_context())
            .await
            .expect("given what it uses");
        let error = registry
            .build(&config(&[("blind", "blind")]), ASKED, &test_context())
            .await
            .expect_err("not what it does not declare");
        assert!(
            causes(&error).iter().any(|e| matches!(e, ComponentError::NoService("AccessPolicy"))),
            "{error}"
        );
    }

    /// A component may ask for the facets its kind needs and no other, an
    /// owner for those it built its container for (issue 337).
    #[tokio::test]
    async fn a_facet_that_was_not_declared_is_refused() {
        let mut registry = ComponentRegistry::new(
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .component_bundles,
        );
        registry.register_async(
            ComponentKind::new("greedy").provides::<dyn Named>(),
            |descriptor, context| async move {
                context.all::<dyn Named>().await?;
                Ok(named(&descriptor.id))
            },
        );
        registry.register(
            ComponentKind::new("part").provides::<dyn Named>().provides::<dyn Other>(),
            |descriptor, _| {
                Ok(named(&descriptor.id)
                    .with(Arc::new(Name(descriptor.id.clone())) as Arc<dyn Other>))
            },
        );
        let registry = Arc::new(registry);
        let error = registry
            .build(&config(&[("greedy", "greedy")]), NAMED, &test_context())
            .await
            .expect_err("its kind does not need Named");
        assert!(
            causes(&error).iter().any(|e| matches!(e, ComponentError::Undeclared("Named"))),
            "{error}"
        );

        let container = registry
            .build(&config(&[("a", "part")]), NAMED, &test_context())
            .await
            .expect("provides Named");
        assert!(
            matches!(
                container.get_all::<dyn Other>(None),
                Err(ComponentError::Undeclared("Other"))
            ),
            "not built for Other"
        );
        assert!(
            matches!(
                container.one::<dyn Choice>().await,
                Err(ComponentError::Undeclared("Choice"))
            ),
            "not built for Choice"
        );
    }
}
