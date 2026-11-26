//! Tools as a model is offered them and their calls as they are recorded: a `ToolDescription` with
//! its parameters, who sees its calls and how they are shown, the `ToolSet` of a provider, a
//! `ToolCall` and how it ended (`ToolCallResult`) - with what its tool gave its model
//! (`ToolContent`) -, and where a call stands while its turn runs (`ToolCallState`).
//!
//! A call as it is recorded, and where it stands, is `call`'s; here a tool as it is offered.

mod call;

pub use call::{
    ToolCall, ToolCallResult, ToolCallState, ToolCallStatus, ToolContent, ToolResultStatus,
};

use crate::confidentiality::{Classification, Visibility};
use crate::intern::intern;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::ptr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolParameters {
    Object {
        properties: HashMap<String, ToolParameters>,
        // Left out when empty, so it must be read as empty when missing.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        required: Vec<String>,
    },
    String {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The language the value is written in, if it is one (issue 247): what a rule derived
        /// from a call compares it by. The model is not told of it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<ArgumentLanguage>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Array {
        items: Box<ToolParameters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Boolean {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

/// How many references a reading of a JSON Schema follows in all
/// (`ToolParameters::from_json_schema`): definitions that refer to each other are read again on
/// every way to them, which a document of many could make without end.
const MAX_REFERENCES: usize = 1000;

/// One reading of a JSON Schema into `ToolParameters` (`ToolParameters::from_json_schema`): the
/// document its references point into, the schemas on the way to what is read - so that one of
/// them again is cut, as a reference back -, and how many references it may follow yet.
struct SchemaReading<'a> {
    document: &'a Value,
    following: Vec<&'a Value>,
    left: usize,
}

impl<'a> SchemaReading<'a> {
    /// The parameters `schema` describes, `written` - a description written where it is referred
    /// to - in the place of its own.
    fn of(&mut self, schema: &'a Value, written: Option<&str>) -> ToolParameters {
        let Some(obj) = schema.as_object() else {
            return ToolParameters::unrecognized(written.map(str::to_string));
        };
        if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
            let written = written.or_else(|| obj.get("description").and_then(Value::as_str));
            let target = reference.strip_prefix('#').and_then(|p| self.document.pointer(p));
            let back = |target: &&Value| self.following.iter().any(|on| ptr::eq(*on, *target));
            let Some(target) = target.filter(|target| !back(target) && self.left > 0) else {
                return ToolParameters::unrecognized(ToolParameters::description_of(obj, written));
            };
            self.left -= 1;
            self.following.push(target);
            let read = self.of(target, written);
            self.following.pop();
            return read;
        }
        let description = ToolParameters::description_of(obj, written);
        match ToolParameters::type_of(obj) {
            Some("object") => {
                let properties = obj
                    .get("properties")
                    .and_then(Value::as_object)
                    .map(|props| props.iter().map(|(k, v)| (k.clone(), self.of(v, None))).collect())
                    .unwrap_or_default();
                let required = obj
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                ToolParameters::Object { properties, required }
            }
            Some("array") => {
                let items = obj
                    .get("items")
                    .filter(|items| items.is_object())
                    .map(|items| self.of(items, None))
                    .unwrap_or_else(|| ToolParameters::string().described("Array item"));
                ToolParameters::Array { items: Box::new(items), description }
            }
            Some("string") => ToolParameters::String { description, language: None },
            Some("number") | Some("integer") => ToolParameters::Number { description },
            Some("boolean") => ToolParameters::Boolean { description },
            _ => ToolParameters::unrecognized(description),
        }
    }
}

/// A language an argument of a tool is written in (issue 247), whose meaning a pattern cannot
/// tell: a rule compares it as the language reads it (`ArgumentMatcher`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentLanguage {
    /// A bash command line.
    Shell,
}

/// The parameters of a tool: built as a schema - which there are, of which
/// type, which are required -, and described apart from it (issue 57):
/// one by one (`described`), or all of them by their paths, from where the
/// texts of a tool are kept (`describe`); or read, descriptions included,
/// of a JSON Schema a remote side gives (`from_json_schema`).
impl ToolParameters {
    pub fn string() -> Self {
        Self::String { description: None, language: None }
    }

    /// A string that is a bash command line.
    pub fn shell() -> Self {
        Self::String { description: None, language: Some(ArgumentLanguage::Shell) }
    }

