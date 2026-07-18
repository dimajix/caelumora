use crate::runtime::ActorRuntime;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::SecurityContext;
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

#[derive(Default)]
pub struct ComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
}

impl std::fmt::Debug for ComponentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("factory_count", &self.factories.len())
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

    fn build(&self, node: &ComponentDescriptor, context: &ComponentContext) -> ComponentResult<ComponentFacets> {
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
