//! The workflows of ComfyUI the operator configured (issue 512): read from the directory the
//! section `comfyui` names, each a graph in ComfyUI's API format with what it says of itself
//! beside it, checked whole when the runtime is built and found by name.
//!
//! A workflow is a pair of files of one name: `<name>.json`, the graph as ComfyUI exports it
//! ("Export (API)") - what `POST /prompt` takes, unchanged -, and `<name>.yaml`, which describes it
//! for a model and says to which inputs of its nodes the parameters of the tool go (`Parameter`),
//! within which bounds of size, and which node gives the images. What is not checked can only be
//! told by a server - whether a node is one ComfyUI has, whether the one named gives images.

use caelumora_agents::tool::{ToolError, ToolResult};
use caelumora_model::config::parse_yaml;
use caelumora_model::value::identifier::Identifier;
use serde::de;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// Why the workflows could not be read: naming the workflow or its file, never a path beyond it.
#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("the directory of the workflows cannot be read: {0}")]
    Unreadable(String),
    #[error("the workflow file '{file}' cannot be read: {cause}")]
    FileUnreadable { file: String, cause: String },
    #[error("'{0}' names no workflow: lower case letters, digits, '_' and '-'")]
    Named(String),
    #[error("the workflow '{name}' has no file '{name}.{missing}' beside the other")]
    Unpaired { name: String, missing: &'static str },
    #[error("the workflow file '{file}' is not what a workflow is: {cause}")]
    Invalid { file: String, cause: String },
    #[error("the workflow '{0}' maps no prompt")]
    NoPrompt(String),
    #[error(
        "the workflow '{workflow}' maps {parameter} to '{input}', which its graph does not have"
    )]
    NoSuchInput { workflow: String, parameter: Parameter, input: Input },
    #[error("the workflow '{workflow}' maps {parameter} to '{input}', which takes no {wanted}")]
    WrongType { workflow: String, parameter: Parameter, input: Input, wanted: &'static str },
    #[error("the workflow '{workflow}' takes its images from node '{node}', which its graph does not have")]
    NoSuchNode { workflow: String, node: String },
    #[error("the workflow '{0}' bounds its size so that no size fits")]
    NoSize(String),
    #[error("the workflow '{workflow}' sets its {parameter} to {value}, outside its own bounds")]
    OutOfBounds { workflow: String, parameter: Parameter, value: u64 },
}

/// A parameter of the tool that a workflow may map to an input of one of its nodes: the tool's,
/// the same for every workflow, which may leave one unused - all but the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Parameter {
    Prompt,
    NegativePrompt,
    Width,
    Height,
    Seed,
}

impl Parameter {
    /// What the input it goes to has to hold: a text, or a whole number.
    fn wanted(self) -> &'static str {
        match self {
            Parameter::Prompt | Parameter::NegativePrompt => "text",
            Parameter::Width | Parameter::Height | Parameter::Seed => "whole number",
        }
    }

    /// Whether `value`, an input's value in a graph, is what it goes to - never a link to another
    /// node's output, which a value cannot replace.
    fn fits(self, value: &Value) -> bool {
        match self {
            Parameter::Prompt | Parameter::NegativePrompt => value.is_string(),
            Parameter::Width | Parameter::Height | Parameter::Seed => value.is_u64(),
        }
    }
}

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Parameter::Prompt => "prompt",
            Parameter::NegativePrompt => "negative_prompt",
            Parameter::Width => "width",
            Parameter::Height => "height",
            Parameter::Seed => "seed",
        })
    }
}

/// An input of a node of a graph, written `<node id>.<input>`: `6.text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    node: String,
    input: String,
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.node, self.input)
    }
}

impl<'de> Deserialize<'de> for Input {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        match text.split_once('.') {
            Some((node, input)) if !node.is_empty() && !input.is_empty() => {
                Ok(Self { node: node.to_string(), input: input.to_string() })
            }
            _ => Err(de::Error::custom(format!("'{text}' is no input: written <node id>.<input>"))),
        }
    }
}