    pub fn number() -> Self {
        Self::Number { description: None }
    }

    pub fn boolean() -> Self {
        Self::Boolean { description: None }
    }

    pub fn array(items: ToolParameters) -> Self {
        Self::Array { items: Box::new(items), description: None }
    }

    /// No parameters: an object without properties, as a tool that takes nothing declares.
    pub fn none() -> Self {
        Self::Object { properties: HashMap::new(), required: Vec::new() }
    }

    pub fn object(
        properties: impl IntoIterator<Item = (impl Into<String>, ToolParameters)>,
        required: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::Object {
            properties: properties.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            required: required.into_iter().map(|s| s.into()).collect(),
        }
    }

    /// The parameters `schema` describes - a JSON Schema, as MCP and OpenAPI write a tool's input
    /// (issue 252) -, its references (`$ref`) resolved in `document`: the JSON Schema of an MCP
    /// tool's input itself, the whole document of an OpenAPI service. As well as they can be: an
    /// object by its properties and what of them is required, a string, a number or an integer, an
    /// array and a boolean by their `type` - one of a list beside `"null"` -, each but the object
    /// with its `description`, followed by the values of its `enum` and its `pattern`, so that the
    /// model learns what a value may be; a description written beside a reference takes the place
    /// of the one where it points, not of those. A schema that is no object, has no `type` among
    /// these (`oneOf`, several types), or refers to what `document` does not hold - a fragment is
    /// read as written, not percent-decoded -, back to where it began, or past
    /// `MAX_REFERENCES` followed in all, becomes a string with its own description, or else one
    /// described as unrecognized, since `ToolParameters` models no more of JSON Schema; the
    /// entries of an array whose `items` is no schema, strings described as such.
    pub fn from_json_schema(schema: &Value, document: &Value) -> Self {
        let mut reading = SchemaReading { document, following: vec![schema], left: MAX_REFERENCES };
        reading.of(schema, None)
    }

    /// The one type `obj` names: its `type`, or the one of a list of them beside `"null"`, which
    /// says that a value may be missing; `None` for none, or several.
    fn type_of(obj: &Map<String, Value>) -> Option<&str> {
        match obj.get("type")? {
            Value::String(name) => Some(name),
            Value::Array(names) => {
                let mut named = names.iter().filter_map(Value::as_str).filter(|n| *n != "null");
                let first = named.next();
                named.next().map_or(first, |_| None)
            }
            _ => None,
        }
    }

