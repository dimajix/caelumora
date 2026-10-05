//! Deleting marks (issue 164): what a user deletes stays stored with a
//! `Removal` - who and when - and is hidden from every read of the stores
//! except those that ask for what was deleted. Removing it for good is a
//! purge, an administrator's act or a retention policy's (issue 07). A
//! participant that leaves a channel is marked the same way.

use crate::actor::ActorId;
use crate::security::Secured;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// That something was deleted, or left: by whom - the caller of the act,
/// as the log has it - and when. The entity's state, not the record of the
/// act, which is the audit log's (issue 140): it is gone again when the
/// entity is restored or purged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Removal {
    pub by: ActorId,
    pub at: DateTime<Utc>,
}

impl Removal {
    /// Removed by `by`, now.
    pub fn now(by: ActorId) -> Self {
        Self { by, at: Utc::now() }
    }
}

/// An entity the `EntityStore` keeps: what the stores are generic over,
/// besides `Secured`. Whether it was deleted is asked of every kind alike,
/// so that every kind is read the same way; only a `Deletable` kind can be.
pub trait Stored: Secured {
    /// Whether, and by whom and when, it was deleted; never for a kind that
    /// is not `Deletable`.
    fn deleted(&self) -> Option<&Removal> {
        None
    }
}

/// A stored kind that deleting marks rather than removes (issue 164): what
/// holds content, an identity, or a definition that is managed - sessions,
/// channels, agents, users and their profiles, personalities and session
/// templates. Not a preconfigured component: it is reached only through the
/// personalities and templates that refer to it, and hidden with them.
pub trait Deletable: Stored {
    /// Marks it deleted, or with `None` takes the mark away.
    fn set_deleted(&mut self, deleted: Option<Removal>);
}

/// Implements `Stored` and `Deletable` for types with a field
/// `deleted: Option<Removal>`.
#[macro_export]
macro_rules! impl_deletable {
    ($($t:ty),+ $(,)?) => {$(
        impl $crate::removal::Stored for $t {
            fn deleted(&self) -> Option<&$crate::removal::Removal> {
                self.deleted.as_ref()
            }
        }
        impl $crate::removal::Deletable for $t {
            fn set_deleted(&mut self, deleted: Option<$crate::removal::Removal>) {
                self.deleted = deleted;
            }
        }
    )+};
}