/// The seeds chosen at random lie below: exact as a number in JSON and in a browser's JavaScript.
pub(crate) const MAX_SEED: u64 = 1 << 53;

/// What a call asks of a workflow, as the tool's parameters say it: what it leaves out, the
/// workflow's graph says - but the seed, which is chosen at random, so that another call gives
/// another image.
#[derive(Debug, Deserialize)]
pub(crate) struct Arguments {
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) negative_prompt: Option<String>,
    #[serde(default)]
    pub(crate) width: Option<u64>,
    #[serde(default)]
    pub(crate) height: Option<u64>,
    #[serde(default = "random_seed")]
    pub(crate) seed: u64,
}

fn random_seed() -> u64 {
    rand::random_range(0..MAX_SEED)
}

/// The sizes a workflow takes, in pixels per side: from `min` to `max`, a multiple of
/// `multiple_of`. Left out, or in part, what most models take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SizeBounds {
    min: u64,
    max: u64,
    multiple_of: u64,
}

impl Default for SizeBounds {
    fn default() -> Self {
        Self { min: 256, max: 2048, multiple_of: 16 }
    }
}

impl SizeBounds {
    /// Whether `side` is a size it takes.
    fn takes(&self, side: u64) -> bool {
        (self.min..=self.max).contains(&side) && side.is_multiple_of(self.multiple_of)
    }

    /// Whether any size fits: the first multiple from `min` on is not beyond `max`.
    fn admits_any(&self) -> bool {
        self.multiple_of > 0
            && self
                .min
                .div_ceil(self.multiple_of)
                .checked_mul(self.multiple_of)
                .is_some_and(|first| first <= self.max)
    }
}

/// What the YAML beside a workflow says of it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Described {
    description: String,
    parameters: BTreeMap<Parameter, Input>,
    #[serde(default)]
    size: SizeBounds,
    #[serde(default, deserialize_with = "node_id")]
    output: Option<String>,
}

/// A node's id, as YAML may write it: `9` or `"9"`.
fn node_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        None => None,
        Some(Value::String(id)) => Some(id),
        Some(Value::Number(id)) => Some(id.to_string()),
        Some(other) => {
            return Err(de::Error::custom(format!("'{other}' is no node's id")));
        }
    })
}

/// One workflow: its graph as exported, what it says of itself, and where the tool's parameters
/// go.
#[derive(Debug)]
pub struct Workflow {
    name: Identifier,
    description: String,
    graph: Map<String, Value>,
    inputs: BTreeMap<Parameter, Input>,
    size: SizeBounds,
    output: Option<String>,
}

impl Workflow {
    /// The workflow `name` of the graph `graph` and what `described` says of it, checked: the
    /// prompt mapped; every input mapped there, of a value its parameter fits; its own width and
    /// height within its bounds, which some size fits; the node its images come from there.
    ///
    /// # Errors
    /// `Invalid` for a graph that is not an object of nodes; `NoPrompt`, `NoSuchInput`,
    /// `WrongType`, `NoSize`, `OutOfBounds`, `NoSuchNode`.
    fn checked(
        name: &Identifier,
        graph: Value,
        described: Described,
    ) -> Result<Self, WorkflowError> {
        let Value::Object(graph) = graph else {
            return Err(WorkflowError::Invalid {
                file: format!("{name}.json"),
                cause: "no object of nodes".to_string(),
            });
        };
        if !described.parameters.contains_key(&Parameter::Prompt) {
            return Err(WorkflowError::NoPrompt(name.to_string()));
        }
        if !described.size.admits_any() {
            return Err(WorkflowError::NoSize(name.to_string()));
        }
        for (parameter, input) in &described.parameters {
            let Some(value) = graph
                .get(&input.node)
                .and_then(|node| node.get("inputs"))
                .and_then(|inputs| inputs.get(&input.input))
            else {
                return Err(WorkflowError::NoSuchInput {
                    workflow: name.to_string(),
                    parameter: *parameter,
                    input: input.clone(),
                });
            };
            if !parameter.fits(value) {
                return Err(WorkflowError::WrongType {
                    workflow: name.to_string(),
                    parameter: *parameter,
                    input: input.clone(),
                    wanted: parameter.wanted(),
                });
            }
            if let (Parameter::Width | Parameter::Height, Some(side)) = (parameter, value.as_u64())
            {
                if !described.size.takes(side) {
                    return Err(WorkflowError::OutOfBounds {
                        workflow: name.to_string(),
                        parameter: *parameter,
                        value: side,
                    });
                }
            }
        }
        if let Some(node) = described.output.as_ref().filter(|node| !graph.contains_key(*node)) {
            return Err(WorkflowError::NoSuchNode {
                workflow: name.to_string(),
                node: node.clone(),
            });
        }
        Ok(Self {
            name: name.clone(),
            description: described.description,
            graph,
            inputs: described.parameters,
            size: described.size,
            output: described.output,
        })
    }