    /// What `obj` says of its value: `written` - a description written where it is referred to -
    /// or else its own `description`, ended as a sentence, then the values of its `enum` and its
    /// `pattern`; `None` where there is none of them.
    fn description_of(obj: &Map<String, Value>, written: Option<&str>) -> Option<String> {
        let text = written.or_else(|| obj.get("description").and_then(Value::as_str)).map(|text| {
            let text = text.trim_end();
            match text.ends_with(['.', '!', '?', ':']) {
                true => text.to_string(),
                false => format!("{text}."),
            }
        });
        let values = obj.get("enum").and_then(Value::as_array).map(|values| {
            let values: Vec<String> = values
                .iter()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                .collect();
            format!("One of: {}.", values.join(", "))
        });
        let pattern = obj.get("pattern").and_then(Value::as_str).map(|p| format!("Matching: {p}"));
        let parts: Vec<String> = [text, values, pattern].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    /// A string, as what is not recognized is offered: with `description`, or described as
    /// unrecognized.
    fn unrecognized(description: Option<String>) -> Self {
        let description =
            description.unwrap_or_else(|| "Unrecognized parameter schema".to_string());
        Self::String { description: Some(description), language: None }
    }

    /// The parameter, described by `text`. An object has no description of
    /// its own - what it holds is described - and stays as it is.
    pub fn described(mut self, text: impl Into<String>) -> Self {
        if let Some(description) = self.description_mut() {
            *description = Some(text.into());
        }
        self
    }

    /// Describes every parameter below `path` that is not described yet by
    /// what `text` answers for its path: `<path>.<name>` for a property of an object, the path of a
    /// list for each of its entries' properties if they are objects, and
    /// `<path>.item` for an entry otherwise -
    /// `ask_user.questions.choices.label`, `ask_user.ask.item`. One
    /// described already (`described`) keeps its description: a text that
    /// several tools share.
    ///
    /// # Errors
    /// The first error `text` answers.
    pub fn describe<E>(
        &mut self,
        path: &str,
        text: &mut impl FnMut(&str) -> Result<String, E>,
    ) -> Result<(), E> {
        match self {
            ToolParameters::Object { properties, .. } => {
                for (name, property) in properties {
                    let path = format!("{path}.{name}");
                    property.describe_at(&path, text)?;
                }
                Ok(())
            }
            ToolParameters::Array { items, .. } => match items.as_mut() {
                ToolParameters::Object { .. } => items.describe(path, text),
                item => item.describe_at(&format!("{path}.item"), text),
            },
            _ => Ok(()),
        }
    }

    /// Describes this parameter by the text of `path` unless it is
    /// described, and what it holds below it.
    fn describe_at<E>(
        &mut self,
        path: &str,
        text: &mut impl FnMut(&str) -> Result<String, E>,
    ) -> Result<(), E> {
        if let Some(description @ None) = self.description_mut() {
            *description = Some(text(path)?);
        }
        self.describe(path, text)
    }

    /// Its description, `None` for an object, which has none.
    fn description_mut(&mut self) -> Option<&mut Option<String>> {
        match self {
            ToolParameters::Object { .. } => None,
            ToolParameters::String { description, .. }
            | ToolParameters::Number { description }
            | ToolParameters::Array { description, .. }
            | ToolParameters::Boolean { description } => Some(description),
        }
    }

    /// The language of the property `name` of an object, if it is a string written in one.
    pub fn language_of(&self, name: &str) -> Option<ArgumentLanguage> {
        match self {
            ToolParameters::Object { properties, .. } => match properties.get(name)? {
                ToolParameters::String { language, .. } => *language,
                _ => None,
            },
            _ => None,
        }
    }
}

/// Who sees a tool's calls (issues 54, 133, 281): the call itself - that
/// the tool was called -, its arguments, and its result. Neither the
/// arguments nor the result are seen by more than the call. The agent keeps
/// every call in full; what it publishes to its channel is redacted by
/// these flags (`Message::published`). How sensitive a result is, is
/// another question: its classification (`ToolCallResult::classification`,
/// issue 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolVisibility {
    pub call: Visibility,
    pub arguments: Visibility,
    pub result: Visibility,
}

impl ToolVisibility {
    /// Call, arguments and result are published to the channel.
    pub const SHOWN: ToolVisibility = ToolVisibility {
        call: Visibility::Channel,
        arguments: Visibility::Channel,
        result: Visibility::Channel,
    };
    /// Others may see that the tool was called and with what, but not what
    /// it returned.
    pub const WITHOUT_RESULT: ToolVisibility = ToolVisibility {
        call: Visibility::Channel,
        arguments: Visibility::Channel,
        result: Visibility::Owner,
    };
    /// Others may see that the tool was called, but not with what or what
    /// it returned.
    pub const CALL_ONLY: ToolVisibility = ToolVisibility {
        call: Visibility::Channel,
        arguments: Visibility::Owner,
        result: Visibility::Owner,
    };
    /// Nothing of the call leaves the agent.
    pub const HIDDEN: ToolVisibility = ToolVisibility {
        call: Visibility::Owner,
        arguments: Visibility::Owner,
        result: Visibility::Owner,
    };

    pub fn is_shown(&self) -> bool {
        *self == Self::SHOWN
    }
}

impl Default for ToolVisibility {
    fn default() -> Self {
        Self::SHOWN
    }
}

/// How a frontend shows a tool's calls and their results (issue 165), so
/// that it need not recognise a tool by its name, which components and
/// configuration choose. Display only: what a call does, and whether it
/// waits for anyone, does not depend on it.
///
/// Open (issue 323): a name, defined as a constant next to the tools that
/// are shown so (`SKILL_PRESENTATION`, …; the shell's with how a command is
/// called and answered, `caelumora_api::terminal::SHELL_PRESENTATION`), with
/// what it shows of their arguments and results; those of any tool, `DEFAULT`
/// and `QUIET`, are the type's own. A frontend shows those it knows and
/// every other as `DEFAULT`. Written as its name, and read back as the
/// presentation of that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolPresentation(&'static str);

impl ToolPresentation {
    /// As every call: its name, its arguments and its result.
    pub const DEFAULT: Self = Self::new("default");

