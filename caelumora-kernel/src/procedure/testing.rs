//! What the tests of procedures share: a procedure that holds a key until it is let go, against
//! which a test sees which keys another procedure locks, and how.

use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure, ProcedureExecutor};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinHandle;
use tokio::time;

/// `key` held - exclusively or shared - by a procedure of its own, until it is let go.
pub(crate) struct Held {
    release: Arc<Semaphore>,
    holding: JoinHandle<()>,
}

impl Held {
    /// Holds `key` through `executor` - exclusively if `exclusive`, shared otherwise -, once the
    /// hold is in place.
    pub(crate) async fn key(executor: &Arc<ProcedureExecutor>, key: Key, exclusive: bool) -> Self {
        let release = Arc::new(Semaphore::new(0));
        let (held, holds) = oneshot::channel();
        let holding = Holding { key, exclusive, held, release: Arc::clone(&release) };
        let holding = tokio::spawn({
            let executor = Arc::clone(executor);
            async move { executor.run(holding).await.unwrap() }
        });
        holds.await.expect("the key is held");
        Self { release, holding }
    }

    /// Lets the key go, once the procedure that holds it is done.
    pub(crate) async fn release(self) {
        self.release.add_permits(1);
        self.holding.await.unwrap();
    }
}

/// Carries `procedure` out through `executor`, holds that it waits - still under way after a
/// while -, lets `held` go and answers what it ended with once it went on.
pub(crate) async fn waits_for<P: Procedure>(
    held: Held,
    executor: &Arc<ProcedureExecutor>,
    procedure: P,
) -> Result<P::Output, P::Error> {
    let executor = Arc::clone(executor);
    let running = tokio::spawn(async move { executor.run(procedure).await });
    time::sleep(Duration::from_millis(50)).await;
    assert!(!running.is_finished(), "it waits for what is held");
    held.release().await;
    time::timeout(Duration::from_secs(5), running)
        .await
        .expect("it goes on once it is let go")
        .unwrap()
}

/// The procedure of `Held`.
struct Holding {
    key: Key,
    exclusive: bool,
    held: oneshot::Sender<()>,
    release: Arc<Semaphore>,
}

#[async_trait]
impl Procedure for Holding {
    type Plan = ();
    type Output = ();
    type Error = anyhow::Error;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        match self.exclusive {
            true => locks.exclusive(self.key),
            false => locks.shared(self.key),
        }
    }

    async fn carry_out(self, (): (), _: &Admitted<'_>, _: &mut Compensation) -> anyhow::Result<()> {
        let _ = self.held.send(());
        self.release.acquire().await?.forget();
        Ok(())
    }
}
