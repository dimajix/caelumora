//! Compaction (issues 16, 464): an agent's context replaced by a shorter one when it grows too long
//! for its model, as a plugin of its own (`plugin::CompactionPlugin`).
//!
//! When a context is compacted and how are kept apart. The [`component`] - `compaction` -
//! compacts the agent's context on its own, as an activity (`caelumora_agents::activity`, issue
//! 284), once it fills a share of its model's size, and carries out `/compact @agent` for the
//! agent's owners; an agent whose personality does not list it does not compact. What stands in
//! the place of the older part of the context is left to the agent's [`CompactionStrategy`] in
//! either case - by default the [`summary_compaction`], which has the agent's model summarise it.
//! Of strategies an agent has exactly one.
//!
//! Whatever the strategy, the rules of a compaction are the same, and the component's (issue
//! 413): where the transcript is cut, whose the replacement is and how it is classified, what is
//! given again; the context is replaced, not rewritten: the transcript begins anew on a branch of
//! its own (`Transcript::compact`), and what the agent saw before stays recorded and resolvable
//! (`AgentChannelState::compact`).
//!
//! What a component gave the agent - a skill loaded - is given again after
//! a compaction by that component, a [`KnowledgeRestorer`] (issue 323): the
//! compaction asks the restorers of the agent's components, and knows no
//! kind of knowledge itself.

mod compactor;
pub mod component;
mod cut;
pub mod plugin;
pub mod summary_compaction;
#[cfg(test)]
mod testing;

use async_trait::async_trait;
use caelumora_agents::turn::Turn;
use caelumora_model::context::KnowledgeRef;
use caelumora_model::message::Message;
use caelumora_model::prompt_catalogue::PromptError;
use caelumora_model::store::EntityKind;
use std::fmt::Debug;
use uuid::Uuid;

/// The namespace of the texts of compaction in the prompt catalogue (issue 57).
pub const NAMESPACE: &str = "compaction";

/// Their texts, added to the catalogue under `NAMESPACE` by the plugin.
pub const PROMPTS: &str = include_str!("prompts.yaml");

/// What stands in the place of the older part of an agent's context when it is compacted. An agent
/// has exactly one; where the part ends, what was given in it, and how the context is replaced is
/// its `compaction` component's, as is when it compacts.
#[async_trait]
pub trait CompactionStrategy: Send + Sync + Debug {
    /// What replaces `part` - the older part of `turn`'s context, as its model is given it - for
    /// the agent to go on from. `given` names, by kind and name, what was given in the part and is
    /// given again after the replacement.
    ///
    /// # Errors
    /// `ModelUnreachable` if a model it asks cannot be asked, `NoSummary` if it answers nothing,
    /// `Prompt` if a text it asks with cannot be had.
    async fn replace(
        &self,
        turn: &Turn<'_>,
        part: Vec<Message>,
        given: &[String],
    ) -> Result<Replacement, CompactionError>;
}

/// What a strategy puts in the place of the part of the context it replaces.
#[derive(Debug)]
pub struct Replacement {
    /// The text that stands for the part, as the agent is given it.
    pub text: String,
    /// The message its model answered with, which carries the measures of the call that wrote the
    /// text.
    pub call: Message,
}

/// What a component gave the agent and gives again after a compaction
/// (issue 323), as it is now: the entities of one kind recorded as given -
/// as `KnowledgeSource::Entity`, in a `TranscriptEntry::Given` or as what a
/// turn read - in the part of the transcript a compaction replaces. A facet
/// of the component that gave them, as a skill is given by the `skills`
/// component.
#[async_trait]
pub trait KnowledgeRestorer: Send + Sync + Debug {
    /// The kind of what it gives again.
    fn kind(&self) -> EntityKind;

    /// What of `given` - entities of its kind, in the order they were first
    /// given - it gives `turn`'s agent again, as each is now; one it can no
    /// longer give there is left out.
    ///
    /// # Errors
    /// `KnowledgeUnreadable` if what it gives cannot be read, `NoSession`
    /// if the agent is in none.
    async fn give_again(
        &self,
        turn: &mut Turn<'_>,
        given: &[Uuid],
    ) -> Result<Vec<GivenAgain>, CompactionError>;
}

/// What a `KnowledgeRestorer` gives again: its name, as the agent's owners
/// are told it, the text the agent is given, as the message that follows
/// the summary - classified as `knowledge` says -, and the knowledge that
/// message records.
#[derive(Debug, Clone, PartialEq)]
pub struct GivenAgain {
    pub name: String,
    pub text: String,
    pub knowledge: KnowledgeRef,
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    /// How many messages were replaced.
    pub replaced: usize,
    /// How many messages were kept as they were.
    pub kept: usize,
    /// What was given again, as it is now, each by its kind and name -
    /// `skill release-notes`.
    pub given_again: Vec<String>,
    /// How much the agent had been given before that can no longer be
    /// given and was not given again.
    pub lost: usize,
}

impl Compaction {
    /// What it did, in words for the users of the agent `agent_name`'s
    /// channel: as `/compact` answers, and as the activity of compacting
    /// ends (issue 284).
    pub fn describe(&self, agent_name: &str) -> String {
        let mut text = format!(
            "{} compacted its context: {} messages replaced, {} kept as they were.",
            agent_name, self.replaced, self.kept
        );
        if !self.given_again.is_empty() {
            text.push_str(&format!(" Given again: {}.", self.given_again.join(", ")));
        }
        if self.lost > 0 {
            text.push_str(&format!(" {} of what it had been given can no longer be given here and was not given again.", self.lost));
        }
        text
    }
}

/// Why a compaction failed. Its text says so to the agent's owners.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("the transcript could not be read")]
    TranscriptUnreadable,
    #[error("its model could not be asked")]
    ModelUnreachable,
    #[error("its model wrote no summary")]
    NoSummary,
    #[error("what it had been given could not be read again")]
    KnowledgeUnreadable,
    #[error("the agent is in no session")]
    NoSession,
    #[error("the context could not be replaced")]
    NotReplaced,
    /// A text it is given, or asks its model with, cannot be had (issue 57).
    #[error("{0}")]
    Prompt(#[from] PromptError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compaction_tells_what_it_did() {
        let compaction = Compaction {
            replaced: 3,
            kept: 1,
            given_again: vec!["skill release-notes".to_string()],
            lost: 1,
        };
        assert_eq!(
            compaction.describe("Jonathan"),
            "Jonathan compacted its context: 3 messages replaced, 1 kept as they were. Given again: skill release-notes. \
            1 of what it had been given can no longer be given here and was not given again."
        );
    }
}
