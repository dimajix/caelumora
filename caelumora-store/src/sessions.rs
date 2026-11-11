//! What holds a session for the process it runs in, against the other processes that share the
//! store (`SessionLocks`, issue 554): a session runs in one process, and no other starts, deletes,
//! restores or purges it meanwhile.

use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResult;
use std::fmt::Debug;

/// That a session is held for this process (issue 554), for as long as it is kept: dropping it lets
/// the session go.
pub struct SessionHold {
    /// What its backend keeps while it holds the session, let go as it is dropped.
    _held: Box<dyn Send + Sync>,
}

impl SessionHold {
    /// The hold kept by `held`, whatever its backend keeps while it holds a session.
    pub fn new(held: impl Send + Sync + 'static) -> Self {
        Self { _held: Box::new(held) }
    }
}

/// Holds sessions against the other processes that share the store (issue 554).
pub trait SessionLocks: Debug + Send + Sync {
    /// Holds session `id` for this process; `None` if another process holds it.
    ///
    /// # Errors
    /// If what holds it cannot be made or asked.
    fn try_hold(&self, id: SessionId) -> StoreResult<Option<SessionHold>>;

    /// Removes what holding session `id` keeps, once the session is purged, while the caller holds
    /// it: nothing of a session purged stays behind.
    ///
    /// # Errors
    /// If what holds it cannot be removed.
    fn forget(&self, id: SessionId) -> StoreResult<()>;
}
