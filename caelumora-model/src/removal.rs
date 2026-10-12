//! Deleting marks (issue 164): what a user deletes stays stored with a
//! `Removal` - who and when - and is hidden from every read of the stores
//! except those that ask for what was deleted. Removing it for good is a
//! purge, an administrator's act or a retention policy's (issue 07). A
//! participant that leaves a channel is marked the same way.

use crate::actor::ActorId;
use crate::confidentiality::Classification;
use crate::security::Secured;
use crate::store::EntityKind;
use crate::store::query::Field;
use crate::value::name::ScopedName;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::str::FromStr;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
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
/// A store keeps it as it is serialized, and hands out copies, so that a
/// backend can keep any kind (issue 323).
pub trait Stored: Secured + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its kind (issue 323), by which what is done with it is asked and
    /// named.
    const KIND: EntityKind;

    /// What it is kept and found by.
    type Id: Clone + Eq + Hash + Display + FromStr + Debug + Send + Sync + 'static;

    /// The fields a query of its kind names (issue 293).
    type Field: Field;

    /// Its id.
    fn id(&self) -> Self::Id;

    /// Its name and the scope it lies in, if its kind is `Scoped` - a name
    /// unique of its kind in that scope, which the store keeps so (issue
    /// 250) -; `None` for a kind without one.
    fn unique_name(&self) -> Option<ScopedName> {
        None
    }

    /// Whether, and by whom and when, it was deleted; never for a kind that
    /// is not `Deletable`.
    fn deleted(&self) -> Option<&Removal> {
        None
    }

    /// Nothing if it may be stored as it is; why not otherwise, which the
    /// store answers as `StoreError::Validation` when it is created or
    /// changed.
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// How it is classified - a channel, a workspace, a skill, an artifact
    /// -, `None` for a kind that is not. Whoever reaches it through the store
    /// must be cleared for it (issue 322).
    fn classification(&self) -> Option<&Classification> {
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
