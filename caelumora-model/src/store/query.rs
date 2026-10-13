//! Queries of a store's lists (issue 293): a declarative condition, a
//! limit and a cursor, which a store evaluates in memory and a database could
//! push into a `WHERE` clause. A condition names fields of a kind - each a
//! path into the document as it is serialized - and compares them with
//! values of the forms serde writes (`FieldValue`), so that it reads the same
//! in memory, in a JSON file and in a `jsonb` column. Who may read what is
//! still asked afterwards, entity by entity; a limit is met by reading on.

use crate::store::{StoreError, StoreResult};
use crate::value::name::fold;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Debug;
use uuid::Uuid;

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

/// What a filter compares a field with, in the forms serde writes - made
/// without serde, so that making a filter cannot fail. Generic: every id is
/// a `Uuid`, and a type whose serialized form is an enum's converts by
/// `From`, written after serde's tags.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// An id, as serde writes it: a string holding the UUID.
    Uuid(Uuid),
    /// A string - a unit variant as serde writes it, `"GlobalScope"`.
    Text(String),
    /// A variant as serde writes it, `{"User": "…"}`: an object that has
    /// the key, with a value meeting the inner one. It need not have only
    /// that key - containment, as `@>` in Postgres -, so that a value names
    /// only what identifies it: a channel's scope its channel alone.
    Tagged(&'static str, Box<FieldValue>),
    /// The variant `tag`, whatever it holds: as serde writes one that holds
    /// nothing, `"GlobalScope"`, or one that does, an object that has the
    /// key - as `?` in Postgres (issue 317).
    Variant(&'static str),
}

impl FieldValue {
    /// The variant `tag` holding `value`.
    pub fn tagged(tag: &'static str, value: impl Into<FieldValue>) -> Self {
        FieldValue::Tagged(tag, Box::new(value.into()))
    }

    /// The variant `tag`, whatever it holds.
    pub fn variant(tag: &'static str) -> Self {
        FieldValue::Variant(tag)
    }

    /// Whether `found`, what lies at a field of a document, meets this value.
    pub fn matches(&self, found: &Value) -> bool {
        match self {
            FieldValue::Uuid(id) => found.as_str().and_then(|text| Uuid::parse_str(text).ok()) == Some(*id),
            FieldValue::Text(text) => found.as_str() == Some(text.as_str()),
            FieldValue::Tagged(tag, value) => found.get(tag).is_some_and(|inner| value.matches(inner)),
            FieldValue::Variant(tag) => found.as_str() == Some(tag) || found.get(tag).is_some(),
        }
    }
}

impl From<Uuid> for FieldValue {
    fn from(id: Uuid) -> Self {
        FieldValue::Uuid(id)
    }
}

impl From<&str> for FieldValue {
    fn from(text: &str) -> Self {
        FieldValue::Text(text.to_string())
    }
}

/// A condition on the documents of a kind. Purely declarative: equality,
/// membership, absence, the boolean operators, and whether any element of
/// a collection meets a condition of its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter<F: Field> {
    Eq(F, FieldValue),
    In(F, Vec<FieldValue>),
    /// The field is absent or `null`.
    IsNull(F),
    And(Vec<Filter<F>>),
    Or(Vec<Filter<F>>),
    Not(Box<Filter<F>>),
    /// Some element of the collection `F` meets the condition, whose fields
    /// lie within the element.
    Any(F, Box<Filter<F>>),
    /// The field, a name, begins with the text as names are compared -
    /// regardless of case, accents and runs of whitespace (`Name`): a search
    /// by what one has typed so far (issue 221). A database compares the
    /// folded forms with `LIKE`.
    NamePrefix(F, String),
}

impl<F: Field> Filter<F> {
    /// `field` meets `value`.
    pub fn eq(field: F, value: impl Into<FieldValue>) -> Self {
        Filter::Eq(field, value.into())
    }

    /// `field` meets one of `values`.
    pub fn is_in<V: Into<FieldValue>>(field: F, values: impl IntoIterator<Item = V>) -> Self {
        Filter::In(field, values.into_iter().map(Into::into).collect())
    }

    /// Some element of `collection` meets `condition`.
    pub fn any(collection: F, condition: Filter<F>) -> Self {
        Filter::Any(collection, Box::new(condition))
    }

    /// `field`, a name, begins with `prefix` as names are compared.
    pub fn name_prefix(field: F, prefix: &str) -> Self {
        Filter::NamePrefix(field, prefix.to_string())
    }

    /// Whether `document` - an entity as it is serialized, or an element of
    /// a collection in it - meets the condition.
    pub fn matches(&self, document: &Value) -> bool {
        match self {
            Filter::Eq(field, value) => lookup(document, *field).is_some_and(|found| value.matches(found)),
            Filter::In(field, values) => lookup(document, *field).is_some_and(|found| values.iter().any(|value| value.matches(found))),
            Filter::IsNull(field) => lookup(document, *field).is_none_or(Value::is_null),
            Filter::And(conditions) => conditions.iter().all(|c| c.matches(document)),
            Filter::Or(conditions) => conditions.iter().any(|c| c.matches(document)),
            Filter::Not(condition) => !condition.matches(document),
            Filter::Any(collection, condition) => match lookup(document, *collection) {
                Some(Value::Array(elements)) => elements.iter().any(|element| condition.matches(element)),
                _ => false,
            },
            Filter::NamePrefix(field, prefix) => lookup(document, *field).and_then(Value::as_str).is_some_and(|name| fold(name).starts_with(&fold(prefix))),
        }
    }
}

