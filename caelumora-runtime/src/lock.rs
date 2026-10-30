//! `LocksById`: one lock per id, kept in its table only while it is held or waited for.
//!
//! A manager that must not run two operations on one entity at once - starting and deleting a
//! session, restoring and purging a project - holds a table of them by the entity's id. An id's
//! lock comes into the table with the first caller who asks for it and leaves it with the last who
//! lets go, holder or waiter, so that the table grows with nothing but the ids in use, and no
//! caller is given a new lock while another still holds or waits for the old one (issue 495).

use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// One lock per id, each in the table only while it is held or waited for.
pub(crate) struct LocksById<K: Eq + Hash> {
    locks: DashMap<K, Arc<Mutex<()>>>,
}

impl<K: Eq + Hash + Copy> LocksById<K> {
    /// An empty table.
    pub(crate) fn new() -> Self {
        Self { locks: DashMap::new() }
    }

    /// Waits for the lock of `id` and holds it until the guard is dropped.
    pub(crate) async fn lock(&self, id: K) -> IdLockGuard<'_, K> {
        let lock = Arc::clone(&self.locks.entry(id).or_default());
        // Made before the wait, so that a waiter given up takes the entry out as well.
        let claim = Claim { locks: &self.locks, id };
        let held = lock.lock_owned().await;
        IdLockGuard { _held: held, _claim: claim }
    }

    /// The ids whose lock is held or waited for now.
    pub(crate) fn ids(&self) -> Vec<K> {
        self.locks.iter().map(|entry| *entry.key()).collect()
    }
}

/// The lock of one id held: its entry goes when the last holder or waiter lets go.
pub(crate) struct IdLockGuard<'a, K: Eq + Hash> {
    // Dropped in this order: the lock is let go before the claim looks whether it was the last.
    _held: OwnedMutexGuard<()>,
    _claim: Claim<'a, K>,
}

/// A caller's part in the entry of `id`, from asking for its lock until letting it go: the last
/// one takes the entry out of the table.
struct Claim<'a, K: Eq + Hash> {
    locks: &'a DashMap<K, Arc<Mutex<()>>>,
    id: K,
}

impl<K: Eq + Hash> Drop for Claim<'_, K> {
    fn drop(&mut self) {
        // Under the lock of the table's shard, where nobody takes the lock out meanwhile: a count
        // of one is the table's own, and nobody else holds or waits for it.
        self.locks.remove_if(&self.id, |_, lock| Arc::strong_count(lock) == 1);
    }
}

#[cfg(test)]
mod tests {
    use super::LocksById;
    use std::future;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio::time;

    #[tokio::test]
    async fn whoever_comes_after_a_release_waits_for_whoever_waited_before() {
        let locks = Arc::new(LocksById::new());
        let first = locks.lock(1).await;
        let (held_tx, held_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let waiting = tokio::spawn({
            let locks = Arc::clone(&locks);
            async move {
                let _second = locks.lock(1).await;
                held_tx.send(()).unwrap();
                release_rx.await.unwrap();
            }
        });
        time::sleep(Duration::from_millis(20)).await;
        drop(first);
        held_rx.await.unwrap();

        assert!(
            time::timeout(Duration::from_millis(50), locks.lock(1)).await.is_err(),
            "a third caller waits for the second, who holds the lock"
        );
        release_tx.send(()).unwrap();
        waiting.await.unwrap();
        let third = time::timeout(Duration::from_millis(50), locks.lock(1)).await;
        assert!(third.is_ok(), "once the second lets go, the third holds the lock");
    }

    #[tokio::test]
    async fn the_table_holds_only_the_ids_in_use() {
        let locks = LocksById::new();
        let one = locks.lock(1).await;
        let two = locks.lock(2).await;
        let mut ids = locks.ids();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2], "both locks held are in the table");

        drop(one);
        assert_eq!(locks.ids(), vec![2], "a lock let go leaves the table");
        drop(two);
        assert!(locks.ids().is_empty(), "the table is empty once nobody holds a lock");
    }

    #[tokio::test]
    async fn a_waiter_given_up_leaves_no_entry() {
        let locks = Arc::new(LocksById::new());
        let first = locks.lock(1).await;
        let waiting = tokio::spawn({
            let locks = Arc::clone(&locks);
            async move {
                let _second = locks.lock(1).await;
                future::pending::<()>().await;
            }
        });
        time::sleep(Duration::from_millis(20)).await;

        // Let go while the second waits, which is given up before it runs again.
        drop(first);
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled(), "the waiter is given up");
        assert!(locks.ids().is_empty(), "the waiter given up takes the entry out with it");
    }
}