    /// Its graph with `arguments` written into the inputs it maps, those it does not map left out:
    /// what `POST /prompt` is given.
    ///
    /// # Errors
    /// `InvalidParameter` for a width or height outside its bounds, which it names.
    pub(crate) fn graph_with(&self, arguments: &Arguments) -> ToolResult<Map<String, Value>> {
        let SizeBounds { min, max, multiple_of } = self.size;
        for (side, value) in [("width", arguments.width), ("height", arguments.height)] {
            if let Some(value) = value.filter(|value| !self.size.takes(*value)) {
                return Err(ToolError::InvalidParameter(format!(
                    "{side} {value}: the workflow '{}' takes {min} to {max}, a multiple of \
                     {multiple_of}",
                    self.name
                )));
            }
        }
        let mut graph = self.graph.clone();
        for (parameter, input) in &self.inputs {
            let value = match parameter {
                Parameter::Prompt => Some(Value::from(arguments.prompt.clone())),
                Parameter::NegativePrompt => arguments.negative_prompt.clone().map(Value::from),
                Parameter::Width => arguments.width.map(Value::from),
                Parameter::Height => arguments.height.map(Value::from),
                Parameter::Seed => Some(Value::from(arguments.seed)),
            };
            let inputs = graph.get_mut(&input.node).and_then(|node| node.get_mut("inputs"));
            if let (Some(value), Some(Value::Object(inputs))) = (value, inputs) {
                inputs.insert(input.input.clone(), value);
            }
        }
        Ok(graph)
    }

    /// The node whose images are the result, if it names one; else every node's are.
    pub(crate) fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// How it is named: by its files, and to a model.
    pub fn name(&self) -> &Identifier {
        &self.name
    }

    /// What it is for, as the tool's description lists it.
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// The workflows the operator configured, read from one directory and checked whole, found by
/// name.
#[derive(Debug, Default)]
pub struct Workflows {
    by_name: BTreeMap<Identifier, Arc<Workflow>>,
}

impl Workflows {
    /// The workflows in `dir`: each `<name>.json` in ComfyUI's API format with `<name>.yaml` beside
    /// it, named by them. Other files are not read.
    ///
    /// # Errors
    /// `Unreadable`, `FileUnreadable`; `Named` for a name that is no identifier; `Unpaired` for a
    /// file without the other; `Invalid` for one that does not parse as what it is to be; else as
    /// a workflow is checked.
    pub fn read(dir: &Path) -> Result<Self, WorkflowError> {
        let unreadable = |e: io::Error| WorkflowError::Unreadable(e.kind().to_string());
        let mut names: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
        for entry in fs::read_dir(dir).map_err(unreadable)? {
            let path = entry.map_err(unreadable)?.path();
            let Some(kind) = ["json", "yaml"]
                .into_iter()
                .find(|kind| path.extension().is_some_and(|extension| extension == *kind))
            else {
                continue;
            };
            let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            names.entry(name.to_string()).or_default().insert(kind);
        }
        let mut by_name = BTreeMap::new();
        for (name, kinds) in names {
            let Ok(name) = Identifier::new(&name) else {
                return Err(WorkflowError::Named(name));
            };
            if let Some(missing) = ["json", "yaml"].into_iter().find(|kind| !kinds.contains(kind)) {
                return Err(WorkflowError::Unpaired { name: name.to_string(), missing });
            }
            let workflow = Self::read_one(dir, &name)?;
            by_name.insert(name, Arc::new(workflow));
        }
        Ok(Self { by_name })
    }

