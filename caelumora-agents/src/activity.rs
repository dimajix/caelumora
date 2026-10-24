//! What an agent does of its own accord within a turn, before a model call (issues 284, 464), as a
//! facet of its components - compacting its context -: the counterpart of the commands it carries
//! out when asked (`command`).
//!
//! The actor asks its activities before every call of its model whether one is due, and carries
//! out the first one due in the state `Busy`, telling the channel that it does so
//! (`ActivityStarted`) and what came of it (`ActivityFinished`); then it asks again, until none is
//! due, and makes the call. Each is tried at most once before a call (`Turn::try_activity`). What
//! an activity changes in the agent's context it changes through the turn it is given; where the
//! agent goes on after it - its next call - is the actor's.

use crate::situation::Situation;
use crate::turn::Turn;
use async_trait::async_trait;
use std::fmt::Debug;

/// What a component does of the agent's own accord before a model call (issue 284) - compacting
/// its context. An agent has any number.
#[async_trait]
pub trait ActivityProvider: Send + Sync + Debug {
    /// What it does, as its channel is told it begins - "compacting its context".
    fn activity(&self) -> &str;

    /// Whether it is due before the next model call of `situation`.
    fn is_due(&self, situation: &Situation<'_>) -> bool;

    /// Carries it out on `turn`: what came of it, in words for the channel.
    ///
    /// # Errors
    /// `ActivityError` if it could not be carried out; the turn ends then, given up as failed.
    async fn carry_out(&self, turn: &mut Turn<'_>) -> Result<String, ActivityError>;
}

/// Why an `ActivityProvider` could not carry out its activity (issue 336): a plugin's provider
/// translates its own errors into it, deciding there what may be shown.
#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    /// A failure in words that may be shown to users.
    #[error("{0}")]
    Failed(String),
}
