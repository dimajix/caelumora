use crate::runtime::ActorRuntime;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

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
}

pub type ComponentResult<T> = Result<T, ComponentError>;


#[derive(Debug, Clone)]
pub struct ComponentContext {
    pub deps: HashMap<String, ComponentFacets>,
    pub security_context: SecurityContext,
    pub secret_manager: Arc<SecretManager>,
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

/// Reserved `kind` that resolves to a preconfigured component (see `ComponentRegistry::configure`)
/// instead of invoking a factory. `params` must deserialize to `{ "id": "<preconfigured id>" }`.
pub const PRECONFIGURED_REF_KIND: &str = "ref";

#[derive(Deserialize)]
struct PreconfiguredRef {
    id: String,
}

#[derive(Default)]
pub struct ComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
    preconfigured: HashMap<String, ComponentDescriptor>,
}

impl std::fmt::Debug for ComponentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("factory_count", &self.factories.len())
            .field("preconfigured_count", &self.preconfigured.len())
            .finish()
    }
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, kind: &str, factory: ComponentFactory) {
        self.factories.insert(kind.to_string(), factory);
    }

    /// Registers descriptors that can be referenced elsewhere via `kind: "ref"`. Descriptors are
    /// *not* built here — each `ref` is built fresh, on demand, inside `build()`, using the
    /// ComponentContext of whichever component graph is referencing it. This ensures things like
    /// secret resolution run under the actual caller's ComponentContext rather than a synthetic
    /// one, at the cost of not sharing a single instance across references (e.g. a preconfigured
    /// `mcp_tool` re-runs discovery for every personality that references it).
    pub fn configure(&mut self, descriptors: &[ComponentDescriptor]) -> ComponentResult<()> {
        for descriptor in descriptors {
            if descriptor.kind == PRECONFIGURED_REF_KIND {
                return Err(ComponentError::InvalidConfig(format!(
                    "preconfigured component '{}' cannot itself be a ref",
                    descriptor.id
                )));
            }
            if self.preconfigured.contains_key(&descriptor.id) {
                return Err(ComponentError::DuplicateId(descriptor.id.clone()));
            }
            self.preconfigured.insert(descriptor.id.clone(), descriptor.clone());
        }
        Ok(())
    }

    fn build(&self, node: &ComponentDescriptor, context: &ComponentContext) -> ComponentResult<ComponentFacets> {
        if node.kind == PRECONFIGURED_REF_KIND {
            let target: PreconfiguredRef = serde_json::from_value(node.params.clone())
                .map_err(|e| ComponentError::InvalidConfig(e.to_string()))?;
            let descriptor = self
                .preconfigured
                .get(&target.id)
                .ok_or_else(|| ComponentError::InvalidConfig(format!("no preconfigured component '{}'", target.id)))?;
            return self.build(descriptor, context);
        }

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
    pub fn from_config(
        config: &ComponentConfig,
        actor_runtime: &ActorRuntime,
        security_context: SecurityContext,
    ) -> ComponentResult<Self> {
        let registry = actor_runtime.component_registry();
        let secret_manager = actor_runtime.secret_manager();

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

            let context = ComponentContext {
                deps: dep_facets,
                security_context: security_context.clone(),
                secret_manager: secret_manager.clone(),
            };

            let facets = registry.build(node_config, &context)?;
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
        ComponentContext {
            deps: HashMap::new(),
            security_context: SecurityContext {
                session_id,
                ..SecurityContext::default()
            },
            secret_manager: Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new()))),
        }
    }

    fn descriptor(id: &str, kind: &str, params: serde_json::Value) -> ComponentDescriptor {
        ComponentDescriptor {
            id: id.to_string(),
            kind: kind.to_string(),
            params,
            deps: vec![],
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailOpen,
        }
    }

    #[test]
    fn ref_kind_builds_fresh_using_the_callers_context_each_time() {
        let build_count = Arc::new(AtomicUsize::new(0));
        let counted = build_count.clone();
        let seen_sessions: Arc<Mutex<Vec<SessionId>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen_sessions.clone();

        let mut registry = ComponentRegistry::new();
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

        registry.configure(&[descriptor("shared-marker", "counter", json!({}))]).unwrap();

        let ref_a = descriptor("ref-a", PRECONFIGURED_REF_KIND, json!({ "id": "shared-marker" }));
        let ref_b = descriptor("ref-b", PRECONFIGURED_REF_KIND, json!({ "id": "shared-marker" }));

        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let facets_a = registry.build(&ref_a, &context_with_session(session_a)).unwrap();
        let facets_b = registry.build(&ref_b, &context_with_session(session_b)).unwrap();

        assert_eq!(build_count.load(Ordering::SeqCst), 2, "each ref must build fresh, not reuse a cached instance");
        assert_eq!(*seen_sessions.lock().unwrap(), vec![session_a, session_b], "the factory must see the caller's own context, not a synthetic one");
        assert!(!Arc::ptr_eq(&facets_a.get::<dyn Marker>().unwrap(), &facets_b.get::<dyn Marker>().unwrap()));
    }

    #[test]
    fn ref_kind_errors_on_unknown_id() {
        let registry = ComponentRegistry::new();
        let node = descriptor("ref-a", PRECONFIGURED_REF_KIND, json!({ "id": "does-not-exist" }));
        let result = registry.build(&node, &test_context());
        assert!(matches!(result, Err(ComponentError::InvalidConfig(_))));
    }

    #[test]
    fn configure_rejects_duplicate_ids() {
        let mut registry = ComponentRegistry::new();
        registry.register("counter", Box::new(|_node, _context| Ok(ComponentFacets::empty())));

        let result = registry.configure(&[
            descriptor("dup", "counter", json!({})),
            descriptor("dup", "counter", json!({})),
        ]);
        assert!(matches!(result, Err(ComponentError::DuplicateId(id)) if id == "dup"));
    }

    #[test]
    fn configure_rejects_ref_kind_as_a_preconfigured_descriptor() {
        let mut registry = ComponentRegistry::new();
        let result = registry.configure(&[descriptor("a", PRECONFIGURED_REF_KIND, json!({ "id": "b" }))]);
        assert!(matches!(result, Err(ComponentError::InvalidConfig(_))));
    }
}
