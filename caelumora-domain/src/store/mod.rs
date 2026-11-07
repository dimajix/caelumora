//! What the domain says of being stored (issues 93, 323, 523): what an entity is to be stored
//! (`Stored`) - its kind (`EntityKind`), its revision (`Revision`), what encloses it
//! (`Enclosing`) -, what a state the stores keep tells of its changes (`Journaled`), and how a
//! store fails (`StoreError`). What a query of a store asks is `query`'s, what a page of a history
//! is filtered by is `history`'s; the stores themselves are `caelumora-store`'s (issue 517).

#[cfg(test)]
pub(crate) mod testing;

use crate::confidentiality::Classification;
use crate::intern::intern;
use crate::query::Field;
use crate::removal::Removal;
use crate::security::{Refused, Secured};
use crate::value::name::ScopedName;
use crate::value::ValueError;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },
    /// An id that is taken - of an entity, or the name of a kind (issue
    /// 323).
    #[error("conflict: {0}")]
    Conflict(String),
    /// A name that another of the kind has in its name space (issue 250):
    /// creating, renaming or restoring one under it is refused.
    #[error("the name is taken by another {entity}")]
    NameTaken { entity: &'static str },
    #[error("validation: {0}")]
    Validation(String),
    /// A text that does not fit the field it was to be written into
    /// (issue 248): what the field is called and why.
    #[error("{0}")]
    Invalid(#[from] ValueError),
    #[error("backend: {0}")]
    Backend(String),
    /// Refused (issue 323): what was refused is its source.
    #[error("permission denied")]
    PermissionDenied(#[source] Refused),
    /// Written on a revision that is no longer the stored one (issue 492): something changed
    /// it meanwhile, or purged it. Whoever wrote reads it again and makes their change once
    /// more.
    #[error("the {entity} {id} was changed meanwhile")]
    Outdated { entity: &'static str, id: String },
}

impl StoreError {
    /// Whether it says that what was asked for is not there, or refused to the caller (`NotFound`,
    /// `PermissionDenied`), rather than that the store failed or the request did not fit.
    pub fn is_absent_or_refused(&self) -> bool {
        matches!(self, Self::NotFound { .. } | Self::PermissionDenied(_))
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// What a lookup answers when what it looks for may no longer exist (issue 192), and what a write
/// answers when it may have been refused as outdated (issue 492).
pub trait StoreResultExt<T> {
    /// What was found, `None` if it is not there (`StoreError::NotFound`);
    /// any other error stays one.
    fn optional(self) -> StoreResult<Option<T>>;

    /// What a write answered, `None` if it was refused as outdated (`StoreError::Outdated`):
    /// whoever wrote reads again and makes the change once more. Any other error stays one.
    fn written(self) -> StoreResult<Option<T>>;

    /// What a creation answered, `None` if the id was taken (`StoreError::Conflict`) - made
    /// meanwhile elsewhere, or there before, deleted or not. Any other error stays one.
    fn created(self) -> StoreResult<Option<T>>;
}

impl<T> StoreResultExt<T> for StoreResult<T> {
    fn optional(self) -> StoreResult<Option<T>> {
        match self {
            Ok(found) => Ok(Some(found)),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn written(self) -> StoreResult<Option<T>> {
        match self {
            Ok(written) => Ok(Some(written)),
            Err(StoreError::Outdated { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn created(self) -> StoreResult<Option<T>> {
        match self {
            Ok(created) => Ok(Some(created)),
            Err(StoreError::Conflict(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// A state the `StateStore` keeps (issue 193): it tells what changed since
/// it was last stored, as a sequence of changes, so that a store writes only
/// that, and replays such changes onto what it stored before. A state is
/// small parts and growing ones - chains, values kept by digest - and its
/// `Change` has a variant per part: a growing part's records what was
/// added, a small part's its whole value, which `changes` puts at the end of
/// every sequence. Order holds within a part; across parts it is the fixed
/// order of `changes`, which loses nothing, since every reference between
/// parts is a digest or an id, resolved when read and never while replaying.
pub trait Journaled: Serialize + DeserializeOwned + Send {
    type Change: Serialize + DeserializeOwned + Send;
    /// What changed since `mark_stored`, in order.
    fn changes(&self) -> Vec<Self::Change>;
    /// Forgets the changes: they are stored.
    fn mark_stored(&mut self);
    /// Replays `change`, as a store does when loading what it appended -
    /// idempotent, and no change to store again (`Chain::apply`).
    ///
    /// # Errors
    /// If the change does not fit the state.
    fn apply(&mut self, change: Self::Change) -> Result<(), String>;
}

/// A kind of entity the stores keep: what the actions that concern every
/// kind alike are keyed by, and its name in errors. Open
/// (issue 323): each stored type names its own (`Stored::KIND`), so that a
/// kind is added without a change here. Its name is unique among the kinds;
/// it is also what the JSON store keeps the kind's files under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityKind(&'static str);

impl EntityKind {
    /// The kind of `name`, written in `snake_case`.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// Its name, as errors give it.
    pub fn name(self) -> &'static str {
        self.0
    }

    /// Its name as a type's is written - `SessionTemplate` -, as
    /// `ActionType::describe` names what is done with it.
    pub fn title(self) -> String {
        self.name()
            .split('_')
            .map(|word| {
                let mut letters = word.chars();
                letters
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + letters.as_str())
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// Written as its name; read back as the kind of that name (`intern`), so
/// that an event from another instance (issue 122) names the kind this one
/// defines - or one nothing here waits for.
impl Serialize for EntityKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for EntityKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|name| Self(intern(&name)))
    }
}

impl Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The revision of a stored entity (issue 492): counted up by every write of it, so that a write
/// made on an outdated one is refused (`StoreError::Outdated`) instead of overwriting what came
/// between - by every storage, in the step that writes, so that it holds against every writer of
/// the storage, also of another process. A new entity carries the first, which no stored one has:
/// it is created as the one after it (issue 500), so that a copy never stored is never taken for
/// one read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision(u64);

impl Revision {
    /// The revision after this one: what a write of an entity at this one is written as.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Its number: the first, that of an entity never stored, is 0.
    pub fn number(self) -> u64 {
        self.0
    }
}

/// An entity the `EntityStore` keeps: what the stores are generic over, besides `Secured`. Every
/// kind is marked when it is deleted, not removed (issue 164): what holds content, an identity, or
/// a definition that is managed. A store keeps it as it is serialized, and hands out copies, so
/// that a backend can keep any kind (issue 323). Every kind carries its revision (issue 492), by
/// which a write of it is judged.
pub trait Stored: Secured + Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Its kind (issue 323), by which what is done with it is asked and
    /// named.
    const KIND: EntityKind;

    /// What it is kept and found by: a UUID under a type of its kind's
    /// (`define_id!`), so that every store keys every kind alike.
    type Id: Clone
        + Eq
        + Hash
        + Display
        + FromStr
        + Debug
        + Send
        + Sync
        + From<Uuid>
        + Into<Uuid>
        + 'static;

    /// The kind of entity it lies in - a channel in a `Session` -, or
    /// `NoEntity` for a kind that lies in none (issue 335). Its events name
    /// that entity beside its own id.
    type Within: Enclosing;

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

    /// Whether, and by whom and when, it was deleted (issue 164).
    fn deleted(&self) -> Option<&Removal>;

    /// Marks it deleted, or with `None` takes the mark away.
    fn set_deleted(&mut self, deleted: Option<Removal>);

    /// Its revision (issue 492): as it was read, what a write of it is judged against - the first
    /// if it was never stored -; once the store wrote it, the one it was written as.
    fn revision(&self) -> Revision;

    /// Sets its revision: the store's, as it writes it.
    fn set_revision(&mut self, revision: Revision);

    /// Whether its purge has begun (issue 492): what lies behind it is being removed, and no
    /// restoring takes it back any more. Begun today only for what was deleted, which then stays
    /// deleted until it is purged.
    fn purging(&self) -> bool;

    /// Marks its purge begun: the store's, as it begins it (`EntityStore::begin_purge`).
    fn set_purging(&mut self);

    /// Nothing if it may be stored as it is; why not otherwise, which the
    /// store answers as `StoreError::Validation` when it is created or
    /// changed.
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    /// How it is classified - a channel, a workspace, a skill, an artifact -,
    /// `None` for a kind that is not. Whoever reaches it through the store
    /// must be cleared for it (issue 322).
    fn classification(&self) -> Option<&Classification> {
        None
    }
}

/// A kind whose changes are told by an update event (issues 248, 358): the
/// event says what was asked, `apply` makes it into the kind's values - where
/// it was asked and wherever it is announced, so that every live copy changes
/// alike.
pub trait Updatable: Stored {
    /// What a change of it says, as it is asked and announced
    /// (`GlobalEvent::updated`).
    type Update: Serialize + DeserializeOwned + Send + Sync;

    /// Applies `update`: what it says is made into its values here.
    ///
    /// # Errors
    /// `ValueError` if what it says does not fit; it is then unchanged.
    fn apply(&mut self, update: &Self::Update) -> Result<(), ValueError>;

    /// Notes that it was changed `at`, for a kind that keeps when it was
    /// changed last; nothing for one that does not.
    fn touch(&mut self, _at: DateTime<Utc>) {}
}

/// What a kind can lie in (issue 335): the kind of entity that encloses
/// it, or `NoEntity`. It says how an entity with an id `I` is named where
/// it lies - its `Address` -, and how its events write that.
pub trait Enclosing {
    /// How an entity with the id `I` that lies in it is named: by the id of
    /// the entity it lies in and its own, or by its own alone where it lies
    /// in none.
    type Address<I>;

    /// `address` as an event writes it: the id of the entity it lies in, if
    /// any, and its own.
    fn write<I: Display>(address: &Self::Address<I>) -> (Option<String>, String);

    /// The address an event wrote as `within` and `id`; `None` if one of
    /// them is no id of its kind, or names an entity it cannot lie in.
    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<Self::Address<I>>;
}

/// Lying in an entity of a stored kind: named by its id and one's own.
impl<T: Stored> Enclosing for T {
    type Address<I> = (T::Id, I);

    fn write<I: Display>((within, id): &(T::Id, I)) -> (Option<String>, String) {
        (Some(within.to_string()), id.to_string())
    }

    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<(T::Id, I)> {
        Some((within?.parse().ok()?, id.parse().ok()?))
    }
}

/// What a kind that lies in no entity names as its `Stored::Within`: a
/// session, an agent, a user. There is none of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoEntity {}

/// Lying in nothing: named by one's own id alone.
impl Enclosing for NoEntity {
    type Address<I> = I;

    fn write<I: Display>(id: &I) -> (Option<String>, String) {
        (None, id.to_string())
    }

    fn read<I: FromStr>(within: Option<&str>, id: &str) -> Option<I> {
        match within {
            None => id.parse().ok(),
            Some(_) => None,
        }
    }
}

/// How an entity of the kind `T` is named where it lies (issue 335): its
/// id, or the id of the entity it lies in and its own - a channel's
/// `(SessionId, ChannelId)`.
pub type Address<T> = <<T as Stored>::Within as Enclosing>::Address<<T as Stored>::Id>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_not_found_is_none_and_every_other_error_stays_one() {
        let found: StoreResult<u32> = Ok(1);
        assert_eq!(found.optional().unwrap(), Some(1));
        let gone: StoreResult<u32> =
            Err(StoreError::NotFound { entity: "user", id: "1".to_string() });
        assert_eq!(gone.optional().unwrap(), None);
        let failed: StoreResult<u32> = Err(StoreError::Backend("disk".to_string()));
        assert!(matches!(failed.optional(), Err(StoreError::Backend(_))));
    }
}