    /// The workflow `name`, of its two files in `dir`.
    fn read_one(dir: &Path, name: &Identifier) -> Result<Workflow, WorkflowError> {
        let text = |kind: &str| {
            let file = format!("{name}.{kind}");
            fs::read_to_string(dir.join(&file))
                .map_err(|e| WorkflowError::FileUnreadable { file, cause: e.kind().to_string() })
        };
        let invalid = |kind: &str, cause: String| WorkflowError::Invalid {
            file: format!("{name}.{kind}"),
            cause,
        };
        let graph: Value =
            serde_json::from_str(&text("json")?).map_err(|e| invalid("json", e.to_string()))?;
        let described: Described =
            parse_yaml(&text("yaml")?).map_err(|e| invalid("yaml", e.to_string()))?;
        Workflow::checked(name, graph, described)
    }

    /// The workflow named `name`, if one is.
    pub fn named(&self, name: &str) -> Option<&Arc<Workflow>> {
        self.by_name.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// The graph of a workflow with a prompt (6), a latent of 1024 by 1024 (13), a sampler (3)
    /// and what saves the images (9).
    fn graph() -> Value {
        json!({
            "6": {
                "class_type": "CLIPTextEncode",
                "inputs": { "text": "a cat", "clip": ["30", 0] }
            },
            "13": {
                "class_type": "EmptySD3LatentImage",
                "inputs": { "width": 1024, "height": 1024, "batch_size": 1 }
            },
            "3": { "class_type": "KSampler", "inputs": { "seed": 0, "steps": 8 } },
            "9": {
                "class_type": "SaveImage",
                "inputs": { "images": ["8", 0], "filename_prefix": "x" }
            }
        })
    }

    const DESCRIBED: &str = "description: fast\nparameters:\n  prompt: 6.text\n  width: 13.width\n  height: 13.height\n  seed: 3.seed\noutput: 9\n";

    /// A directory with the workflow `name` of `graph` and `described`, and the files `others`.
    fn dir_with(name: &str, graph: &Value, described: &str, others: &[(&str, &str)]) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(format!("{name}.json")), graph.to_string()).unwrap();
        fs::write(dir.path().join(format!("{name}.yaml")), described).unwrap();
        for (file, content) in others {
            fs::write(dir.path().join(file), content).unwrap();
        }
        dir
    }

    fn read(graph: &Value, described: &str) -> Result<Workflows, WorkflowError> {
        Workflows::read(dir_with("z_image", graph, described, &[]).path())
    }

    #[test]
    fn a_workflow_is_its_graph_and_what_it_says_of_itself_found_by_name() {
        let dir = dir_with("z_image", &graph(), DESCRIBED, &[("notes.md", "not read")]);
        let workflows = Workflows::read(dir.path()).expect("one workflow");
        let workflow = workflows.named("z_image").expect("by its name");
        assert_eq!((workflow.name().as_str(), workflow.description()), ("z_image", "fast"));
        assert!(workflows.named("flux").is_none());
        assert!(Workflows::default().named("z_image").is_none(), "none without a directory");
    }

