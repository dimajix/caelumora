//! Deleting marks (issue 164): what a user deletes stays stored with a `Removal` - which deletion,
//! who and when - and is hidden from every read of the stores except those that ask for what was
//! deleted. Removing it for good is a purge, an administrator's act or a retention policy's (issue
//! 07): an act of its own, which the entity records apart from the mark (`Stored::purging`). A
//! participant that leaves a channel is marked the same way.

use crate::actor::ActorId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

define_id!(RemovalId);

/// That something was deleted, or left: in which deletion, by whom - the caller of the act, as the
/// log has it - and when. The entity's state, not the record of the act, which is the audit log's
/// (issue 140): it is gone again when the entity is restored or purged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removal {
    /// The deletion: made once with the mark and given to everything deleted with it, so that what
    /// went with a deletion is told by it - not by the time, which not every backend keeps exactly
    /// (issue 478).
    pub id: RemovalId,
    pub by: ActorId,
    pub at: DateTime<Utc>,
}

impl Removal {
    /// A new deletion, by `by`, now.
    pub fn now(by: ActorId) -> Self {
        Self { id: RemovalId::new(), by, at: Utc::now() }
    }
}
