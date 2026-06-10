use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::agent::{ComponentFailPolicy, ComponentNodeConfig, PipelineConfig, PipelineStage};
use caelumora_model::message::Message;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

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
    pub deps: Vec<String>,
}

#[async_trait]
pub trait MessageProcessor: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn process_messages(&self, input: Vec<Message>, context: &mut TurnContext) -> Result<Vec<Message>>;
}

pub trait PerceiveInProcessor: MessageProcessor {}
pub trait HistoryWriteProcessor: MessageProcessor {}
pub trait ContextBuildProcessor: MessageProcessor {}
pub trait PlanPreProcessor: MessageProcessor {}
pub trait EmitOutProcessor: MessageProcessor {}

#[derive(Debug, Clone)]
pub struct ComponentCapabilities {
    pub perceive_in: Option<Arc<dyn PerceiveInProcessor>>,
    pub history_write: Option<Arc<dyn HistoryWriteProcessor>>,
    pub context_build: Option<Arc<dyn ContextBuildProcessor>>,
    pub plan_pre: Option<Arc<dyn PlanPreProcessor>>,
    pub emit_out: Option<Arc<dyn EmitOutProcessor>>,
}

impl ComponentCapabilities {
    pub fn empty() -> Self {
        Self {
            perceive_in: None,
            history_write: None,
            context_build: None,
            plan_pre: None,
            emit_out: None,
        }
    }
}

pub type ComponentFactory = Arc<dyn Fn(ComponentDescriptor) -> Result<ComponentCapabilities> + Send + Sync>;

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

    fn build(&self, node: &ComponentNodeConfig) -> Result<ComponentCapabilities> {
        let Some(factory) = self.factories.get(&node.kind) else {
            return Err(anyhow!("Unsupported component kind: {}", node.kind));
        };
        factory(ComponentDescriptor {
            id: node.id.clone(),
            kind: node.kind.clone(),
            params: node.params.clone(),
            deps: node.deps.clone(),
        })
    }
}

#[derive(Debug)]
struct ComponentNode {
    id: String,
    fail_policy: ComponentFailPolicy,
    capabilities: ComponentCapabilities,
}

#[derive(Debug, Clone)]
pub struct StageProcessor<T>
where
    T: ?Sized + Send + Sync,
{
    pub id: String,
    pub fail_policy: ComponentFailPolicy,
    pub processor: Arc<T>,
}

#[derive(Debug, Default)]
pub struct ComponentContainer {
    nodes: HashMap<String, ComponentNode>,
    stages: HashMap<PipelineStage, Vec<String>>,
}

impl ComponentContainer {
    pub fn from_config(config: &PipelineConfig, registry: &ComponentRegistry) -> Result<Self> {
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

        let mut nodes = HashMap::new();
        for id in ordered {
            let node_config = config_by_id.get(&id).unwrap();
            let processor = registry.build(node_config)?;
            nodes.insert(
                id.clone(),
                ComponentNode {
                    id,
                    fail_policy: node_config.fail_policy,
                    capabilities: processor,
                },
            );
        }

        let mut stages: HashMap<PipelineStage, Vec<String>> = HashMap::new();
        for binding in &config.pipeline_bindings {
            for component_id in &binding.components {
                if !nodes.contains_key(component_id) {
                    return Err(anyhow!(
                        "Stage binding references unknown component '{}'",
                        component_id
                    ));
                }
            }
            stages
                .entry(binding.stage)
                .or_default()
                .extend(binding.components.clone());
        }

        Ok(Self { nodes, stages })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    fn stage_nodes(
        &self,
        stage: PipelineStage,
    ) -> Result<Vec<&ComponentNode>> {
        let Some(component_ids) = self.stages.get(&stage) else {
            return Ok(Vec::new());
        };

        let mut nodes = Vec::with_capacity(component_ids.len());
        for component_id in component_ids {
            let node = self
                .nodes
                .get(component_id)
                .ok_or_else(|| anyhow!("Unknown component node '{}'", component_id))?;
            nodes.push(node);
        }
        Ok(nodes)
    }

    pub fn perceive_in_processors(&self) -> Result<Vec<StageProcessor<dyn PerceiveInProcessor>>> {
        self.stage_nodes(PipelineStage::PerceiveIn).map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|n| {
                    n.capabilities.perceive_in.as_ref().map(|processor| StageProcessor {
                        id: n.id.clone(),
                        fail_policy: n.fail_policy,
                        processor: Arc::clone(processor),
                    })
                })
                .collect()
        })
    }

    pub fn history_write_processors(&self) -> Result<Vec<StageProcessor<dyn HistoryWriteProcessor>>> {
        self.stage_nodes(PipelineStage::HistoryWrite).map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|n| {
                    n.capabilities.history_write.as_ref().map(|processor| StageProcessor {
                        id: n.id.clone(),
                        fail_policy: n.fail_policy,
                        processor: Arc::clone(processor),
                    })
                })
                .collect()
        })
    }

    pub fn context_build_processors(&self) -> Result<Vec<StageProcessor<dyn ContextBuildProcessor>>> {
        self.stage_nodes(PipelineStage::ContextBuild).map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|n| {
                    n.capabilities.context_build.as_ref().map(|processor| StageProcessor {
                        id: n.id.clone(),
                        fail_policy: n.fail_policy,
                        processor: Arc::clone(processor),
                    })
                })
                .collect()
        })
    }

    pub fn plan_pre_processors(&self) -> Result<Vec<StageProcessor<dyn PlanPreProcessor>>> {
        self.stage_nodes(PipelineStage::PlanPre).map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|n| {
                    n.capabilities.plan_pre.as_ref().map(|processor| StageProcessor {
                        id: n.id.clone(),
                        fail_policy: n.fail_policy,
                        processor: Arc::clone(processor),
                    })
                })
                .collect()
        })
    }

    pub fn emit_out_processors(&self) -> Result<Vec<StageProcessor<dyn EmitOutProcessor>>> {
        self.stage_nodes(PipelineStage::EmitOut).map(|nodes| {
            nodes
                .into_iter()
                .filter_map(|n| {
                    n.capabilities.emit_out.as_ref().map(|processor| StageProcessor {
                        id: n.id.clone(),
                        fail_policy: n.fail_policy,
                        processor: Arc::clone(processor),
                    })
                })
                .collect()
        })
    }
}
