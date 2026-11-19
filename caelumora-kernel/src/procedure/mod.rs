//! Procedures (issue 593): every change of a lifecycle - a session or a project made, started,
//! stopped, deleted, restored, purged, someone taken into a session or let go from it - as one
//! procedure, which the `ProcedureExecutor` carries out while the procedure holds the objects it
//! changes.
//!
//! A procedure is first only asked for. Carried out in a task of the executor's, it enumerates
//! what it would change from the top down and locks it as it finds it (`Procedure::enumerate`):
//! exclusively what it changes, shared what only has to stay as it is - and the parents of every
//! key shared with it, which the key names by its path (`Key`). An object it finds locked defers
//! it: it lets go of everything, waits until that object is free and enumerates again. Holding its
//! objects, it checks the state it finds - permissions too, once - and carries the change out or
//! refuses it (`Procedure::carry_out`), recording how to undo each step (`Compensation`), which
//! the executor undoes, the last first, if a later step fails. Nothing waits while it holds
//! anything, so no order of locks is needed; a key at which a deferred procedure waits admits
//! nobody after it, so that a stop is not starved by a stream of small changes.
//!
//! A procedure keeps its place in the queue of one key alone: deferred at another, it waits there
//! anew, at the end. One that locks many keys under steady traffic on them can so be held back
//! long - fair for each key, not across keys.
//!
//! No procedure is carried out within another: it would wait for the keys the other holds, and so
//! for itself; the executor refuses it (`ProcedureError::Nested`). Code procedures share is shared
//! as functions. What changes an object asks for the proof that a procedure holds its objects
//! (`Admitted`), which the executor alone gives out; the objects themselves know nothing of
//! locking.

mod compensation;
mod executor;
pub mod follower;
mod locks;
pub mod participant;
pub mod project;
pub mod session;
#[cfg(test)]
pub(crate) mod testing;

pub use compensation::Compensation;
pub use executor::ProcedureExecutor;
pub use locks::{Admitted, Deferred, Locks};

use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, ThreadId};
use caelumora_domain::project::ProjectId;
use caelumora_domain::session::SessionId;
use std::fmt;

/// An object of the logical graph a procedure locks, named by its path from the top: locking it
/// locks its parents shared (`Key::parent`). A project stands apart from the path, since a session
/// is put into a shared project and taken out again: a procedure locks it where it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A session, with everything in it.
    Session(SessionId),
    /// A project.
    Project(ProjectId),
    /// A channel of a session.
    Channel(SessionId, ChannelId),
    /// A thread of a channel.
    Thread(SessionId, ChannelId, ThreadId),
    /// The part of an actor in a session and in every channel of it.
    Participant(SessionId, ActorId),
}

impl Key {
    /// The key it hangs under; `None` for a session or a project.
    fn parent(&self) -> Option<Key> {
        match *self {
            Key::Session(_) | Key::Project(_) => None,
            Key::Channel(session, _) | Key::Participant(session, _) => Some(Key::Session(session)),
            Key::Thread(session, channel, _) => Some(Key::Channel(session, channel)),
        }
    }

    /// Its parents and itself, from the top down.
    fn path(&self) -> Vec<Key> {
        let mut path = vec![*self];
        while let Some(parent) = path.last().and_then(Key::parent) {
            path.push(parent);
        }
        path.reverse();
        path
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Session(session) => write!(f, "session {session}"),
            Key::Project(project) => write!(f, "project {project}"),
            Key::Channel(session, channel) => write!(f, "channel {session}/{channel}"),
            Key::Thread(session, channel, thread) => {
                write!(f, "thread {session}/{channel}/{thread}")
            }
            Key::Participant(session, actor) => write!(f, "participant {actor} of {session}"),
        }
    }
}

/// One change of a lifecycle across the objects it touches, carried out by the
/// `ProcedureExecutor` in a task of its own.
#[async_trait]
pub trait Procedure: Send + Sync + 'static {
    /// What enumerating found and carrying the procedure out needs - what reading failed at too.
    type Plan: Send;
    /// What it answers its caller.
    type Output: Send + 'static;
    /// What it fails with - and what the executor fails with.
    type Error: From<ProcedureError> + Send + 'static;

    /// Finds from the top down what it would change and locks it as it finds it; reads and locks,
    /// nothing else. Called again after every conflict, which defers it.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred>;

    /// Holding its objects, checks the state it finds - permissions too - and carries the change
    /// out or refuses it, recording in `log` how to undo each step. One that panics is not undone:
    /// its locks are let go and its caller is answered `ProcedureError::Ended`, but what it did
    /// stays - which is why no procedure unwraps.
    async fn carry_out(
        self,
        plan: Self::Plan,
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<Self::Output, Self::Error>;
}

/// What the executor fails with instead of carrying a procedure out.
#[derive(Debug, thiserror::Error)]
pub enum ProcedureError {
    /// Asked for in carrying out another, which holds the keys it would wait for.
    #[error("A procedure is not carried out within another")]
    Nested,
    /// The executor admits no procedure any more: the process is ending.
    #[error("Caelumora is shutting down: nothing is started or changed any more")]
    ShuttingDown,
    /// Its task ended without an answer; the log tells why.
    #[error("The procedure ended without an answer")]
    Ended,
}