/// For the tests of a type that converts into a `FieldValue`: each of
/// `samples`, converted, meets exactly the serialized samples equal to it -
/// which holds the conversion, written by hand, to serde's tags.
#[cfg(test)]
pub(crate) fn meets_exactly_its_equals<T>(samples: &[T])
where T: Clone + PartialEq + Debug + Serialize + Into<FieldValue> {
    for value in samples {
        let converted: FieldValue = value.clone().into();
        for other in samples {
            let serialized = serde_json::to_value(other).expect("a sample serializes");
            assert_eq!(converted.matches(&serialized), value == other, "{value:?} against {other:?}");
        }
    }
}

/// `value` serialized, for a filter to be judged on.
fn json<T: Serialize + ?Sized>(value: &T) -> StoreResult<Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Validation(format!("an entity cannot be serialized for a query: {e}")))
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
    fn a_name_prefix_is_compared_as_names_are() {
        let named = |name: &str| json!({ "owner": { "id": name } });
        let starting = |prefix: &str| Filter::name_prefix(Doc::Owner, prefix);
        assert!(starting("zo").matches(&named("Zoë Smith")), "regardless of case and accents");
        assert!(starting("ZOE  S").matches(&named("Zoë Smith")), "and of runs of whitespace");
        assert!(!starting("smith").matches(&named("Zoë Smith")), "from its beginning");
        assert!(!starting("zo").matches(&json!({ "owner": { "id": 1 } })), "a name only");
        assert!(!Filter::name_prefix(Doc::Missing, "").matches(&named("x")), "where there is one");
    }

    #[test]
    fn a_filter_reads_a_document_by_paths() {
        let document = json!({ "owner": { "id": "alice" }, "tags": [{ "name": "a" }, { "name": "b" }] });
        assert!(Filter::eq(Doc::Owner, "alice").matches(&document));
        assert!(!Filter::eq(Doc::Owner, "bob").matches(&document));
        assert!(Filter::is_in(Doc::Owner, ["bob", "alice"]).matches(&document));
        assert!(Filter::IsNull(Doc::Missing).matches(&document) && !Filter::IsNull(Doc::Owner).matches(&document));
        assert!(Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "b")).matches(&document), "an element of the collection");
        assert!(!Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "c")).matches(&document));
        assert!(!Filter::any(Doc::Owner, Filter::eq(Doc::Tag, "a")).matches(&document), "what is no collection has no element");
        let both = Filter::And(vec![Filter::eq(Doc::Owner, "alice"), Filter::Not(Box::new(Filter::IsNull(Doc::Tags)))]);
        assert!(both.matches(&document));
        assert!(Filter::Or(vec![Filter::eq(Doc::Owner, "bob"), Filter::IsNull(Doc::Missing)]).matches(&document));
    }

    /// A value meets what serde writes: an id the string of its UUID, a
    /// variant an object holding its key - among others, if it has them.
    #[test]
    fn a_value_meets_the_forms_serde_writes() {
        let id = Uuid::new_v4();
        assert!(FieldValue::from(id).matches(&json!(id.to_string())));
        assert!(FieldValue::from(id).matches(&json!(id.to_string().to_uppercase())), "a UUID, not its spelling");
        assert!(!FieldValue::from(id).matches(&json!(Uuid::new_v4().to_string())));
        assert!(!FieldValue::from(id).matches(&json!({ "User": id.to_string() })));
        assert!(FieldValue::from("GlobalScope").matches(&json!("GlobalScope")));
        assert!(!FieldValue::from("GlobalScope").matches(&json!({ "GlobalScope": null })));
        let user = FieldValue::tagged("User", id);
        assert!(user.matches(&json!({ "User": id.to_string() })));
        assert!(!user.matches(&json!({ "Agent": id.to_string() })), "another variant");
        assert!(!user.matches(&json!(id.to_string())));
        let channel = FieldValue::tagged("ChannelScope", FieldValue::tagged("channel_id", id));
        assert!(channel.matches(&json!({ "ChannelScope": { "session_id": Uuid::new_v4().to_string(), "channel_id": id.to_string() } })), "containment");
        assert!(!channel.matches(&json!({ "SessionScope": { "session_id": id.to_string() } })));
        let users = FieldValue::variant("UserScope");
        assert!(users.matches(&json!({ "UserScope": { "user_id": id.to_string() } })), "a variant, whatever it holds");
        assert!(!users.matches(&json!({ "SessionScope": { "session_id": id.to_string() } })));
        assert!(FieldValue::variant("GlobalScope").matches(&json!("GlobalScope")), "also one that holds nothing");
        assert!(!FieldValue::variant("GlobalScope").matches(&json!("UserScope")));
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
