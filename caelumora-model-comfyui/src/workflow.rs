//! A workflow of ComfyUI the operator configured (issues 512, 525): what one image model of the
//! backend `comfyui` runs, a graph in ComfyUI's API format with what it says of itself beside it,
//! read from the path its model names and checked whole when the kernel is built.
//!
//! A workflow is a pair of files of one name: `<path>.json`, the graph as ComfyUI exports it
//! ("Export (API)") - what `POST /prompt` takes, unchanged -, and `<path>.yaml`, which describes it
//! for a model and says to which inputs of its nodes the parts of a request go (`Parameter`),
//! within which bounds of size, and which node gives the images. What is not checked can only be
//! told by a server - whether a node is one ComfyUI has, whether the one named gives images.

use caelumora_api::image::ImageRequest;
use caelumora_domain::config::parse_yaml;
use serde::de;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Why a workflow could not be read: naming its file or what in it is wrong, never a path beyond
/// it. Whose workflow it is, the model's backend says where it reports it.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkflowError {
    #[error("the workflow file '{file}' cannot be read: {cause}")]
    FileUnreadable { file: String, cause: String },
    #[error("the workflow file '{file}' is not what a workflow is: {cause}")]
    Invalid { file: String, cause: String },
    #[error("the workflow maps no prompt")]
    NoPrompt,
    #[error("the workflow maps {parameter} to '{input}', which its graph does not have")]
    NoSuchInput { parameter: Parameter, input: Input },
    #[error("the workflow maps {parameter} to '{input}', which takes no {wanted}")]
    WrongType { parameter: Parameter, input: Input, wanted: &'static str },
    #[error("the workflow takes its images from node '{0}', which its graph does not have")]
    NoSuchNode(String),
    #[error("the workflow bounds its size so that no size fits")]
    NoSize,
    #[error("the workflow sets its {parameter} to {value}, outside its own bounds")]
    OutOfBounds { parameter: Parameter, value: u64 },
}

/// A part of an image request (`ImageRequest`) that a workflow may map to an input of one of its
/// nodes: the same for every workflow, which may leave one unused - all but the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Parameter {
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
pub(crate) struct Input {
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

/// One workflow: its graph as exported, what it says of itself, and where the parts of a request
/// go.
#[derive(Debug)]
pub(crate) struct Workflow {
    description: String,
    graph: Map<String, Value>,
    inputs: BTreeMap<Parameter, Input>,
    size: SizeBounds,
    output: Option<String>,
}

impl Workflow {
    /// The workflow of the two files at `path`: `<path>.json` and `<path>.yaml`.
    ///
    /// # Errors
    /// `FileUnreadable`; `Invalid` for one that does not parse as what it is to be, or a graph
    /// that is not an object of nodes; else as a workflow is checked.
    pub(crate) fn read(path: &Path) -> Result<Self, WorkflowError> {
        let file = |kind: &str| {
            let mut file = OsString::from(path.as_os_str());
            file.push(format!(".{kind}"));
            let file = PathBuf::from(file);
            // Named by its file alone: a path beyond it may be shown to nobody.
            let shown = file.file_name().map(|name| name.to_string_lossy().into_owned());
            (file, shown.unwrap_or_default())
        };
        let text = |kind: &str| {
            let (file, shown) = file(kind);
            fs::read_to_string(&file).map_err(|e| WorkflowError::FileUnreadable {
                file: shown,
                cause: e.kind().to_string(),
            })
        };
        let invalid =
            |kind: &str, cause: String| WorkflowError::Invalid { file: file(kind).1, cause };
        let graph: Value =
            serde_json::from_str(&text("json")?).map_err(|e| invalid("json", e.to_string()))?;
        let Value::Object(graph) = graph else {
            return Err(invalid("json", "no object of nodes".to_string()));
        };
        let described: Described =
            parse_yaml(&text("yaml")?).map_err(|e| invalid("yaml", e.to_string()))?;
        Self::checked(graph, described)
    }

    /// The workflow of the graph `graph` and what `described` says of it, checked: the prompt
    /// mapped; every input mapped there, of a value its parameter fits; its own width and height
    /// within its bounds, which some size fits; the node its images come from there.
    ///
    /// # Errors
    /// `NoPrompt`, `NoSuchInput`, `WrongType`, `NoSize`, `OutOfBounds`, `NoSuchNode`.
    fn checked(graph: Map<String, Value>, described: Described) -> Result<Self, WorkflowError> {
        if !described.parameters.contains_key(&Parameter::Prompt) {
            return Err(WorkflowError::NoPrompt);
        }
        if !described.size.admits_any() {
            return Err(WorkflowError::NoSize);
        }
        for (parameter, input) in &described.parameters {
            let Some(value) = graph
                .get(&input.node)
                .and_then(|node| node.get("inputs"))
                .and_then(|inputs| inputs.get(&input.input))
            else {
                return Err(WorkflowError::NoSuchInput {
                    parameter: *parameter,
                    input: input.clone(),
                });
            };
            if !parameter.fits(value) {
                return Err(WorkflowError::WrongType {
                    parameter: *parameter,
                    input: input.clone(),
                    wanted: parameter.wanted(),
                });
            }
            if let (Parameter::Width | Parameter::Height, Some(side)) = (parameter, value.as_u64())
            {
                if !described.size.takes(side) {
                    return Err(WorkflowError::OutOfBounds { parameter: *parameter, value: side });
                }
            }
        }
        if let Some(node) = described.output.as_ref().filter(|node| !graph.contains_key(*node)) {
            return Err(WorkflowError::NoSuchNode(node.clone()));
        }
        Ok(Self {
            description: described.description,
            graph,
            inputs: described.parameters,
            size: described.size,
            output: described.output,
        })
    }