    #[test]
    fn what_a_workflow_maps_is_there_and_of_its_type() {
        let refused = |described: &str| read(&graph(), described).unwrap_err();
        assert!(matches!(
            refused("description: x\nparameters:\n  seed: 3.seed\n"),
            WorkflowError::NoPrompt(_)
        ));
        assert!(matches!(
            refused("description: x\nparameters:\n  prompt: 7.text\n"),
            WorkflowError::NoSuchInput { parameter: Parameter::Prompt, .. }
        ));
        assert!(
            matches!(
                refused("description: x\nparameters:\n  prompt: 6.clip\n"),
                WorkflowError::WrongType { parameter: Parameter::Prompt, .. }
            ),
            "a link to another node takes no value"
        );
        assert!(matches!(
            refused("description: x\nparameters:\n  prompt: 6.text\n  seed: 6.text\n"),
            WorkflowError::WrongType { parameter: Parameter::Seed, wanted: "whole number", .. }
        ));
        assert!(matches!(
            refused("description: x\nparameters:\n  prompt: 6.text\noutput: 8\n"),
            WorkflowError::NoSuchNode { .. }
        ));
        assert!(
            matches!(
                refused("description: x\nparameters:\n  prompt: 6.text\n  steps: 3.steps\n"),
                WorkflowError::Invalid { .. }
            ),
            "no parameter of the tool"
        );
        assert!(
            matches!(
                refused("description: x\nparameters:\n  prompt: text\n"),
                WorkflowError::Invalid { .. }
            ),
            "no input"
        );
        assert!(matches!(read(&json!([1]), DESCRIBED).unwrap_err(), WorkflowError::Invalid { .. }));
    }

    #[test]
    fn a_workflows_own_size_lies_within_bounds_some_size_fits() {
        let bounded = |size: &str| read(&graph(), &format!("{DESCRIBED}size: {size}\n"));
        let small = bounded("{ max: 768 }").unwrap_err();
        assert!(
            matches!(
                small,
                WorkflowError::OutOfBounds { parameter: Parameter::Width, value: 1024, .. }
            ),
            "{small}"
        );
        assert!(matches!(bounded("{ min: 300, max: 302 }").unwrap_err(), WorkflowError::NoSize(_)));
        assert!(matches!(bounded("{ multiple_of: 0 }").unwrap_err(), WorkflowError::NoSize(_)));
        let far = "{ min: 1, max: 18446744073709551614, multiple_of: 18446744073709551615 }";
        assert!(
            matches!(bounded(far).unwrap_err(), WorkflowError::NoSize(_)),
            "found at once, however far"
        );
        assert!(bounded("{ max: 1536, multiple_of: 64 }").is_ok(), "given in part");
        let mut wide = graph();
        wide["13"]["inputs"]["width"] = json!(4096);
        assert!(
            matches!(
                read(&wide, DESCRIBED).unwrap_err(),
                WorkflowError::OutOfBounds { parameter: Parameter::Width, value: 4096, .. }
            ),
            "the bounds most models take, left out: up to 2048"
        );
        let fits = |size: SizeBounds, side| size.takes(side);
        let size = SizeBounds { min: 256, max: 1536, multiple_of: 64 };
        assert!(fits(size, 1536) && !fits(size, 1000) && !fits(size, 2048));
    }

    #[test]
    fn a_workflow_is_two_files_of_a_name_that_is_an_identifier() {
        let dir = dir_with("z_image", &graph(), DESCRIBED, &[("flux.json", "{}")]);
        assert!(matches!(
            Workflows::read(dir.path()).unwrap_err(),
            WorkflowError::Unpaired { missing: "yaml", .. }
        ));
        let dir = dir_with("Z Image", &graph(), DESCRIBED, &[]);
        assert!(matches!(Workflows::read(dir.path()).unwrap_err(), WorkflowError::Named(_)));
        let missing = Workflows::read(&dir.path().join("none")).unwrap_err();
        assert!(matches!(missing, WorkflowError::Unreadable(_)));
        assert!(!missing.to_string().contains("none"), "no path: {missing}");
    }
}
