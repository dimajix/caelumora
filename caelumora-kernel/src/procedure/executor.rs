//! `ProcedureExecutor`: carries procedures out, each in a task of its own, while it holds the
//! objects it changes.

use super::compensation::Compensation;
use super::locks::{Holder, Locks, ProcedureId, Table};
use super::{Procedure, ProcedureError};
use std::any;
use std::sync;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time;
use tokio_util::task::TaskTracker;
use tracing::{error, warn};

tokio::task_local! {
    /// The procedure the current task carries out, if it carries one out (`ProcedureExecutor`).
    static CARRYING_OUT: ProcedureId;
}

/// What carries the procedures of a process out, each in a task of its own, so that a caller
/// giving up does not cut one off midway: it keeps the table of the objects they hold, defers one
/// whose object another holds, and has it go on once the object is free. Made once by the
/// `KernelBuilder` (`Kernel::procedures`), and held by the entry points - the frontends, a plugin's
/// writer, whoever follows an event -, never by a building block of a procedure.
#[derive(Debug)]
pub struct ProcedureExecutor {
    table: Arc<Table>,
    next: AtomicU64,
    tasks: TaskTracker,
    /// Whether it admits procedures; no longer once it shuts down.
    admitting: sync::Mutex<bool>,
}

impl ProcedureExecutor {
    /// An executor that holds nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            table: Arc::default(),
            next: AtomicU64::new(0),
            tasks: TaskTracker::new(),
            admitting: sync::Mutex::new(true),
        }
    }

    /// Carries `procedure` out in a task of its own and answers what it ended with. Dropping the
    /// answer leaves the procedure to go on.
    ///
    /// # Errors
    /// What the procedure fails with; `ProcedureError::Nested` if asked for in carrying out
    /// another, `ShuttingDown` once the executor shuts down, `Ended` if its task ended without an
    /// answer.
    pub async fn run<P: Procedure>(&self, procedure: P) -> Result<P::Output, P::Error> {
        if let Ok(outer) = CARRYING_OUT.try_with(|outer| *outer) {
            error!(
                "{} is asked for in carrying out procedure {}, which holds what it would wait for",
                any::type_name::<P>(),
                outer
            );
            return Err(ProcedureError::Nested.into());
        }
        let (answer, answered) = oneshot::channel();
        {
            let admitting = self.admitting.lock().unwrap_or_else(sync::PoisonError::into_inner);
            if !*admitting {
                return Err(ProcedureError::ShuttingDown.into());
            }
            let table = Arc::clone(&self.table);
            let id = self.next_id();
            self.tasks.spawn(CARRYING_OUT.scope(id, async move {
                let _ = answer.send(carry_out(&table, id, procedure).await);
            }));
        }
        answered.await.unwrap_or_else(|_| {
            warn!("{} ended without an answer", any::type_name::<P>());
            Err(ProcedureError::Ended.into())
        })
    }

    /// Admits no procedure any more, waits for those under way, then carries out `last` - all
    /// within `within`; `None` if that was not enough, and what is still under way is left to end
    /// with the process.
    pub async fn shut_down<P: Procedure>(
        &self,
        last: P,
        within: Duration,
    ) -> Option<Result<P::Output, P::Error>> {
        *self.admitting.lock().unwrap_or_else(sync::PoisonError::into_inner) = false;
        self.tasks.close();
        let id = self.next_id();
        let ending = async {
            self.tasks.wait().await;
            CARRYING_OUT.scope(id, carry_out(&self.table, id, last)).await
        };
        time::timeout(within, ending).await.ok()
    }

    fn next_id(&self) -> ProcedureId {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

/// Carries `procedure`, of `id`, out: enumerates it until it holds what it found without a
/// conflict, waiting at each conflict holding nothing; then carries it out, undoing what it
/// recorded if it fails, and lets go of everything.
async fn carry_out<P: Procedure>(
    table: &Table,
    id: ProcedureId,
    procedure: P,
) -> Result<P::Output, P::Error> {
    let name = any::type_name::<P>();
    let mut locks = Locks::new(table, Holder { id, name });
    let plan = loop {
        let enumerated = procedure.enumerate(&mut locks).await;
        match locks.checked(enumerated) {
            Ok(plan) => break plan,
            Err(deferred) => locks.wait_for(deferred).await,
        }
    };
    locks.settle();
    let admitted = locks.admitted();
    let mut log = Compensation::default();
    let carried_out = procedure.carry_out(plan, &admitted, &mut log).await;
    if carried_out.is_err() {
        log.undo(name, &admitted).await;
    }
    drop(locks);
    carried_out
}

#[cfg(test)]
mod tests {
    use super::ProcedureExecutor;
    use crate::procedure::{
        Admitted, Compensation, Deferred, Key, Locks, Procedure, ProcedureError,
    };
    use async_trait::async_trait;
    use caelumora_domain::channel::ChannelId;
    use caelumora_domain::session::SessionId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio::time;

    /// A procedure of the tests: locks `keys` as told, counts how often it enumerated, records
    /// in `log` when it begins and ends carrying out, and holds its keys until `release` lets it
    /// go on; records an undo, then fails, if told to fail.
    struct Probe {
        name: &'static str,
        keys: Vec<(Key, bool)>,
        enumerated: Arc<AtomicUsize>,
        log: Arc<Mutex<Vec<String>>>,
        release: Option<Arc<Semaphore>>,
        fail: bool,
    }

    impl Probe {
        fn new(name: &'static str, log: &Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                name,
                keys: Vec::new(),
                enumerated: Arc::new(AtomicUsize::new(0)),
                log: Arc::clone(log),
                release: None,
                fail: false,
            }
        }

        fn shared(mut self, key: Key) -> Self {
            self.keys.push((key, false));
            self
        }

        fn exclusive(mut self, key: Key) -> Self {
            self.keys.push((key, true));
            self
        }

        fn held_until(mut self, release: &Arc<Semaphore>) -> Self {
            self.release = Some(Arc::clone(release));
            self
        }
    }

    #[async_trait]
    impl Procedure for Probe {
        type Plan = ();
        type Output = &'static str;
        type Error = anyhow::Error;

        async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
            self.enumerated.fetch_add(1, Ordering::SeqCst);
            for (key, exclusive) in &self.keys {
                match exclusive {
                    true => locks.exclusive(*key)?,
                    false => locks.shared(*key)?,
                }
            }
            Ok(())
        }

        async fn carry_out(
            self,
            _: (),
            _: &Admitted<'_>,
            log: &mut Compensation,
        ) -> anyhow::Result<&'static str> {
            self.log.lock().unwrap().push(format!("{} begins", self.name));
            if let Some(release) = &self.release {
                release.acquire().await.unwrap().forget();
            }
            self.log.lock().unwrap().push(format!("{} ends", self.name));
            if self.fail {
                let first = Arc::clone(&self.log);
                log.record("the first step", move |_| {
                    Box::pin(async move {
                        first.lock().unwrap().push("first undone".to_string());
                        Ok(())
                    })
                });
                let second = Arc::clone(&self.log);
                log.record("the second step", move |_| {
                    Box::pin(async move {
                        second.lock().unwrap().push("second undone".to_string());
                        anyhow::bail!("it stays")
                    })
                });
                anyhow::bail!("{} fails", self.name);
            }
            Ok(self.name)
        }
    }

    /// Waits until `log` holds `entry`.
    async fn until_logged(log: &Arc<Mutex<Vec<String>>>, entry: &str) {
        time::timeout(Duration::from_secs(5), async {
            while !log.lock().unwrap().iter().any(|logged| logged == entry) {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{entry:?} never came: {:?}", log.lock().unwrap()));
    }

    fn session() -> Key {
        Key::Session(SessionId::new())
    }

    fn channel_of(session: Key) -> Key {
        let Key::Session(session) = session else { unreachable!("a session's key") };
        Key::Channel(session, ChannelId::new())
    }

    /// Two that change one object take turns: the second begins once the first has ended.
    #[tokio::test]
    async fn two_that_lock_one_key_exclusively_take_turns() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let key = session();
        let release = Arc::new(Semaphore::new(0));
        let first = Probe::new("first", &log).exclusive(key).held_until(&release);
        let running = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(first).await }
        });
        until_logged(&log, "first begins").await;
        let second = tokio::spawn({
            let executor = Arc::clone(&executor);
            let second = Probe::new("second", &log).exclusive(key);
            async move { executor.run(second).await }
        });
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*log.lock().unwrap(), vec!["first begins"], "the second waits");
        release.add_permits(1);
        assert_eq!(running.await.unwrap().unwrap(), "first");
        assert_eq!(second.await.unwrap().unwrap(), "second");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["first begins", "first ends", "second begins", "second ends"]
        );
    }

    /// Two that only need an object to stay as it is hold it side by side.
    #[tokio::test]
    async fn two_that_lock_one_key_shared_hold_it_side_by_side() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let key = session();
        let release = Arc::new(Semaphore::new(0));
        let mut running = Vec::new();
        for name in ["first", "second"] {
            let probe = Probe::new(name, &log).shared(key).held_until(&release);
            let executor = Arc::clone(&executor);
            running.push(tokio::spawn(async move { executor.run(probe).await }));
        }
        until_logged(&log, "first begins").await;
        until_logged(&log, "second begins").await;
        release.add_permits(2);
        for running in running {
            running.await.unwrap().unwrap();
        }
    }

    /// A key locks its parents shared: a session locked exclusively keeps out who locks a channel
    /// of it, and a channel locked keeps out whoever changes its session.
    #[tokio::test]
    async fn a_key_locks_its_parents_shared() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let session = session();
        let channel = channel_of(session);
        let release = Arc::new(Semaphore::new(0));
        let holding = Probe::new("channel", &log).exclusive(channel).held_until(&release);
        let held = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(holding).await }
        });
        until_logged(&log, "channel begins").await;
        let stopping = tokio::spawn({
            let executor = Arc::clone(&executor);
            let stop = Probe::new("session", &log).exclusive(session);
            async move { executor.run(stop).await }
        });
        let other = Probe::new("other channel", &log).exclusive(channel_of(session));
        let beside = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(other).await }
        });
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            *log.lock().unwrap(),
            vec!["channel begins"],
            "the session waits for its channel, and keeps out another channel behind it"
        );
        release.add_permits(1);
        held.await.unwrap().unwrap();
        stopping.await.unwrap().unwrap();
        beside.await.unwrap().unwrap();
        let log = log.lock().unwrap();
        let at = |entry: &str| log.iter().position(|logged| logged == entry);
        assert!(at("session ends") < at("other channel begins"), "fair: {log:?}");
    }

    /// Fair: one that waits for an object keeps out whoever comes after it, shared too, so that it
    /// is not starved by a stream of those that only read.
    #[tokio::test]
    async fn one_that_waits_at_a_key_keeps_out_whoever_comes_after_it() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let key = session();
        let release = Arc::new(Semaphore::new(0));
        let reading = Probe::new("reader", &log).shared(key).held_until(&release);
        let reader = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(reading).await }
        });
        until_logged(&log, "reader begins").await;
        let stop = Probe::new("stop", &log).exclusive(key);
        let stopping = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(stop).await }
        });
        time::sleep(Duration::from_millis(20)).await;
        let late = Probe::new("late reader", &log).shared(key);
        let late = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(late).await }
        });
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*log.lock().unwrap(), vec!["reader begins"], "the late reader waits too");
        release.add_permits(1);
        reader.await.unwrap().unwrap();
        stopping.await.unwrap().unwrap();
        late.await.unwrap().unwrap();
        let log = log.lock().unwrap();
        let at = |entry: &str| log.iter().position(|logged| logged == entry);
        assert!(at("stop ends") < at("late reader begins"), "the stop first: {log:?}");
    }

    /// A procedure deferred lets go of what it held, so that nobody waits for it in a circle, and
    /// enumerates anew once it goes on - since what it found may have changed meanwhile.
    #[tokio::test]
    async fn one_deferred_lets_go_of_what_it_held_and_enumerates_anew() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let (one, two) = (session(), session());
        let release = Arc::new(Semaphore::new(0));
        let holding = Probe::new("holder", &log).exclusive(two).held_until(&release);
        let holder = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(holding).await }
        });
        until_logged(&log, "holder begins").await;
        let both = Probe::new("both", &log).exclusive(one).exclusive(two);
        let enumerated = Arc::clone(&both.enumerated);
        let deferred = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(both).await }
        });
        time::sleep(Duration::from_millis(50)).await;
        let first = Probe::new("first alone", &log).exclusive(one);
        executor.run(first).await.expect("what the deferred one locked first is free meanwhile");
        release.add_permits(1);
        holder.await.unwrap().unwrap();
        deferred.await.unwrap().unwrap();
        assert_eq!(enumerated.load(Ordering::SeqCst), 2, "enumerated again once it went on");
        assert!(!executor.table.knows(one) && !executor.table.knows(two), "and all let go");
    }

    /// No procedure within another: it would wait for the keys the outer one holds.
    #[tokio::test]
    async fn a_procedure_is_not_carried_out_within_another() {
        struct Outer(Arc<ProcedureExecutor>, Key);

        #[async_trait]
        impl Procedure for Outer {
            type Plan = ();
            type Output = ();
            type Error = anyhow::Error;

            async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
                locks.exclusive(self.1)
            }

            async fn carry_out(
                self,
                _: (),
                _: &Admitted<'_>,
                _: &mut Compensation,
            ) -> anyhow::Result<()> {
                let log = Arc::new(Mutex::new(Vec::new()));
                let inner = Probe::new("inner", &log).exclusive(self.1);
                self.0.run(inner).await.map(drop)
            }
        }

        let executor = Arc::new(ProcedureExecutor::new());
        let error = executor
            .run(Outer(Arc::clone(&executor), session()))
            .await
            .expect_err("the inner one is refused, not waited for");
        assert!(matches!(error.downcast_ref(), Some(ProcedureError::Nested)), "{error}");
    }

    /// A procedure goes on in its own task when its caller gives up waiting for it.
    #[tokio::test]
    async fn a_caller_giving_up_does_not_cut_a_procedure_off() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let probe = Probe::new("given up", &log).exclusive(session()).held_until(&release);
        let waiting = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(probe).await }
        });
        until_logged(&log, "given up begins").await;
        waiting.abort();
        release.add_permits(1);
        until_logged(&log, "given up ends").await;
    }

    /// What a procedure that fails recorded is undone, the last first; what cannot be is logged,
    /// and the rest undone all the same. Its keys are let go.
    #[tokio::test]
    async fn what_a_procedure_that_fails_did_is_undone_the_last_first() {
        let executor = ProcedureExecutor::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        let key = session();
        let failing = Probe { fail: true, ..Probe::new("failing", &log).exclusive(key) };
        let error = executor.run(failing).await.expect_err("it fails");
        assert_eq!(error.to_string(), "failing fails");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["failing begins", "failing ends", "second undone", "first undone"]
        );
        assert!(!executor.table.knows(key), "let go");
    }

    /// Shutting down, the executor admits no procedure any more, waits for those under way and
    /// then carries out the last.
    #[tokio::test]
    async fn shutting_down_waits_for_what_is_under_way_and_admits_nothing_new() {
        let executor = Arc::new(ProcedureExecutor::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let under_way = Probe::new("under way", &log).exclusive(session()).held_until(&release);
        let running = tokio::spawn({
            let executor = Arc::clone(&executor);
            async move { executor.run(under_way).await }
        });
        until_logged(&log, "under way begins").await;
        let shutting_down = tokio::spawn({
            let executor = Arc::clone(&executor);
            let last = Probe::new("last", &log).exclusive(session());
            async move { executor.shut_down(last, Duration::from_secs(5)).await }
        });
        time::sleep(Duration::from_millis(20)).await;
        let late = executor.run(Probe::new("late", &log)).await.expect_err("not admitted");
        assert!(matches!(late.downcast_ref(), Some(ProcedureError::ShuttingDown)), "{late}");
        release.add_permits(1);
        running.await.unwrap().unwrap();
        let last = shutting_down.await.unwrap().expect("within its bound");
        assert_eq!(last.unwrap(), "last");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["under way begins", "under way ends", "last begins", "last ends"]
        );
    }
}
