//! Tools as a model is offered them and their calls as they are recorded: a `Tool` with its
//! parameters, who sees its calls and how they are shown, the `ToolSet` of a provider, a `ToolCall`
//! and how it ended (`ToolCallResult`) - with what its tool gave its model (`ToolMedia`) -, and
//! where a call stands while its turn runs (`ToolCallState`).
//!
//! A call as it is recorded, and where it stands, is `call`'s; here a tool as it is offered.

mod call;

pub use call::{
    ToolCall, ToolCallResult, ToolCallState, ToolCallStatus, ToolMedia, ToolResultStatus,
};

use crate::confidentiality::{Classification, Visibility};
use crate::intern::intern;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;

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
/// texts of a tool are kept (`describe`).
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

    pub fn object(
        properties: impl IntoIterator<Item = (impl Into<String>, ToolParameters)>,
        required: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::Object {
            properties: properties.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            required: required.into_iter().map(|s| s.into()).collect(),
        }
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
/// are shown so (`SHELL_PRESENTATION`, `SKILL_PRESENTATION`, …), with what
/// it shows of their arguments and results; those of any tool, `DEFAULT`
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
pub struct Tool {
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
    pub tools: Vec<Tool>,
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
    pub fn new(name: impl Into<String>, description: impl Into<String>, tools: Vec<Tool>) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
