//! Compaction (issue 16): an agent's context replaced by a shorter one when
//! it grows too long for its model.
//!
//! When a context is compacted and how are kept apart. The [`command`]
//! component carries out `/compact @agent` for the agent's owners; how the
//! context is compacted is left to the agent's [`CompactionStrategy`], a
//! component of its own of which an agent has exactly one - by default the
//! [`summary_compaction`], which has the agent's model summarise the older
//! part of its transcript. Compacting on the agent's own (issue 284) asks
//! the same strategy.
//!
//! Whatever the strategy, the context is replaced, not rewritten: the
//! transcript begins anew on a branch of its own, and what the agent saw
//! before stays recorded and resolvable (`AgentChannelState::compact`).

pub mod command;
pub mod summary_compaction;

use crate::agent::turn::Turn;
use crate::component::ComponentRegistry;
use async_trait::async_trait;
use std::fmt::Debug;

/// How an agent's context is compacted: where it is cut and what replaces
/// the part before the cut. An agent has exactly one.
#[async_trait]
pub trait CompactionStrategy: Send + Sync + Debug {
    /// Compacts the context of `turn`'s agent, moving the turn's context to
    /// the compacted one - or `None` if there is nothing to compact, and
    /// nothing was changed. Nothing is changed either if it fails.
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<Compaction>, CompactionError>;
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    /// How many messages were replaced.
    pub replaced: usize,
    /// How many messages were kept as they were.
    pub kept: usize,
    /// The names of the skills given again, as they are now.
    pub skills_given: Vec<String>,
    /// How many skills the agent had loaded before that can no longer be
    /// loaded and were not given again.
    pub skills_lost: usize,
}

/// Why a compaction failed. Its text says so to the agent's owners.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("the transcript could not be read")]
    TranscriptUnreadable,
    #[error("the agent made all the calls of its model it may in one turn")]
    CallLimitReached,
    #[error("its model could not be asked")]
    ModelUnreachable,
    #[error("its model wrote no summary")]
    NoSummary,
    #[error("the skills could not be read")]
    SkillsUnreadable,
    #[error("the agent is in no session")]
    NoSession,
    #[error("the context could not be replaced")]
    NotReplaced,
}

/// Registers the compaction kinds with `registry`, and the strategy as a
/// facet of which an agent has exactly one, with its default kind.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn CompactionStrategy>(Some(summary_compaction::SUMMARY_COMPACTION_KIND));

    command::register(registry);
    summary_compaction::register(registry);
}