    /// Not at all while it runs and where it succeeded, as `DEFAULT` where it did not: what it did
    /// shows elsewhere - an attachment in the answer -, and the call would only say so again. A
    /// call so shown cannot be interrupted where it is not shown; for a call that ends at once.
    pub const QUIET: Self = Self::new("quiet");

    /// The presentation of `name`, written in `snake_case`.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// Its name, as a frontend knows it.
    pub fn name(self) -> &'static str {
        self.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

impl Default for ToolPresentation {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Serialize for ToolPresentation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ToolPresentation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|name| Self(intern(&name)))
    }
}

/// A tool of an agent: what its model is told of it - its name, what it does, its parameters - and
/// what only the kernel acts on, whether its calls need approval, who sees them, how its results
/// are classified. A backend sends the model the first three alone; the record of what a model was
/// offered keeps all of it, as it was then (issues 137, 194).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescription {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    /// Left out in a record made before issue 194, which kept only what the
    /// model was told: not known, read as no.
    #[serde(default)]
    pub requires_approval: bool,
    /// Who sees its calls. Approvers see the arguments of a call that needs
    /// approval regardless: they cannot decide otherwise.
    #[serde(default, skip_serializing_if = "ToolVisibility::is_shown")]
    pub visibility: ToolVisibility,
    /// How its results are classified where they do not classify
    /// themselves (issue 133) - configured with the tool, e.g. for an MCP
    /// server. `None`: as the channel the call is made in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    /// How frontends show its calls.
    #[serde(default, skip_serializing_if = "ToolPresentation::is_default")]
    pub presentation: ToolPresentation,
}

/// The tools of one source of an agent - a component, e.g. an MCP server -
/// with what they are for (issue 194).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSet {
    /// The id of the component the tools come from, unique within the
    /// agent.
    pub name: String,
    /// What the tools are for, written for a model.
    pub description: String,
    pub tools: Vec<ToolDescription>,
    /// Why the source gives no tools now, in general terms - never the
    /// error, which may name hosts (issue 32). `None`: available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Where the arguments of its calls go (issue 180): the classification
    /// its remote side is cleared for. `None`: they stay in the system, or
    /// are bounded otherwise - a workspace's storage by its own ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<Classification>,
}

impl ToolSet {
    /// The available set `name` of `tools`, for `description`.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        tools: Vec<ToolDescription>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            tools,
            unavailable: None,
            destination: None,
        }
    }

    /// It as it is offered in a channel its remote side is not cleared for
    /// (issue 180): unavailable, without its tools.
    pub fn withheld(self) -> Self {
        Self {
            tools: Vec::new(),
            unavailable: Some("its remote side is not cleared for this channel".to_string()),
            ..self
        }
    }
}

