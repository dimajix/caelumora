//! `Compensation`: what a procedure leaves to settle when it is done - the steps it did, undone the
//! last first if a later one fails, and what it keeps, let go after them either way -, a whole
//! procedure's by the executor, a part's at once (`all_or_nothing`).
//!
//! It is no rollback: an event announced or an actor started can be balanced, not taken back, and
//! what is no procedure sees the states in between. What cannot be undone is logged and stays.

use futures::future::BoxFuture;
use std::future::Future;
use std::ops::AsyncFnOnce;
use tracing::warn;

/// What a procedure leaves to settle when it is done: the steps it did, undone the last first if a
/// later one fails, and what it keeps, let go after them either way.
#[derive(Default)]
pub struct Compensation {
    steps: Vec<(&'static str, BoxFuture<'static, anyhow::Result<()>>)>,
    kept: Vec<Box<dyn Send>>,
}

impl Compensation {
    /// Records how to undo the step just done - `undo`, which runs only if it is undone, answering
    /// whether it could be -, `what` naming it in the log if it cannot be.
    pub fn record(
        &mut self,
        what: &'static str,
        undo: impl Future<Output = anyhow::Result<()>> + Send + 'static,
    ) {
        self.steps.push((what, Box::pin(undo)));
    }

    /// Keeps `held` until the procedure is done - or the part carried out all or nothing
    /// (`all_or_nothing`) -, undone too: let go once every step recorded is undone. What holds a
    /// session against the other processes (`SessionManager::hold`).
    pub(crate) fn keep(&mut self, held: impl Send + 'static) {
        self.kept.push(Box::new(held));
    }

    /// Undoes the steps recorded, the last first, for `procedure`, which failed; logs what cannot
    /// be undone. What it keeps is let go after.
    async fn undo(self, procedure: &str) {
        for (what, undo) in self.steps.into_iter().rev() {
            if let Err(error) = undo.await {
                warn!("{procedure} failed, and {what} could not be undone: {error:#}");
            }
        }
        drop(self.kept);
    }
}

/// Carries `part` out with a compensation of its own, undone at once if it fails: a part of a
/// procedure kept or undone as a whole, whatever becomes of the rest of it - what it recorded is
/// let go once it is done, so that a later step that fails leaves it as it is. `what` names it in
/// the log of what cannot be undone.
///
/// # Errors
/// What `part` fails with, once what it did is undone.
pub(crate) async fn all_or_nothing<T, E>(
    what: &str,
    part: impl AsyncFnOnce(&mut Compensation) -> Result<T, E>,
) -> Result<T, E> {
    let mut log = Compensation::default();
    let done = part(&mut log).await;
    if done.is_err() {
        log.undo(what).await;
    }
    done
}
