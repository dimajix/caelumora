//! The table of what each procedure holds and which waits where, and `Locks`, through which a
//! procedure enumerating its objects locks them.
//!
//! A key is held shared by any number of procedures or exclusively by one. A lock is never waited
//! for while anything is held: a conflict defers the procedure (`Deferred`), which lets go of
//! everything and waits at the key in its queue (`Table::wait`), holding nothing. A key whose queue
//! is not empty admits nobody but the first in it - fair, so that nobody is starved -, who keeps
//! its place while it enumerates again, until it locks the key, defers elsewhere or is done. A key
//! is in the table only while it is held or waited for.

use super::Key;
use crate::channel::LEAVE_GRACE;
use std::collections::hash_map::Entry as MapEntry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::pin;
use std::sync;
use tokio::sync::Notify;
use tokio::time;
use tracing::warn;

/// Which of the procedures under way one is, for the table.
pub(super) type ProcedureId = u64;

/// How a key is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Mode {
    /// It stays as it is: others may hold it shared too.
    Shared,
    /// It is changed: nobody else holds it.
    Exclusive,
}

/// A procedure, as the table names it in the log: its id and the name of its type.
#[derive(Debug, Clone, Copy)]
pub(super) struct Holder {
    pub(super) id: ProcedureId,
    pub(super) name: &'static str,
}

/// Who holds a key and how, and who waits for it, in order.
#[derive(Debug, Default)]
struct Entry {
    holders: HashMap<ProcedureId, (Mode, &'static str)>,
    waiting: VecDeque<ProcedureId>,
}

impl Entry {
    /// Whether `id` may hold the key as `mode` asks besides those who hold it now.
    fn admits(&self, id: ProcedureId, mode: Mode) -> bool {
        let mut others = self.holders.iter().filter(|(holder, _)| **holder != id);
        match mode {
            Mode::Exclusive => others.next().is_none(),
            Mode::Shared => others.all(|(_, (held, _))| *held == Mode::Shared),
        }
    }

    /// Whether it is neither held nor waited for, and leaves the table.
    fn is_unused(&self) -> bool {
        self.holders.is_empty() && self.waiting.is_empty()
    }
}

/// The keys held and waited for by the procedures under way.
#[derive(Debug, Default)]
pub(super) struct Table {
    entries: sync::Mutex<HashMap<Key, Entry>>,
    /// Told whenever a key is let go or a waiter leaves its queue.
    changed: Notify,
}

impl Table {
    fn entries(&self) -> sync::MutexGuard<'_, HashMap<Key, Entry>> {
        self.entries.lock().unwrap_or_else(sync::PoisonError::into_inner)
    }

    /// Locks `key` for `holder` as `mode` asks, if it admits it now; whether it did. One it holds
    /// already is held as the stronger of both modes.
    fn try_lock(&self, holder: Holder, key: Key, mode: Mode) -> bool {
        let mut entries = self.entries();
        let entry = entries.entry(key).or_default();
        if entry.holders.get(&holder.id).is_some_and(|(held, _)| *held >= mode) {
            return true;
        }
        let first = entry.waiting.front().copied();
        if first.is_some_and(|waiter| waiter != holder.id) || !entry.admits(holder.id, mode) {
            return false;
        }
        if first == Some(holder.id) {
            entry.waiting.pop_front();
        }
        let held = entry.holders.entry(holder.id).or_insert((mode, holder.name));
        held.0 = held.0.max(mode);
        true
    }

    /// Lets go of `keys`, held by `id`.
    fn release(&self, id: ProcedureId, keys: impl IntoIterator<Item = Key>) {
        let mut entries = self.entries();
        for key in keys {
            if let MapEntry::Occupied(mut entry) = entries.entry(key) {
                entry.get_mut().holders.remove(&id);
                if entry.get().is_unused() {
                    entry.remove();
                }
            }
        }
        drop(entries);
        self.changed.notify_waiters();
    }

    /// Takes `id` out of the queue of `key`, where it waited or kept its place.
    fn leave_queue(&self, id: ProcedureId, key: Key) {
        let mut entries = self.entries();
        if let MapEntry::Occupied(mut entry) = entries.entry(key) {
            entry.get_mut().waiting.retain(|waiter| *waiter != id);
            if entry.get().is_unused() {
                entry.remove();
            }
        }
        drop(entries);
        self.changed.notify_waiters();
    }

    /// Waits, holding nothing, until `holder` is the first in the queue of `key` and the key admits
    /// it as `mode` asks; it keeps its place there. Logs a wait longer than `LEAVE_GRACE`, with
    /// whoever holds the key: what a procedure waits for must not wait for a procedure itself.
    async fn wait(&self, holder: Holder, key: Key, mode: Mode) {
        {
            let mut entries = self.entries();
            let entry = entries.entry(key).or_default();
            if !entry.waiting.contains(&holder.id) {
                entry.waiting.push_back(holder.id);
            }
        }
        let mut warned = false;
        loop {
            let mut changed = pin!(self.changed.notified());
            changed.as_mut().enable();
            if self.is_turn_of(holder.id, key, mode) {
                return;
            }
            if warned {
                changed.await;
            } else if time::timeout(LEAVE_GRACE, changed).await.is_err() {
                warned = true;
                warn!(
                    "{} waits for {} longer than {:?}, held by {}",
                    holder.name,
                    key,
                    LEAVE_GRACE,
                    self.holders_of(key).join(", ")
                );
            }
        }
    }

