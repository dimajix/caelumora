//! What an agent's model is given (issue 47): the facets that build it - a
//! `ContextProvider` assembles it, `PromptProvider`s contribute prompt parts
//! -, what their components share, and the built-in components that provide
//! them alone, one file per component kind. An agent has exactly one
//! `ContextProvider` and any number of `PromptProvider`s (issue 98). A
//! component that offers a prompt part along with another facet - tools,
//! say - stays in `components`.

pub mod channel_briefing_prompt_provider;
pub mod channel_language_prompt_provider;
pub mod channel_participants_prompt_provider;
pub mod external_context_provider;
pub mod static_system_prompt_provider;
pub mod tool_sets_prompt_provider;
pub mod turn_context_provider;

use caelumora_runtime::component::{ComponentError, ComponentImpl, ComponentRegistry};
use crate::situation::Situation;
use caelumora_runtime::view::ViewError;
use async_trait::async_trait;
use caelumora_model::context::{KnowledgeRef, PromptPart};
use caelumora_model::confidentiality::Classification;
use caelumora_model::store::StoreError;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Debug;

/// Where a prompt part goes in the context (issue 47).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptPosition {
    /// Before everything, as the system prompt. Many models put it first
    /// and cache what follows it, so it should rarely change.
    #[default]
    Start,
    /// Transiently before the newest message of the turn, never stored in
    /// the context - for what changes often, such as notes. Given as a user
    /// message: some models merge every system message into the system
    /// prompt and move it to the front, where it would invalidate the cache.
    BeforeTurnInput,
}

impl PromptPosition {
    /// The parameter `position` of a prompt provider's configuration
    /// `params`, `Start` if it has none.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if it is neither `start` nor
    /// `before_turn_input`.
    pub fn of_params(params: &Value) -> Result<Self, ComponentError> {
        match params.get("position") {
            None => Ok(Self::default()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| ComponentError::InvalidConfig("parameter 'position' must be \"start\" or \"before_turn_input\"".to_string())),
        }
    }
}

/// What a [`PromptProvider`] contributes: the text of its part, the
/// knowledge from other places it contains, recorded with the turn's
/// messages (issue 17), and how what the text holds besides that knowledge
/// is classified (issue 168). The turn has seen both: what it writes is
/// classified at least so.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub text: String,
    pub knowledge: Vec<KnowledgeRef>,
    /// `None`: not known, counted as the channel - a part that does not
    /// say what it holds may hold anything the channel may.
    pub classification: Option<Classification>,
}

impl Prompt {
    /// `text`, which holds nothing but `knowledge` - each entry classified
    /// in its reference.
    pub fn of_knowledge(text: String, knowledge: Vec<KnowledgeRef>) -> Self {
        Self { text, knowledge, classification: Some(Classification::PUBLIC) }
    }

    /// `text`, holding no knowledge from elsewhere, classified
    /// `classification`.
    pub fn classified(text: String, classification: Classification) -> Self {
        Self { text, knowledge: Vec::new(), classification: Some(classification) }
    }
}

/// A text that does not say how it is classified: as the channel.
impl From<String> for Prompt {
    fn from(text: String) -> Self {
        Self { text, knowledge: Vec::new(), classification: None }
    }
}

/// One section of a prompt part that lists entries, e.g. the notes of one
/// agent: an optional heading, then one `- ` line per entry.
#[derive(Debug, Clone, Default)]
pub struct Section {
    pub heading: Option<String>,
    pub lines: Vec<String>,
}

impl Section {
    /// A section under `heading`.
    pub fn new(heading: impl Into<String>, lines: Vec<String>) -> Self {
        Self { heading: Some(heading.into()), lines }
    }

    /// A section without a heading.
    pub fn lines(lines: Vec<String>) -> Self {
        Self { heading: None, lines }
    }
}

/// A prompt part titled `title` (issue 249): the title in brackets on a
/// line of its own, then `text`, which ends in a line break. Empty if `text`
/// is: a part that says nothing is no part. Every part the runtime writes
/// is made so, so that the parts of a prompt stay apart.
pub fn titled(title: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let end = if text.ends_with('\n') { "" } else { "\n" };
    format!("[{title}]\n{text}{end}")
}

/// A prompt part that lists entries, titled `title`: `preamble` - what the
/// entries are, if anything needs saying - then each section that has
/// entries. Empty if none has: a part with nothing to list is no part.
pub fn render_sections(title: &str, preamble: &str, sections: &[Section]) -> String {
    let sections: Vec<&Section> = sections.iter().filter(|s| !s.lines.is_empty()).collect();
    if sections.is_empty() {
        return String::new();
    }
    let mut text = if preamble.is_empty() { String::new() } else { format!("{preamble}\n") };
    for section in sections {
        if let Some(heading) = &section.heading {
            text.push_str(heading);
            text.push('\n');
        }
        for line in &section.lines {
            text.push_str("- ");
            text.push_str(line);
            text.push('\n');
        }
    }
    titled(title, &text)
}

