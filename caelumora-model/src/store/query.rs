//! Queries of a store's lists (issue 293): a declarative condition, a
//! limit and a cursor, which a store evaluates in memory and a database could
//! push into a `WHERE` clause. A condition names fields of a kind - each a
//! path into the document as it is serialized - and compares them with JSON
//! values, so that it reads the same in memory, in a JSON file and in a
//! `jsonb` column. Who may read what is still asked afterwards, entity by
//! entity; a limit is met by reading on.

use crate::store::{StoreError, StoreResult};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Debug;

/// A field of a kind that a query may name: where it lies in the document
/// as it is serialized. A field used inside `Filter::Any` lies within an
/// element of the collection, its path relative to it.
pub trait Field: Copy + Debug + Send + Sync + 'static {
    fn path(self) -> &'static [&'static str];
}

/// The fields of a kind nothing queries yet: none. Such a kind is listed by
/// limit and cursor alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoField {}

impl Field for NoField {
    fn path(self) -> &'static [&'static str] {
        match self {}
    }
}

/// A condition on the documents of a kind. Purely declarative: equality,
/// membership, absence, the boolean operators, and whether any element of
/// a collection meets a condition of its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter<F: Field> {
    Eq(F, Value),
    In(F, Vec<Value>),
    /// The field is absent or `null`.
    IsNull(F),
    And(Vec<Filter<F>>),
    Or(Vec<Filter<F>>),
    Not(Box<Filter<F>>),
    /// Some element of the collection `F` meets the condition, whose fields
    /// lie within the element.
    Any(F, Box<Filter<F>>),
}

impl<F: Field> Filter<F> {
    /// `field` equals `value` as it is serialized.
    ///
    /// # Errors
    /// `StoreError::Validation` if `value` cannot be serialized.
    pub fn eq<T: Serialize + ?Sized>(field: F, value: &T) -> StoreResult<Self> {
        Ok(Filter::Eq(field, json(value)?))
    }

    /// `field` equals one of `values` as they are serialized.
    ///
    /// # Errors
    /// `StoreError::Validation` if one of `values` cannot be serialized.
    pub fn is_in<T: Serialize>(field: F, values: &[T]) -> StoreResult<Self> {
        Ok(Filter::In(field, values.iter().map(json).collect::<StoreResult<_>>()?))
    }

    /// Some element of `collection` meets `condition`.
    pub fn any(collection: F, condition: Filter<F>) -> Self {
        Filter::Any(collection, Box::new(condition))
    }

    /// Whether `document` - an entity as it is serialized, or an element of
    /// a collection in it - meets the condition.
    pub fn matches(&self, document: &Value) -> bool {
        match self {
            Filter::Eq(field, value) => lookup(document, *field) == Some(value),
            Filter::In(field, values) => lookup(document, *field).is_some_and(|found| values.contains(found)),
            Filter::IsNull(field) => lookup(document, *field).is_none_or(Value::is_null),
            Filter::And(conditions) => conditions.iter().all(|c| c.matches(document)),
            Filter::Or(conditions) => conditions.iter().any(|c| c.matches(document)),
            Filter::Not(condition) => !condition.matches(document),
            Filter::Any(collection, condition) => match lookup(document, *collection) {
                Some(Value::Array(elements)) => elements.iter().any(|element| condition.matches(element)),
                _ => false,
            },
        }
    }
}

/// `value` as a query compares it.
fn json<T: Serialize + ?Sized>(value: &T) -> StoreResult<Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Validation(format!("a value of a query cannot be serialized: {e}")))
}

/// What lies at `field` in `document`, if anything.
fn lookup(document: &Value, field: impl Field) -> Option<&Value> {
    field.path().iter().try_fold(document, |at, key| at.get(key))
}

/// Where a list goes on: after the entity it names, in the order of their
/// keys. Opaque to whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor(String);

impl Cursor {
    /// The cursor after the entity of `key`.
    pub fn after(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The cursor as it is handed out, to be given back by `Cursor::parse`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A cursor handed out before.
    pub fn parse(text: &str) -> Self {
        Self(text.to_string())
    }

    pub(crate) fn key(&self) -> &str {
        &self.0
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

/// `candidates` in the order of their keys, from after the cursor of
/// `query` on - what a list goes through, one by one, until its limit.
/// Each key comes with what the entity is found by.
pub(crate) fn ordered_after<T, F: Field>(mut candidates: Vec<(String, T)>, query: &Query<F>) -> impl Iterator<Item = (String, T)> {
    candidates.sort_by(|(l, _), (r, _)| l.cmp(r));
    let start = match &query.after {
        Some(cursor) => candidates.partition_point(|(key, _)| key.as_str() <= cursor.key()),
        None => 0,
    };
    candidates.into_iter().skip(start)
}

/// `value`, if it meets `filter`, judged on it serialized - for a store
/// that holds its entities as values, not as documents.
///
/// # Errors
/// `StoreError::Validation` if `value` cannot be serialized.
pub(crate) fn meeting<T: Serialize, F: Field>(value: T, filter: Option<&Filter<F>>) -> StoreResult<Option<T>> {
    match filter {
        None => Ok(Some(value)),
        Some(filter) => Ok(filter.matches(&json(&value)?).then_some(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Doc {
        Owner,
        Tags,
        Tag,
        Missing,
    }

    impl Field for Doc {
        fn path(self) -> &'static [&'static str] {
            match self {
                Doc::Owner => &["owner", "id"],
                Doc::Tags => &["tags"],
                Doc::Tag => &["name"],
                Doc::Missing => &["missing"],
            }
        }
    }

    #[test]
    fn a_filter_reads_a_document_by_paths_and_json_values() {
        let document = json!({ "owner": { "id": "alice" }, "tags": [{ "name": "a" }, { "name": "b" }] });
        assert!(Filter::eq(Doc::Owner, "alice").unwrap().matches(&document));
        assert!(!Filter::eq(Doc::Owner, "bob").unwrap().matches(&document));
        assert!(Filter::is_in(Doc::Owner, &["bob", "alice"]).unwrap().matches(&document));
        assert!(Filter::IsNull(Doc::Missing).matches(&document) && !Filter::IsNull(Doc::Owner).matches(&document));
        assert!(Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "b").unwrap()).matches(&document), "an element of the collection");
        assert!(!Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "c").unwrap()).matches(&document));
        assert!(!Filter::any(Doc::Owner, Filter::eq(Doc::Tag, "a").unwrap()).matches(&document), "what is no collection has no element");
        let both = Filter::And(vec![Filter::eq(Doc::Owner, "alice").unwrap(), Filter::Not(Box::new(Filter::IsNull(Doc::Tags)))]);
        assert!(both.matches(&document));
        assert!(Filter::Or(vec![Filter::eq(Doc::Owner, "bob").unwrap(), Filter::IsNull(Doc::Missing)]).matches(&document));
    }

    #[test]
    fn candidates_are_gone_through_in_the_order_of_keys_after_the_cursor() {
        let candidates: Vec<(String, u32)> = vec![("c".into(), 3), ("a".into(), 1), ("b".into(), 2)];
        let all: Vec<u32> = ordered_after(candidates.clone(), &Query::<NoField>::all()).map(|(_, n)| n).collect();
        assert_eq!(all, [1, 2, 3]);
        let after: Vec<u32> = ordered_after(candidates, &Query::<NoField>::all().after(Cursor::after("a"))).map(|(_, n)| n).collect();
        assert_eq!(after, [2, 3]);
    }
}
