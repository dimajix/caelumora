//! What ends work a call started outside the process when the call is dropped before it ended
//! (`EndOnDrop`, issues 161, 512): a command in a workspace, a prompt on a server.

use futures::future::BoxFuture;
use std::fmt::Display;
use std::future::Future;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::time;
use tracing::warn;

/// What a call started outside the process - a command in a workspace, a prompt on a server -,
/// ended if this is dropped before `disarm` (issues 161, 512): a call given up - its turn
/// cancelled or stopped, its time run out - would leave it running for nothing. The ending runs in
/// a task of its own, bounded by `within` and logged as `what` where it fails, which the drop does
/// not await; without a runtime to run it in, nothing is ended.
pub struct EndOnDrop {
    what: String,
    within: Duration,
    /// How what was started is ended, once, in a task of its own; `None` once disarmed.
    ending: Option<BoxFuture<'static, Result<(), String>>>,
}

impl EndOnDrop {
    /// Ends what `what` names by `ending`, within `within`, if dropped before `disarm`.
    pub fn new<E: Display>(
        what: String,
        within: Duration,
        ending: impl Future<Output = Result<(), E>> + Send + 'static,
    ) -> Self {
        let ending = async move { ending.await.map_err(|e| e.to_string()) };
        Self { what, within, ending: Some(Box::pin(ending)) }
    }

    /// Leaves what was started running once this is dropped.
    pub fn disarm(mut self) {
        self.ending = None;
    }
}

impl Drop for EndOnDrop {
    fn drop(&mut self) {
        let Some(ending) = self.ending.take() else { return };
        let Ok(runtime) = Handle::try_current() else { return };
        let (what, within) = (self.what.clone(), self.within);
        runtime.spawn(async move {
            match time::timeout(within, ending).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(what = %what, "What a dropped call started was not ended: {e}")
                }
                Err(_) => {
                    warn!(what = %what, "What a dropped call started was not ended in time")
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::task;

    /// A guard that sets `ended` as it ends what it guards.
    fn guard(ended: &Arc<AtomicBool>) -> EndOnDrop {
        let ended = Arc::clone(ended);
        EndOnDrop::new("work".to_string(), Duration::from_secs(1), async move {
            ended.store(true, Ordering::SeqCst);
            Ok::<(), String>(())
        })
    }

    #[tokio::test]
    async fn what_was_started_is_ended_once_dropped_and_left_once_disarmed() {
        let ended = Arc::new(AtomicBool::new(false));
        drop(guard(&ended));
        task::yield_now().await;
        assert!(ended.load(Ordering::SeqCst), "dropped: ended");

        let left = Arc::new(AtomicBool::new(false));
        guard(&left).disarm();
        task::yield_now().await;
        assert!(!left.load(Ordering::SeqCst), "disarmed: left running");
    }
}
