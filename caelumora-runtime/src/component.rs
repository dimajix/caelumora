use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::component::{ComponentFailPolicy, ComponentNodeConfig, ComponentConfig};
use caelumora_model::agent::{AgentState};
use caelumora_model::message::Message;
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const STATE_HISTORY_WRITER_KIND: &str = "state_history_writer";
pub const STATE_CONTEXT_APPENDER_KIND: &str = "state_context_appender";
pub const STATE_CONTEXT_PROVIDER_KIND: &str = "state_context_provider";

#[derive(Debug, Default, Clone)]
pub struct TurnContext {
    pub scratch: HashMap<String, Value>,
    pub links: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ComponentDescriptor {
    pub id: String,
    pub kind: String,
    pub params: Value,
    pub deps: HashMap<String, ComponentFacets>,
    pub state: Option<Arc<Mutex<AgentState>>>,
}

#[async_trait]
pub trait SystemPromptProvider: Send + Sync + std::fmt::Debug {
    async fn get_prompt(&self) -> Result<String>;
}

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn process_messages(&self, input: Vec<Message>, context: &mut TurnContext) -> Result<Vec<Message>>;
}

#[async_trait]
pub trait ContextProvider: Send + Sync + std::fmt::Debug {
    async fn current_context(&self) -> Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait PlanPreProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}

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

pub type ComponentFactory = Arc<dyn Fn(ComponentDescriptor) -> Result<ComponentFacets> + Send + Sync>;

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

    fn build(&self, node: &ComponentNodeConfig, state: Option<Arc<Mutex<AgentState>>>, dep_facets: HashMap<String, ComponentFacets>) -> Result<ComponentFacets> {
        let Some(factory) = self.factories.get(&node.kind) else {
            return Err(anyhow!("Unsupported component kind: {}", node.kind));
        };
        factory(ComponentDescriptor {
            id: node.id.clone(),
            kind: node.kind.clone(),
            params: node.params.clone(),
            deps: dep_facets,
            state,
        })
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
        registry: &ComponentRegistry,
        state: Option<AgentState>,
    ) -> Result<Self> {
        let mut indegree: HashMap<String, usize> = HashMap::new();
        let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
        let mut config_by_id: HashMap<String, &ComponentNodeConfig> = HashMap::new();

        for node in &config.component_nodes {
            if config_by_id.insert(node.id.clone(), node).is_some() {
                return Err(anyhow!("Duplicate component id: {}", node.id));
            }
            indegree.insert(node.id.clone(), 0);
            outgoing.entry(node.id.clone()).or_default();
        }

        for node in &config.component_nodes {
            for dep in &node.deps {
                if !config_by_id.contains_key(dep) {
                    return Err(anyhow!("Component '{}' depends on unknown component '{}'", node.id, dep));
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
            return Err(anyhow!("Cycle detected in component graph"));
        }

        let mut nodes: HashMap<String, ComponentNode> = HashMap::new();
        let state = state.map(|x| Arc::new(Mutex::new(x)));
        for id in ordered.iter().cloned() {
            let node_config = config_by_id.get(&id).unwrap();

            let mut dep_facets = HashMap::new();
            for dep_id in &node_config.deps {
                let dep_node = nodes.get(dep_id).ok_or_else(|| anyhow!("Dependency '{}' not found for component '{}'", dep_id, id))?;
                dep_facets.insert(dep_id.clone(), dep_node.facets.clone());
            }

            let facets = registry.build(node_config, state.clone(), dep_facets)?;
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
    ) -> Result<Vec<&ComponentNode>> {
        let mut nodes = Vec::new();
        for id in &self.node_order {
            let node = self
                .nodes
                .get(id)
                .ok_or_else(|| anyhow!("Unknown component node '{}'", id))?;
            if node.tags.iter().any(|t| t == tag) {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }


    pub fn get_all<T>(&self, tag: Option<&str>) -> Result<Vec<ComponentImpl<T>>>
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
                        .ok_or_else(|| anyhow!("Unknown component node '{}'", id))
                })
                .collect::<Result<Vec<_>>>()?,
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