/// Contributes a prompt part: the system prompt, or other text the model is
/// given besides the conversation. `TurnContextProvider` composes the parts
/// of every one of the agent's providers, by position, in the order of its
/// configuration.
#[async_trait]
pub trait PromptProvider: Send + Sync + Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    /// The part for `situation` - a turn about to start, or a request to an
    /// external agent about to be sent (issue 204); empty text for none.
    ///
    /// # Errors
    /// `ContextError` if the part cannot be had.
    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError>;
}

/// The part of the prompt provider `provider` for `situation`, as its fail
/// policy says: `None` if it failed and fails open. A function of the
/// prompt module rather than a method of `ComponentImpl`, whose methods are
/// the component registry's alone (issue 370), as `apply_stage_processors`
/// is the message stages'.
///
/// # Errors
/// `ContextError::Component`, naming the component, if it failed and
/// fails closed.
pub(crate) async fn prompt_for(provider: &ComponentImpl<dyn PromptProvider>, situation: &Situation<'_>) -> Result<Option<Prompt>, ContextError> {
    provider.apply_fail_policy(provider.get_prompt(situation).await)
        .map_err(|cause| ContextError::Component { component: provider.id.clone(), cause: Box::new(cause) })
}

/// Assembles what the model of an agent is given besides the conversation,
/// once, at the start of its work (issue 214): the prompt parts, where they
/// go, and the knowledge from other places they hold. It only answers; the
/// caller takes in what it answered, both at once - a turn with
/// `Turn::set_context`, an external agent's work with
/// `Work::take_in_context` (issue 285). An agent has exactly one
/// (`ComponentRegistry::single`, issue 98): the component of its
/// personality that provides it, or the default - `turn_context_provider`,
/// unless the configuration names another (`AppConfig::component_defaults`).
/// Two would each answer for the whole, so a personality with two is
/// refused.
#[async_trait]
pub trait ContextProvider: Send + Sync + Debug {
    /// The prompt parts for `situation`, each where it goes, and the knowledge
    /// they hold.
    ///
    /// # Errors
    /// `ContextError` if what the model is to be given cannot be had - a
    /// prompt provider that fails closed.
    async fn current_context(&self, situation: &Situation<'_>) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>), ContextError>;
}

/// Why what an agent's model is to be given cannot be had (issue 336):
/// what a `ContextProvider` or a `PromptProvider` answers. A plugin's
/// provider translates its own errors into it, deciding there what may be
/// shown; `crate::error::user_message` judges each variant.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// A failure in words that may be shown to users.
    #[error("{0}")]
    Failed(String),
    /// What reading through the views reported, judged as view errors are.
    #[error("{0}")]
    View(#[from] ViewError),
    /// What the store reported, judged as store errors are.
    #[error("{0}")]
    Store(#[from] StoreError),
    /// A prompt provider that fails closed failed with `cause`.
    #[error("component '{component}' failed")]
    Component { component: String, #[source] cause: Box<ContextError> },
}

/// Registers the component kinds of this directory with `registry`, and the
/// `ContextProvider` as a facet of which an agent has exactly one, with its
/// default kind (issue 99).
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn ContextProvider>(Some(turn_context_provider::TURN_CONTEXT_PROVIDER_KIND));

    channel_briefing_prompt_provider::register(registry);
    channel_language_prompt_provider::register(registry);
    channel_participants_prompt_provider::register(registry);
    external_context_provider::register(registry);
    static_system_prompt_provider::register(registry);
    tool_sets_prompt_provider::register(registry);
    turn_context_provider::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_rendered_under_the_title_and_preamble_and_empty_ones_left_out() {
        let text = render_sections("Notes", "What was noted.", &[
            Section::new("Yours:", vec!["#1: tea".to_string()]),
            Section::new("Of Sassy:", Vec::new()),
            Section::lines(vec!["coffee".to_string()]),
        ]);
        assert_eq!(text, "[Notes]\nWhat was noted.\nYours:\n- #1: tea\n- coffee\n");
        assert_eq!(render_sections("Notes", "", &[Section::lines(vec!["tea".to_string()])]), "[Notes]\n- tea\n", "no preamble where none is needed");
        assert_eq!(render_sections("Notes", "What was noted.", &[Section::new("Yours:", Vec::new())]), "", "nothing to list is no part");
    }

    #[test]
    fn a_part_is_titled_and_one_that_says_nothing_is_none() {
        assert_eq!(titled("Planning", "Plan first."), "[Planning]\nPlan first.\n");
        assert_eq!(titled("Planning", "Plan first.\n"), "[Planning]\nPlan first.\n", "one line break at the end");
        assert_eq!(titled("Planning", ""), "");
    }
}
