//! Queries of a store's lists (issue 293): a declarative condition, a
//! limit and a cursor, which a store evaluates in memory and a database could
//! push into a `WHERE` clause. A condition names fields of a kind - each a
//! path into the document as it is serialized - and compares them with
//! values of the forms serde writes (`FieldValue`), so that it reads the same
//! in memory, in a JSON file and in a `jsonb` column. Who may read what is
//! still asked afterwards, entity by entity; a limit is met by reading on.
//!
//! What a condition names and compares is `field`'s, the condition and judging a value by it
//! `filter`'s (`Filter`, `meeting`); here the question of a list, its answer and the order a list
//! goes on in (`Query`, `Cursor`, `Page`, `ordered_after`).

mod field;
mod filter;
#[cfg(test)]
pub(crate) mod testing;

pub use field::{Field, FieldPath, FieldValue, NoField};
pub use filter::{meeting, Filter};

use uuid::Uuid;

/// Where a list goes on: after the entity it names, in the order of their
/// keys. Opaque to whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor(Uuid);

impl Cursor {
    /// The cursor after the entity `id`.
    pub fn after(id: Uuid) -> Self {
        Self(id)
    }

    /// The id of the entity it goes on after.
    pub fn id(&self) -> Uuid {
        self.0
    }
}

/// A question of a list: which of the entities a caller may read, how many
/// at most, from where on - in the order of their keys, their ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Query<F: Field> {
    pub filter: Option<Filter<F>>,
    pub limit: Option<usize>,
    pub after: Option<Cursor>,
}

impl<F: Field> Query<F> {
    /// Every entity the caller may read.
    pub fn all() -> Self {
        Self { filter: None, limit: None, after: None }
    }

    /// Those that meet `filter`.
    pub fn filtered(filter: Filter<F>) -> Self {
        Self { filter: Some(filter), ..Self::all() }
    }

    /// At most `limit` of them.
    pub fn limited(self, limit: usize) -> Self {
        Self { limit: Some(limit), ..self }
    }

    /// Those after `cursor`.
    pub fn after(self, cursor: Cursor) -> Self {
        Self { after: Some(cursor), ..self }
    }
}

/// What a list answers: the entities, and where it goes on if a limit cut
/// it short. A page that goes on is full; the one after it may be empty,
/// where nothing that followed was the caller's to read - finding that out
/// beforehand would cost a policy question for every entity left.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Cursor>,
}

/// `candidates` in the order of their ids, from after the id `after` on:
/// what a storage that holds its documents in no order hands out in a scan
/// (`DocumentStorage::scan`). Each id comes with what the document is found
/// by.
pub fn ordered_after<T>(
    mut candidates: Vec<(Uuid, T)>,
    after: Option<Uuid>,
) -> impl Iterator<Item = (Uuid, T)> {
    candidates.sort_by_key(|(id, _)| *id);
    let start = match after {
        Some(after) => candidates.partition_point(|(id, _)| *id <= after),
        None => 0,
    };
    candidates.into_iter().skip(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_gone_through_in_the_order_of_ids_after_the_cursor() {
        let id = Uuid::from_u128;
        let candidates: Vec<(Uuid, u32)> = vec![(id(3), 3), (id(1), 1), (id(2), 2)];
        let all: Vec<u32> = ordered_after(candidates.clone(), None).map(|(_, n)| n).collect();
        assert_eq!(all, [1, 2, 3]);
        let after: Vec<u32> = ordered_after(candidates, Some(id(1))).map(|(_, n)| n).collect();
        assert_eq!(after, [2, 3]);
    }
}