    /// Its graph with `request` written into the inputs it maps, those it does not map left out:
    /// what `POST /prompt` is given.
    ///
    /// # Errors
    /// Why it does not take `request` - a width or height outside its bounds, which it names -,
    /// for its model to report as a request it does not take.
    pub(crate) fn graph_with(&self, request: &ImageRequest) -> Result<Map<String, Value>, String> {
        let SizeBounds { min, max, multiple_of } = self.size;
        for (side, value) in [("width", request.width), ("height", request.height)] {
            if let Some(value) = value.filter(|value| !self.size.takes(*value)) {
                return Err(format!(
                    "{side} {value}: it takes {min} to {max}, a multiple of {multiple_of}"
                ));
            }
        }
        let mut graph = self.graph.clone();
        for (parameter, input) in &self.inputs {
            let value = match parameter {
                Parameter::Prompt => Some(Value::from(request.prompt.clone())),
                Parameter::NegativePrompt => request.negative_prompt.clone().map(Value::from),
                Parameter::Width => request.width.map(Value::from),
                Parameter::Height => request.height.map(Value::from),
                Parameter::Seed => Some(Value::from(request.seed)),
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

    /// What it is for, as its model describes itself.
    pub(crate) fn description(&self) -> &str {
        &self.description
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

    /// A directory with the workflow `z_image` of `graph` and `described`.
    fn dir_with(graph: &Value, described: &str) -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("z_image.json"), graph.to_string()).unwrap();
        fs::write(dir.path().join("z_image.yaml"), described).unwrap();
        dir
    }

    /// The workflow of `graph` and `described`.
    fn read(graph: &Value, described: &str) -> Result<Workflow, WorkflowError> {
        let dir = dir_with(graph, described);
        Workflow::read(&dir.path().join("z_image"))
    }

    fn request(width: Option<u64>) -> ImageRequest {
        ImageRequest {
            prompt: "a lighthouse".to_string(),
            negative_prompt: Some("fog".to_string()),
            width,
            height: None,
            seed: 7,
        }
    }

    #[test]
    fn a_workflow_is_its_graph_and_what_it_says_of_itself() {
        let workflow = read(&graph(), DESCRIBED).expect("a workflow");
        assert_eq!(workflow.description(), "fast");
        assert_eq!(workflow.output(), Some("9"));
    }

    /// Issue 525: what a request asks is written into the inputs the workflow maps, and what it
    /// does not map is left out; a size outside its bounds is not taken.
    #[test]
    fn a_request_is_written_into_the_inputs_the_workflow_maps() {
        let workflow = read(&graph(), DESCRIBED).unwrap();
        let graph = workflow.graph_with(&request(Some(768))).unwrap();
        assert_eq!(graph["6"]["inputs"]["text"], "a lighthouse");
        assert_eq!(graph["3"]["inputs"]["seed"], 7);
        assert_eq!(graph["13"]["inputs"]["width"], 768);
        assert_eq!(graph["13"]["inputs"]["height"], 1024, "its own");
        assert!(!Value::Object(graph).to_string().contains("fog"), "a part it does not map");
        let wide = workflow.graph_with(&request(Some(4096))).unwrap_err();
        assert_eq!(wide, "width 4096: it takes 256 to 2048, a multiple of 16");
    }

    #[test]
    fn what_a_workflow_maps_is_there_and_of_its_type() {
        let refused = |described: &str| read(&graph(), described).unwrap_err();
        assert!(matches!(
            refused("description: x\nparameters:\n  seed: 3.seed\n"),
            WorkflowError::NoPrompt
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
            WorkflowError::NoSuchNode(_)
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
        assert!(matches!(bounded("{ min: 300, max: 302 }").unwrap_err(), WorkflowError::NoSize));
        assert!(matches!(bounded("{ multiple_of: 0 }").unwrap_err(), WorkflowError::NoSize));
        let far = "{ min: 1, max: 18446744073709551614, multiple_of: 18446744073709551615 }";
        assert!(
            matches!(bounded(far).unwrap_err(), WorkflowError::NoSize),
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
    fn a_workflow_is_two_files_beside_each_other() {
        let dir = dir_with(&graph(), DESCRIBED);
        fs::remove_file(dir.path().join("z_image.yaml")).unwrap();
        let missing = Workflow::read(&dir.path().join("z_image")).unwrap_err();
        assert!(
            matches!(&missing, WorkflowError::FileUnreadable { file, .. }
                if file == "z_image.yaml"),
            "{missing}"
        );
        let shown = missing.to_string();
        assert!(!shown.contains(&*dir.path().to_string_lossy()), "no path: {shown}");
    }
}
