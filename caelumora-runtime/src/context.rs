//! The facets that build what an agent's model is given: a
//! `ContextProvider` assembles it, `PromptProvider`s contribute prompt parts
//! (issue 47). Their implementations are components of their own, e.g.
//! `turn_context_provider` and `static_system_prompt_provider`. An agent
//! has exactly one `ContextProvider` and any number of `PromptProvider`s
//! (issue 98).

use crate::work::AgentWork;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::context::{KnowledgeRef, PromptPart};
use caelumora_model::confidentiality::Classification;
use serde::Deserialize;

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

/// The text of a prompt part that lists entries: `preamble`, then each
/// section that has entries. Empty if none has: a part with nothing to list
/// is no part.
pub fn render_sections(preamble: &str, sections: &[Section]) -> String {
    let sections: Vec<&Section> = sections.iter().filter(|s| !s.lines.is_empty()).collect();
    if sections.is_empty() {
        return String::new();
    }
    let mut text = format!("{preamble}\n");
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
    text
}

/// Contributes a prompt part: the system prompt, or other text the model is
/// given besides the conversation. `TurnContextProvider` composes the parts
/// of every one of the agent's providers, by position, in the order of its
/// configuration.
#[async_trait]
pub trait PromptProvider: Send + Sync + std::fmt::Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    /// The part for `work` - a turn about to start, or a request to an
    /// external agent about to be sent (issue 204); empty text for none.
    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt>;
}

/// Assembles what the model of an agent is given besides the conversation,
/// once, at the start of its work (issue 214): the prompt parts, where they
/// go, and the knowledge from other places they hold. It only answers; the
/// caller sets what it answered - a turn with `Turn::set_context` and
/// `Turn::record_knowledge`. An agent has exactly one
/// (`ComponentRegistry::single`, issue 98): the component of its
/// personality that provides it, or the default - `turn_context_provider`,
/// unless the configuration names another (`AppConfig::component_defaults`).
/// Two would each answer for the whole, so a personality with two is
/// refused.
#[async_trait]
pub trait ContextProvider: Send + Sync + std::fmt::Debug {
    /// The prompt parts for `work`, each where it goes, and the knowledge
    /// they hold.
    ///
    /// # Errors
    /// If what the model is to be given cannot be had - a prompt provider
    /// that fails closed.
    async fn current_context(&self, work: &AgentWork<'_>) -> Result<(Vec<PromptPart>, Vec<KnowledgeRef>)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_rendered_under_the_preamble_and_empty_ones_left_out() {
        let text = render_sections("[Notes]", &[
            Section::new("Yours:", vec!["#1: tea".to_string()]),
            Section::new("Of Sassy:", Vec::new()),
            Section::lines(vec!["coffee".to_string()]),
        ]);
        assert_eq!(text, "[Notes]\nYours:\n- #1: tea\n- coffee\n");
        assert_eq!(render_sections("[Notes]", &[Section::new("Yours:", Vec::new())]), "", "nothing to list is no part");
    }
}