/// Tools and their calls for the tests of this crate and of others.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::{ToolCall, ToolDescription, ToolParameters};
    use serde_json::Value;

    /// A call of the tool `function_name`, without arguments, shown as the tool's are.
    pub fn make_tool_call(function_name: &str) -> ToolCall {
        ToolCall::new(Some("call_1".to_string()), function_name, Value::Null)
    }

    /// The tool `name`, which does what its name says and takes no parameters: asking nobody,
    /// shown, classified as its channel and presented as by default.
    pub fn make_tool_description(name: &str) -> ToolDescription {
        ToolDescription {
            name: name.to_string(),
            description: format!("does {name}"),
            parameters: ToolParameters::none(),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_parameter_is_described_by_its_path() {
        let choice = ToolParameters::object([("label", ToolParameters::string())], ["label"]);
        let question = ToolParameters::object(
            [("header", ToolParameters::string()), ("choices", ToolParameters::array(choice))],
            ["header"],
        );
        let mut parameters = ToolParameters::object(
            [
                ("questions", ToolParameters::array(question)),
                ("ask", ToolParameters::array(ToolParameters::string())),
                ("all", ToolParameters::boolean()),
                ("level", ToolParameters::string().described("shared")),
            ],
            ["questions"],
        );
        let mut asked = Vec::new();
        parameters
            .describe("ask_user", &mut |path: &str| -> Result<String, ()> {
                asked.push(path.to_string());
                Ok(format!("<{path}>"))
            })
            .unwrap();
        asked.sort();
        assert_eq!(asked, [
            "ask_user.all", "ask_user.ask", "ask_user.ask.item",
            "ask_user.questions", "ask_user.questions.choices", "ask_user.questions.choices.label", "ask_user.questions.header",
        ], "a list of objects is described by its own path and its entries' properties; of other entries by `.item`");
        let described = serde_json::to_value(&parameters).unwrap();
        assert_eq!(
            described["properties"]["questions"]["items"]["properties"]["choices"]["items"]
                ["properties"]["label"]["description"],
            "<ask_user.questions.choices.label>"
        );
        assert_eq!(described["properties"]["ask"]["items"]["description"], "<ask_user.ask.item>");
        assert_eq!(
            described["properties"]["level"]["description"], "shared",
            "one described already keeps its description"
        );

        let failed = ToolParameters::object([("a", ToolParameters::string())], ["a"])
            .describe("t", &mut |path: &str| Err(path.to_string()));
        assert_eq!(failed, Err("t.a".to_string()), "the first text that cannot be had");
    }

    #[test]
    fn a_parameter_is_described_one_by_one_and_an_object_has_no_description() {
        assert_eq!(
            ToolParameters::string().described("a channel"),
            ToolParameters::String { description: Some("a channel".to_string()), language: None }
        );
        let object = ToolParameters::object([("a", ToolParameters::number())], ["a"]);
        assert_eq!(object.clone().described("ignored"), object);
    }

    #[test]
    fn parameters_without_required_ones_are_read_back_as_written() {
        let parameters = ToolParameters::Object {
            properties: HashMap::from([(
                "channel".to_string(),
                ToolParameters::string().described("a channel"),
            )]),
            required: Vec::new(),
        };
        let json = serde_json::to_string(&parameters).unwrap();
        assert!(!json.contains("required"), "an empty list is left out");
        assert_eq!(serde_json::from_str::<ToolParameters>(&json).unwrap(), parameters);
    }

    #[test]
    fn boolean_and_array_round_trip_through_json() {
        let params = ToolParameters::object(
            vec![
                ("enabled", ToolParameters::boolean().described("Whether the feature is enabled")),
                (
                    "tags",
                    ToolParameters::array(ToolParameters::string().described("A tag"))
                        .described("List of tags"),
                ),
            ],
            vec!["enabled"],
        );

        let json = serde_json::to_value(&params).unwrap();
        let round_tripped: ToolParameters = serde_json::from_value(json).unwrap();
        assert_eq!(params, round_tripped);

        match round_tripped {
            ToolParameters::Object { properties, .. } => {
                assert!(matches!(properties.get("enabled"), Some(ToolParameters::Boolean { .. })));
                assert!(matches!(properties.get("tags"), Some(ToolParameters::Array { .. })));
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn schema_conversion_recognizes_basic_types() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "number" },
                "verbose": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["query"]
        });

        let params = ToolParameters::from_json_schema(&schema, &schema);
        match params {
            ToolParameters::Object { properties, required } => {
                assert_eq!(required, vec!["query".to_string()]);
                assert!(matches!(properties.get("query"), Some(ToolParameters::String { .. })));
                assert!(matches!(properties.get("limit"), Some(ToolParameters::Number { .. })));
                assert!(matches!(properties.get("verbose"), Some(ToolParameters::Boolean { .. })));
                assert!(matches!(properties.get("tags"), Some(ToolParameters::Array { .. })));
            }
            _ => panic!("Expected Object"),
        }
    }

    #[test]
    fn schema_conversion_falls_back_to_string_for_unrecognized_schema() {
        let schema = json!({ "oneOf": [{ "type": "string" }, { "type": "number" }] });
        let params = ToolParameters::from_json_schema(&schema, &schema);
        assert!(matches!(params, ToolParameters::String { .. }));
    }

    /// Issue 252: a reference is read where it points - into the schema itself, as MCP writes
    /// it, or into the whole document, as OpenAPI does -, a description written beside it in the
    /// place of the one there but not of its allowed values; one that points nowhere, to what is no
    /// schema, or back to where it began, is cut.
    #[test]
    fn references_are_read_where_they_point_and_cut_where_they_lead_back() {
        let schema = json!({
            "type": "object",
            "properties": {
                "owner": { "$ref": "#/$defs/user" },
                "mode": { "$ref": "#/$defs/mode", "description": "How it runs" },
                "slashed": { "$ref": "#/$defs/a~1b" },
                "lost": { "$ref": "#/$defs/missing" },
                "far": { "$ref": "https://example.com/schema" },
                "plain": { "$ref": "#/$defs/plain" },
                "itself": { "$ref": "#" }
            },
            "$defs": {
                "user": { "type": "string", "description": "Who owns it" },
                "mode": { "type": "string", "enum": ["fast", "safe"], "description": "The mode" },
                "a/b": { "type": "boolean" },
                "plain": "no schema"
            }
        });
        let ToolParameters::Object { properties, .. } =
            ToolParameters::from_json_schema(&schema, &schema)
        else {
            panic!("an object");
        };
        let unrecognized = ToolParameters::string().described("Unrecognized parameter schema");
        assert_eq!(properties["owner"], ToolParameters::string().described("Who owns it."));
        assert_eq!(
            properties["mode"],
            ToolParameters::string().described("How it runs. One of: fast, safe."),
            "the description beside it, the values where it points"
        );
        assert_eq!(properties["slashed"], ToolParameters::boolean(), "a pointer's escapes read");
        assert_eq!(properties["lost"], unrecognized);
        assert_eq!(properties["far"], unrecognized, "only into the document");
        assert_eq!(properties["plain"], unrecognized, "no schema");
        assert_eq!(properties["itself"], unrecognized, "back to the schema itself");

        let document = json!({ "components": { "schemas": {
            "Timesheet": { "type": "object", "properties": {
                "project": { "type": "integer", "description": "Project ID" },
                "next": { "$ref": "#/components/schemas/Link" }
            } },
            "Link": { "type": "object", "properties": {
                "to": { "$ref": "#/components/schemas/Timesheet" }
            } }
        } } });
        let body = &document["components"]["schemas"]["Timesheet"];
        let link = ToolParameters::object([("to", unrecognized.clone())], Vec::<String>::new());
        let expected = ToolParameters::object(
            [("project", ToolParameters::number().described("Project ID.")), ("next", link)],
            Vec::<String>::new(),
        );
        assert_eq!(
            ToolParameters::from_json_schema(body, &document),
            expected,
            "of the document, cut where two definitions lead back to each other"
        );
    }

    /// Issue 252: definitions that refer to each other are read again on every way to them, which
    /// is bounded: a reading follows at most `MAX_REFERENCES` references.
    #[test]
    fn a_reading_follows_a_bounded_number_of_references() {
        // Each definition refers twice to the next: 2^30 ways to the last.
        let defs: Map<String, Value> = (0..30)
            .map(|n| {
                let next = json!({ "$ref": format!("#/$defs/d{}", n + 1) });
                let schema = json!({ "type": "object", "properties": { "a": next, "b": next } });
                (format!("d{n}"), schema)
            })
            .collect();
        let schema = json!({ "$ref": "#/$defs/d0", "$defs": defs });
        let read = ToolParameters::from_json_schema(&schema, &schema);
        assert!(matches!(read, ToolParameters::Object { .. }), "read as far as it may follow");
    }

    /// Issue 252: a type beside `"null"` is the type, allowed values are told the model, and an
    /// array whose `items` is no schema holds strings - as Kimai writes its lists of ids.
    #[test]
    fn a_type_of_a_list_allowed_values_and_items_that_are_no_schema_are_read_as_well_as_they_can() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"], "description": "Its name" },
                "either": { "type": ["string", "integer"] },
                "order": { "type": "string", "enum": ["ASC", "DESC"], "description": "The order." },
                "user": { "type": "string", "pattern": "\\d+|all" },
                "users[]": { "type": "array", "items": [] }
            }
        });
        let ToolParameters::Object { properties, .. } =
            ToolParameters::from_json_schema(&schema, &schema)
        else {
            panic!("an object");
        };
        assert_eq!(properties["name"], ToolParameters::string().described("Its name."));
        let unrecognized = ToolParameters::string().described("Unrecognized parameter schema");
        assert_eq!(properties["either"], unrecognized, "several types");
        let order = ToolParameters::string().described("The order. One of: ASC, DESC.");
        assert_eq!(properties["order"], order);
        assert_eq!(properties["user"], ToolParameters::string().described("Matching: \\d+|all"));
        let items = ToolParameters::string().described("Array item");
        assert_eq!(properties["users[]"], ToolParameters::array(items));
    }
}
