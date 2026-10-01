//! The facets that build what an agent's model is given: a
//! `ContextProvider` assembles it, `PromptProvider`s contribute prompt parts
//! (issue 47). Their implementations are components of their own, e.g.
//! `state_context_provider` and `static_system_prompt_provider`. An agent
//! has exactly one `ContextProvider` and any number of `PromptProvider`s
//! (issue 98).

use crate::agent_turn::Turn;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::context::KnowledgeRef;
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
/// given besides the conversation. `StateContextProvider` composes the parts
/// of every one of the agent's providers, by position, in the order of its
/// configuration.
#[async_trait]
pub trait PromptProvider: Send + Sync + std::fmt::Debug {
    fn position(&self) -> PromptPosition {
        PromptPosition::Start
    }
    /// The part for `turn`, which is about to start; empty text for none.
    async fn get_prompt(&self, turn: &Turn) -> Result<Prompt>;
}

/// Assembles what the model is given in a turn (`Turn::set_context`), once,
/// at its start. An agent has exactly one (`ComponentRegistry::single`,
/// issue 98): the component of its personality that provides it, or the
/// default - `state_context_provider`, unless the configuration names
/// another (`AppConfig::component_defaults`). Two would overwrite each
/// other, so a personality with two is refused.
#[async_trait]
pub trait ContextProvider: Send + Sync + std::fmt::Debug {
    async fn current_context(&self, turn: &mut Turn) -> Result<()>;
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
