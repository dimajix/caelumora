use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use caelumora_model::store::EntityStore;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use crate::runtime::ActorRuntime;
use crate::session_reader::SessionReader;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("unsupported component kind: {0}")]
    UnsupportedKind(String),
    #[error("duplicate component id: {0}")]
    DuplicateId(String),
    #[error("component '{component}' depends on unknown component '{dependency}'")]
    UnknownDependency { component: String, dependency: String },
    #[error("cycle detected in component graph")]
    CycleDetected,
    #[error("unknown component node '{0}'")]
    NodeNotFound(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// A component of a graph could not be built. Wraps whatever went wrong
    /// with the component's id and kind, so that the message names the
    /// offending component on its own (issue 02).
    #[error("component '{component}' ({kind}) cannot be built: {cause}")]
    Build { component: String, kind: String, cause: Box<ComponentError> },
}

pub type ComponentResult<T> = Result<T, ComponentError>;


/// What a component is built with: the facets of the components it
/// depends on, and the environment of the whole graph - the security
/// context it is built under, the secrets it may resolve, and, for the
/// components of an actor, the actor's runtime.
#[derive(Debug, Clone)]
pub struct ComponentContext {
    pub deps: HashMap<String, ComponentFacets>,
    pub security_context: SecurityContext,
    pub secret_manager: Arc<SecretManager>,
    /// The runtime of the actor the components are built for (issue 69);
    /// `None` for components of no actor, such as the commands of a session.
    pub actor_runtime: Option<ActorRuntime>,
}

impl ComponentContext {
    /// The context a component graph is built in, without dependencies:
    /// `ComponentContainer::from_config` gives each component this context
    /// with its own dependencies added.
    pub fn new(security_context: SecurityContext, secret_manager: Arc<SecretManager>, actor_runtime: Option<ActorRuntime>) -> Self {
        Self { deps: HashMap::new(), security_context, secret_manager, actor_runtime }
    }

    /// What the components may read of the session's other channels (issue
    /// 17): through the actor's runtime, or nothing for components of no
    /// actor.
    pub fn session_reader(&self) -> Arc<dyn SessionReader> {
        match &self.actor_runtime {
            Some(runtime) => runtime.session_reader(),
            None => crate::session_reader::detached(),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct ComponentFacets {
    map: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl ComponentFacets {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, value: Arc<T>) {
        self.map.insert(TypeId::of::<T>(), Arc::new(value) as Arc<dyn Any + Send + Sync>);
    }

    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<Arc<T>>())
            .cloned()
    }
}

pub type ComponentFactory = Box<dyn Fn(&ComponentDescriptor, &ComponentContext) -> ComponentResult<ComponentFacets> + Send + Sync>;

/// Reserved `kind` that resolves to a preconfigured component stored in the `EntityStore`
/// instead of invoking a factory. `params` must deserialize to `{ "id": "<preconfigured id>" }`.
pub const PRECONFIGURED_REF_KIND: &str = "ref";

#[derive(Deserialize)]
struct PreconfiguredRef {
    id: String,
}

pub struct ComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
    entity_store: Arc<dyn EntityStore>,
}

impl std::fmt::Debug for ComponentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("factory_count", &self.factories.len())
            .finish()
    }
}

impl ComponentRegistry {
    pub fn new(entity_store: Arc<dyn EntityStore>) -> Self {
        Self {
            factories: HashMap::new(),
            entity_store,
        }
    }

    pub fn register(&mut self, kind: &str, factory: ComponentFactory) {
        self.factories.insert(kind.to_string(), factory);
    }

    /// Builds a component node. `kind: "ref"` nodes are resolved by fetching the preconfigured
    /// descriptor from the EntityStore, fresh on every call, using the ComponentContext of
    /// whichever component graph is referencing it. This ensures things like secret resolution
    /// run under the actual caller's ComponentContext rather than a synthetic one, at the cost of
    /// not sharing a single instance across references (e.g. a preconfigured `mcp_tool` re-runs
    /// discovery for every personality that references it).
    async fn build(&self, node: &ComponentDescriptor, context: &ComponentContext) -> ComponentResult<ComponentFacets> {
        if node.kind == PRECONFIGURED_REF_KIND {
            let target: PreconfiguredRef = serde_json::from_value(node.params.clone())
                .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            let descriptor = self
                .entity_store
                .get_component(&context.security_context, &target.id)
                .await
                .map_err(|e| ComponentError::InvalidConfig(format!("no preconfigured component '{}': {}", target.id, e)))?;
            if descriptor.kind == PRECONFIGURED_REF_KIND {
                return Err(ComponentError::InvalidConfig(format!(
                    "preconfigured component '{}' cannot itself be a ref",
                    descriptor.id
                )));
            }
            return self.dispatch(&descriptor, context);
        }

        self.dispatch(node, context)
    }

