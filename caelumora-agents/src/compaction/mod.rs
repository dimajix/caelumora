//! Compaction (issue 16): an agent's context replaced by a shorter one when
//! it grows too long for its model.
//!
//! When a context is compacted and how are kept apart. The agent compacts
//! on its own when its [`CompactionPolicy`] says so (issue 284) - by default
//! the [`threshold_compaction_policy`], once the context fills a share of
//! its model's size -, and the [`command`] component carries out `/compact
//! @agent` for the agent's owners. How the context is compacted is left to
//! the agent's [`CompactionStrategy`] in either case - by default the
//! [`summary_compaction`], which has the agent's model summarise the older
//! part of its transcript. Of either the agent has exactly one.
//!
//! Whatever the strategy, the context is replaced, not rewritten: the
//! transcript begins anew on a branch of its own, and what the agent saw
//! before stays recorded and resolvable (`AgentChannelState::compact`).
//!
//! What a component gave the agent - a skill loaded - is given again after
//! a compaction by that component, a [`KnowledgeRestorer`] (issue 323): a
//! strategy asks the restorers of the agent's components, and knows no
//! kind of knowledge itself.

pub mod command;
pub mod summary_compaction;
pub mod threshold_compaction_policy;

use crate::turn::Turn;
use crate::situation::Situation;
use caelumora_runtime::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_model::context::KnowledgeRef;
use caelumora_model::store::EntityKind;
use std::fmt::Debug;
use uuid::Uuid;

/// When an agent compacts its context on its own (issue 284): before which
/// call of its model. An agent has exactly one; how it is compacted is its
/// `CompactionStrategy`'s.
pub trait CompactionPolicy: Send + Sync + Debug {
    /// Whether the context of `situation` is to be compacted before its next
    /// model call. Asked at most once before each call.
    fn is_due(&self, situation: &Situation<'_>) -> bool;
}

/// How an agent's context is compacted: where it is cut and what replaces
/// the part before the cut. An agent has exactly one; when it compacts on
/// its own is its `CompactionPolicy`'s.
#[async_trait]
pub trait CompactionStrategy: Send + Sync + Debug {
    /// Compacts the context of `turn`'s agent, moving the turn's context to
    /// the compacted one - or `None` if there is nothing to compact, and
    /// nothing was changed. Nothing is changed either if it fails.
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<Compaction>, CompactionError>;
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
    async fn give_again(&self, turn: &mut Turn<'_>, given: &[Uuid]) -> Result<Vec<GivenAgain>, CompactionError>;
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
        let mut text = format!("{} compacted its context: {} messages replaced, {} kept as they were.", agent_name, self.replaced, self.kept);
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
}

/// Registers the compaction kinds with `registry`, and the policy and the
/// strategy as facets of which an agent has exactly one, each with its
/// default kind.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn CompactionPolicy>(Some(threshold_compaction_policy::THRESHOLD_COMPACTION_POLICY_KIND));
    registry.single::<dyn CompactionStrategy>(Some(summary_compaction::SUMMARY_COMPACTION_KIND));

    command::register(registry);
    summary_compaction::register(registry);
    threshold_compaction_policy::register(registry);
}
