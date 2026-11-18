//! `Compensation`: what a procedure did, recorded step by step, and undone the last first by the
//! executor if a later step fails.
//!
//! It is no rollback: an event announced or an actor started can be balanced, not taken back, and
//! what is no procedure sees the states in between. What cannot be undone is logged and stays.

use super::Admitted;
use futures::future::BoxFuture;
use tracing::warn;

/// How a step is undone: given the proof that the procedure still holds its objects, it answers
/// whether it could be.
type Undo = Box<dyn for<'a> FnOnce(&'a Admitted<'a>) -> BoxFuture<'a, anyhow::Result<()>> + Send>;

/// What a procedure did, step by step, to be undone the last first if a later step fails.
#[derive(Default)]
pub struct Compensation {
    steps: Vec<(&'static str, Undo)>,
}

impl Compensation {
    /// Records how to undo the step just done, `what` naming it in the log if it cannot be.
    pub fn record(
        &mut self,
        what: &'static str,
        undo: impl for<'a> FnOnce(&'a Admitted<'a>) -> BoxFuture<'a, anyhow::Result<()>>
            + Send
            + 'static,
    ) {
        self.steps.push((what, Box::new(undo)));
    }

    /// Undoes the steps recorded, the last first, for `procedure`, which failed; logs what cannot
    /// be undone.
    pub(super) async fn undo(self, procedure: &str, admitted: &Admitted<'_>) {
        for (what, undo) in self.steps.into_iter().rev() {
            if let Err(error) = undo(admitted).await {
                warn!("{procedure} failed, and {what} could not be undone: {error:#}");
            }
        }
    }
}