    fn dispatch(&self, node: &ComponentDescriptor, context: &ComponentContext) -> ComponentResult<ComponentFacets> {
        let Some(factory) = self.factories.get(&node.kind) else {
            return Err(ComponentError::UnsupportedKind(node.kind.clone()));
        };
        factory(node, context)
    }
}


#[derive(Debug)]
struct ComponentNode {
    id: String,
    fail_policy: ComponentFailPolicy,
    tags: Vec<String>,
    facets: ComponentFacets,
}

#[derive(Debug, Clone)]
pub struct ComponentImpl<T>
where
    T: ?Sized + Send + Sync,
{
    pub id: String,
    pub fail_policy: ComponentFailPolicy,
    pub facet: Arc<T>,
}

#[derive(Debug, Default)]
pub struct ComponentContainer {
    nodes: HashMap<String, ComponentNode>,
    node_order: Vec<String>,
}

impl ComponentContainer {
    /// Builds the components of `config` in dependency order with the
    /// factories of `registry`. Each component gets `context` with its own
    /// dependencies as `deps`; whatever `deps` `context` carries is replaced,
    /// not merged.
    pub async fn from_config(
        config: &ComponentConfig,
        registry: &ComponentRegistry,
        context: &ComponentContext,
    ) -> ComponentResult<Self> {

        let mut indegree: HashMap<String, usize> = HashMap::new();
        let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
        let mut config_by_id: HashMap<String, &ComponentDescriptor> = HashMap::new();

        for node in &config.component_nodes {
            if config_by_id.insert(node.id.clone(), node).is_some() {
                return Err(ComponentError::DuplicateId(node.id.clone()));
            }
            indegree.insert(node.id.clone(), 0);
            outgoing.entry(node.id.clone()).or_default();
        }

        for node in &config.component_nodes {
            for dep in &node.deps {
                if !config_by_id.contains_key(dep) {
                    return Err(ComponentError::UnknownDependency { component: node.id.clone(), dependency: dep.clone() });
                }
                *indegree.get_mut(&node.id).unwrap() += 1;
                outgoing.entry(dep.clone()).or_default().push(node.id.clone());
            }
        }

        let mut queue: VecDeque<String> = indegree
            .iter()
            .filter_map(|(id, degree)| if *degree == 0 { Some(id.clone()) } else { None })
            .collect();
        let mut ordered = Vec::with_capacity(config.component_nodes.len());

        while let Some(id) = queue.pop_front() {
            ordered.push(id.clone());
            if let Some(children) = outgoing.get(&id) {
                for child in children {
                    let degree = indegree.get_mut(child).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }

        if ordered.len() != config.component_nodes.len() {
            return Err(ComponentError::CycleDetected);
        }

        let mut nodes: HashMap<String, ComponentNode> = HashMap::new();
        for id in ordered.iter().cloned() {
            let node_config = config_by_id.get(&id).unwrap();

            let mut dep_facets = HashMap::new();
            for dep_id in &node_config.deps {
                let dep_node = nodes.get(dep_id).ok_or_else(|| ComponentError::UnknownDependency { component: id.clone(), dependency: dep_id.clone() })?;
                dep_facets.insert(dep_id.clone(), dep_node.facets.clone());
            }

            let context = ComponentContext { deps: dep_facets, ..context.clone() };

            let facets = registry.build(node_config, &context).await
                .map_err(|e| ComponentError::Build {
                    component: id.clone(),
                    kind: node_config.kind.clone(),
                    cause: Box::new(e),
                })?;
            nodes.insert(
                id.clone(),
                ComponentNode {
                    id,
                    fail_policy: node_config.fail_policy,
                    tags: node_config.tags.clone(),
                    facets,
                },
            );
        }

        Ok(Self {
            nodes,
            node_order: ordered,
        })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    fn nodes(
        &self,
        tag: &str,
    ) -> ComponentResult<Vec<&ComponentNode>> {
        let mut nodes = Vec::new();
        for id in &self.node_order {
            let node = self
                .nodes
                .get(id)
                .ok_or_else(|| ComponentError::NodeNotFound(id.clone()))?;
            if node.tags.iter().any(|t| t == tag) {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }


    pub fn get_all<T>(&self, tag: Option<&str>) -> ComponentResult<Vec<ComponentImpl<T>>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let nodes = match tag {
            Some(s) => self.nodes(s)?,
            None => self
                .node_order
                .iter()
                .map(|id| {
                    self.nodes
                        .get(id)
                        .ok_or_else(|| ComponentError::NodeNotFound(id.clone()))
                })
                .collect::<ComponentResult<Vec<_>>>()?,
        };

        Ok(nodes
            .into_iter()
            .filter_map(|n| {
                n.facets.get::<T>().map(|processor| ComponentImpl {
                    id: n.id.clone(),
                    fail_policy: n.fail_policy,
                    facet: processor,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::session::SessionId;
    use caelumora_model::store_impl::memory::MemoryStore;
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
        ComponentContext::new(
            SecurityContext { session_id, ..SecurityContext::default() },
            Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new()))),
            None,
        )
    }

    fn descriptor(id: &str, kind: &str, params: serde_json::Value) -> ComponentDescriptor {
        ComponentDescriptor {
            security_attributes: Default::default(),
            id: id.to_string(),
            kind: kind.to_string(),
            params,
            deps: vec![],
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }
    }

    #[tokio::test]
    async fn ref_kind_builds_fresh_using_the_callers_context_each_time() {
        let build_count = Arc::new(AtomicUsize::new(0));
        let counted = build_count.clone();
        let seen_sessions: Arc<Mutex<Vec<SessionId>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen_sessions.clone();

        let store = Arc::new(MemoryStore::new());
        store
            .put_component(&SecurityContext::default(), &descriptor("shared-marker", "counter", json!({})))
            .await
            .unwrap();

        let mut registry = ComponentRegistry::new(store);
        registry.register(
            "counter",
            Box::new(move |_node, context| {
                counted.fetch_add(1, Ordering::SeqCst);
                recorded.lock().unwrap().push(context.security_context.session_id);
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Marker>(Arc::new(MarkerImpl));
                Ok(facets)
            }),
        );

        let ref_a = descriptor("ref-a", PRECONFIGURED_REF_KIND, json!({ "id": "shared-marker" }));
        let ref_b = descriptor("ref-b", PRECONFIGURED_REF_KIND, json!({ "id": "shared-marker" }));

        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let facets_a = registry.build(&ref_a, &context_with_session(session_a)).await.unwrap();
        let facets_b = registry.build(&ref_b, &context_with_session(session_b)).await.unwrap();

        assert_eq!(build_count.load(Ordering::SeqCst), 2, "each ref must build fresh, not reuse a cached instance");
        assert_eq!(*seen_sessions.lock().unwrap(), vec![session_a, session_b], "the factory must see the caller's own context, not a synthetic one");
        assert!(!Arc::ptr_eq(&facets_a.get::<dyn Marker>().unwrap(), &facets_b.get::<dyn Marker>().unwrap()));
    }

    #[tokio::test]
    async fn ref_kind_errors_on_unknown_id() {
        let store = Arc::new(MemoryStore::new());
        let registry = ComponentRegistry::new(store);
        let node = descriptor("ref-a", PRECONFIGURED_REF_KIND, json!({ "id": "does-not-exist" }));
        let result = registry.build(&node, &test_context()).await;
        assert!(matches!(result, Err(ComponentError::InvalidConfig(_))));
    }

    #[tokio::test]
    async fn ref_kind_rejects_a_preconfigured_descriptor_that_is_itself_a_ref() {
        let store = Arc::new(MemoryStore::new());
        store
            .put_component(
                &SecurityContext::default(),
                &descriptor("a", PRECONFIGURED_REF_KIND, json!({ "id": "b" })),
            )
            .await
            .unwrap();

        let registry = ComponentRegistry::new(store);
        let node = descriptor("ref-a", PRECONFIGURED_REF_KIND, json!({ "id": "a" }));
        let result = registry.build(&node, &test_context()).await;
        assert!(matches!(result, Err(ComponentError::InvalidConfig(_))));
    }
}