    /// Whether `id` is the first in the queue of `key`, and the key admits it as `mode` asks.
    fn is_turn_of(&self, id: ProcedureId, key: Key, mode: Mode) -> bool {
        self.entries().get(&key).is_none_or(|entry| {
            entry.waiting.front().is_none_or(|waiter| *waiter == id) && entry.admits(id, mode)
        })
    }

    /// The names of the procedures that hold `key`, for the log.
    fn holders_of(&self, key: Key) -> Vec<&'static str> {
        self.entries()
            .get(&key)
            .map(|entry| entry.holders.values().map(|(_, name)| *name).collect())
            .unwrap_or_default()
    }

    /// Whether `key` is in the table: held or waited for.
    #[cfg(test)]
    pub(super) fn knows(&self, key: Key) -> bool {
        self.entries().contains_key(&key)
    }
}

/// That a procedure found an object locked and is deferred: returned by `Locks`, and passed on by
/// `Procedure::enumerate`, whose executor then lets go of everything it holds, waits for the object
/// and has it enumerate again.
#[derive(Debug)]
pub struct Deferred {
    key: Key,
    mode: Mode,
}

/// The locks of one procedure: through them it locks the objects it enumerates, and the executor
/// lets go of them all once the procedure is done, deferred or given up.
#[derive(Debug)]
pub struct Locks<'t> {
    table: &'t Table,
    holder: Holder,
    held: HashSet<Key>,
    /// The key at which it waited, where it keeps its place until it locks it.
    queued: Option<Key>,
    /// What deferred it, once a lock conflicted: it locks nothing more until it enumerates again.
    deferred: Option<(Key, Mode)>,
}

impl<'t> Locks<'t> {
    /// The locks of `holder` in `table`, none held yet.
    pub(super) fn new(table: &'t Table, holder: Holder) -> Self {
        Self { table, holder, held: HashSet::new(), queued: None, deferred: None }
    }

    /// Locks `key` shared - it stays as it is while the procedure holds it -, and its parents.
    ///
    /// # Errors
    /// `Deferred` if it or one of its parents is held exclusively by another, or waited for.
    pub fn shared(&mut self, key: Key) -> Result<(), Deferred> {
        self.lock(key, Mode::Shared)
    }

    /// Locks `key` exclusively - the procedure alone changes it while it holds it -, and its
    /// parents shared.
    ///
    /// # Errors
    /// `Deferred` if another holds it, or one of its parents exclusively, or waits for one.
    pub fn exclusive(&mut self, key: Key) -> Result<(), Deferred> {
        self.lock(key, Mode::Exclusive)
    }

    fn lock(&mut self, key: Key, mode: Mode) -> Result<(), Deferred> {
        if let Some((key, mode)) = self.deferred {
            return Err(Deferred { key, mode });
        }
        let path = key.path();
        let last = path.len() - 1;
        for (at, step) in path.into_iter().enumerate() {
            let mode = if at == last { mode } else { Mode::Shared };
            if !self.table.try_lock(self.holder, step, mode) {
                self.deferred = Some((step, mode));
                return Err(Deferred { key: step, mode });
            }
            self.held.insert(step);
        }
        Ok(())
    }

    /// What the procedure `enumerated`, unless a lock conflicted as it did - also where it did not
    /// pass the conflict on: then what deferred it.
    pub(super) fn checked<T>(&mut self, enumerated: Result<T, Deferred>) -> Result<T, Deferred> {
        match self.deferred.take() {
            Some((key, mode)) => Err(Deferred { key, mode }),
            None => enumerated,
        }
    }

    /// Lets go of everything, and waits at the key `deferred` names until it is its turn there. Its
    /// place there is known before it waits, so that a wait given up - dropped - leaves the queue
    /// too.
    pub(super) async fn wait_for(&mut self, deferred: Deferred) {
        self.release();
        let Deferred { key, mode } = deferred;
        if let Some(queued) = self.queued.replace(key).filter(|queued| *queued != key) {
            self.table.leave_queue(self.holder.id, queued);
        }
        self.table.wait(self.holder, key, mode).await;
    }

    /// Gives up the place it kept in a queue, once it enumerated without locking that key.
    pub(super) fn settle(&mut self) {
        if let Some(queued) = self.queued.take() {
            self.table.leave_queue(self.holder.id, queued);
        }
    }

    /// Lets go of every key it holds.
    fn release(&mut self) {
        self.table.release(self.holder.id, self.held.drain());
    }
}

impl Drop for Locks<'_> {
    fn drop(&mut self) {
        self.settle();
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::{Holder, Locks, Table};
    use crate::procedure::Key;
    use caelumora_domain::session::SessionId;
    use std::time::Duration;
    use tokio::time;

    /// A wait given up - its future dropped, as a shutdown whose bound passed drops it - leaves
    /// the key's queue, so that the key admits whoever comes next.
    #[tokio::test]
    async fn a_wait_given_up_leaves_the_queue() {
        let table = Table::default();
        let key = Key::Session(SessionId::new());
        let mut holding = Locks::new(&table, Holder { id: 1, name: "holding" });
        holding.exclusive(key).expect("free");
        let mut waiting = Locks::new(&table, Holder { id: 2, name: "waiting" });
        let deferred = waiting.exclusive(key).expect_err("held by the other");
        let given_up = time::timeout(Duration::from_millis(20), waiting.wait_for(deferred)).await;
        assert!(given_up.is_err(), "it waited until it gave up");
        drop(waiting);
        drop(holding);
        assert!(!table.knows(key), "nobody holds or waits for it");
        let mut next = Locks::new(&table, Holder { id: 3, name: "next" });
        next.exclusive(key).expect("admitted");
    }
}
