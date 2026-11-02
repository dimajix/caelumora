//! The condition of a query (issue 293, `Filter`): purely declarative, judged here on a document as
//! it is serialized, as a database judges it in a `WHERE`.

use super::{Field, FieldPath, FieldValue};
use crate::store::{StoreError, StoreResult};
use crate::value::name::fold;
use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::ops::Range;

/// A condition on the documents of a kind. Purely declarative: equality,
/// membership, absence, comparison with a number, the boolean operators,
/// and whether any element of a collection meets a condition of its own.
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
    /// The field, a text, contains the words as names are compared -
    /// regardless of case, accents and runs of whitespace: a search of what
    /// was written (issue 421). A database compares the folded forms with
    /// `LIKE '%…%'`.
    Contains(F, String),
    /// The field is a number of at least the bound (issue 451): a position
    /// from where a range begins.
    AtLeast(F, u64),
    /// The field is a number below the bound: a position before the end of
    /// a range.
    Below(F, u64),
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

    /// `field`, a text, contains `words` as names are compared.
    pub fn contains(field: F, words: &str) -> Self {
        Filter::Contains(field, words.to_string())
    }

    /// `field` is a number within `range`; no bound at the end if it ends
    /// at `u64::MAX`, after every number.
    pub fn within(field: F, range: Range<u64>) -> Self {
        match range.end {
            u64::MAX => Filter::AtLeast(field, range.start),
            end => Filter::all_of([Filter::AtLeast(field, range.start), Filter::Below(field, end)]),
        }
    }

    /// Every one of `conditions`; any document if there is none.
    pub fn all_of(conditions: impl IntoIterator<Item = Filter<F>>) -> Self {
        Filter::And(conditions.into_iter().collect())
    }

    /// At least one of `conditions`; no document if there is none.
    pub fn any_of(conditions: impl IntoIterator<Item = Filter<F>>) -> Self {
        Filter::Or(conditions.into_iter().collect())
    }

    /// No document.
    pub fn nothing() -> Self {
        Filter::Or(Vec::new())
    }

    /// Not `condition`.
    pub fn negated(condition: Filter<F>) -> Self {
        Filter::Not(Box::new(condition))
    }

    /// The same condition, its fields named by their paths (`FieldPath`):
    /// what a storage is given, which knows no kind's field type.
    pub fn by_path(&self) -> Filter<FieldPath> {
        match self {
            Filter::Eq(field, value) => Filter::Eq(field.path(), value.clone()),
            Filter::In(field, values) => Filter::In(field.path(), values.clone()),
            Filter::IsNull(field) => Filter::IsNull(field.path()),
            Filter::And(conditions) => {
                Filter::And(conditions.iter().map(Filter::by_path).collect())
            }
            Filter::Or(conditions) => Filter::Or(conditions.iter().map(Filter::by_path).collect()),
            Filter::Not(condition) => Filter::Not(Box::new(condition.by_path())),
            Filter::Any(collection, condition) => {
                Filter::Any(collection.path(), Box::new(condition.by_path()))
            }
            Filter::NamePrefix(field, prefix) => Filter::NamePrefix(field.path(), prefix.clone()),
            Filter::Contains(field, words) => Filter::Contains(field.path(), words.clone()),
            Filter::AtLeast(field, bound) => Filter::AtLeast(field.path(), *bound),
            Filter::Below(field, bound) => Filter::Below(field.path(), *bound),
        }
    }

    /// Whether `document` - an entity as it is serialized, or an element of
    /// a collection in it - meets the condition.
    pub fn matches(&self, document: &Value) -> bool {
        match self {
            Filter::Eq(field, value) => {
                lookup(document, *field).is_some_and(|found| value.matches(found))
            }
            Filter::In(field, values) => lookup(document, *field)
                .is_some_and(|found| values.iter().any(|value| value.matches(found))),
            Filter::IsNull(field) => lookup(document, *field).is_none_or(Value::is_null),
            Filter::And(conditions) => conditions.iter().all(|c| c.matches(document)),
            Filter::Or(conditions) => conditions.iter().any(|c| c.matches(document)),
            Filter::Not(condition) => !condition.matches(document),
            Filter::Any(collection, condition) => match lookup(document, *collection) {
                Some(Value::Array(elements)) => {
                    elements.iter().any(|element| condition.matches(element))
                }
                _ => false,
            },
            Filter::NamePrefix(field, prefix) => lookup(document, *field)
                .and_then(Value::as_str)
                .is_some_and(|name| fold(name).starts_with(&fold(prefix))),
            Filter::Contains(field, words) => lookup(document, *field)
                .and_then(Value::as_str)
                .is_some_and(|text| fold(text).contains(&fold(words))),
            Filter::AtLeast(field, bound) => lookup(document, *field)
                .and_then(|found| compared(found, *bound))
                .is_some_and(Ordering::is_ge),
            Filter::Below(field, bound) => lookup(document, *field)
                .and_then(|found| compared(found, *bound))
                .is_some_and(Ordering::is_lt),
        }
    }
}

/// How `found` compares with `bound`, if it is a number - exactly, as a
/// database compares numbers: a negative one below every bound.
fn compared(found: &Value, bound: u64) -> Option<Ordering> {
    let Value::Number(number) = found else { return None };
    match (number.as_u64(), number.as_i64()) {
        (Some(whole), _) => Some(whole.cmp(&bound)),
        (None, Some(_)) => Some(Ordering::Less),
        (None, None) => number.as_f64().and_then(|fraction| fraction.partial_cmp(&(bound as f64))),
    }
}

/// What lies at `field` in `document`, if anything.
fn lookup(document: &Value, field: impl Field) -> Option<&Value> {
    field.path().iter().try_fold(document, |at, key| at.get(key))
}

/// `value`, if it meets `filter`, judged on it serialized - for a store
/// that holds its entities as values, not as documents.
///
/// # Errors
/// `StoreError::Validation` if `value` cannot be serialized.
pub fn meeting<T: Serialize, F: Field>(
    value: T,
    filter: Option<&Filter<F>>,
) -> StoreResult<Option<T>> {
    match filter {
        None => Ok(Some(value)),
        Some(filter) => Ok(filter.matches(&json(&value)?).then_some(value)),
    }
}

/// `value` serialized, for a filter to be judged on.
fn json<T: Serialize + ?Sized>(value: &T) -> StoreResult<Value> {
    serde_json::to_value(value).map_err(|e| {
        StoreError::Validation(format!("an entity cannot be serialized for a query: {e}"))
    })
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
        Itself,
    }

    impl Field for Doc {
        fn path(self) -> &'static [&'static str] {
            match self {
                Doc::Owner => &["owner", "id"],
                Doc::Tags => &["tags"],
                Doc::Tag => &["name"],
                Doc::Missing => &["missing"],
                Doc::Itself => &[],
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
    fn a_text_contains_words_as_names_are_compared() {
        let written = |text: &str| json!({ "owner": { "id": text } });
        let containing = |words: &str| Filter::contains(Doc::Owner, words);
        assert!(
            containing("ZOË").matches(&written("I met zoe today")),
            "regardless of case and accents"
        );
        assert!(
            containing("met  zoe").matches(&written("I met\nZoë")),
            "and of runs of whitespace"
        );
        assert!(!containing("bob").matches(&written("I met Zoë")));
        assert!(containing("").matches(&written("anything")), "no words are in every text");
        assert!(!containing("x").matches(&json!({ "owner": { "id": true } })), "a text only");
    }

    #[test]
    fn a_filter_reads_a_document_by_paths() {
        let document =
            json!({ "owner": { "id": "alice" }, "tags": [{ "name": "a" }, { "name": "b" }] });
        assert!(Filter::eq(Doc::Owner, "alice").matches(&document));
        assert!(!Filter::eq(Doc::Owner, "bob").matches(&document));
        assert!(Filter::is_in(Doc::Owner, ["bob", "alice"]).matches(&document));
        assert!(
            Filter::IsNull(Doc::Missing).matches(&document)
                && !Filter::IsNull(Doc::Owner).matches(&document)
        );
        assert!(
            Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "b")).matches(&document),
            "an element of the collection"
        );
        assert!(!Filter::any(Doc::Tags, Filter::eq(Doc::Tag, "c")).matches(&document));
        assert!(
            !Filter::any(Doc::Owner, Filter::eq(Doc::Tag, "a")).matches(&document),
            "what is no collection has no element"
        );
        let names = json!({ "tags": ["a", "b"] });
        assert!(
            Filter::any(Doc::Tags, Filter::eq(Doc::Itself, "b")).matches(&names),
            "an element itself, by the empty path"
        );
        assert!(!Filter::any(Doc::Tags, Filter::eq(Doc::Itself, "c")).matches(&names));
        let both = Filter::And(vec![
            Filter::eq(Doc::Owner, "alice"),
            Filter::Not(Box::new(Filter::IsNull(Doc::Tags))),
        ]);
        assert!(both.matches(&document));
        assert!(Filter::Or(vec![Filter::eq(Doc::Owner, "bob"), Filter::IsNull(Doc::Missing)])
            .matches(&document));
        assert!(
            Filter::<Doc>::all_of([]).matches(&document)
                && !Filter::<Doc>::nothing().matches(&document)
        );
        assert!(
            Filter::eq(Doc::Owner, true).matches(&json!({ "owner": { "id": true } }))
                && !Filter::eq(Doc::Owner, true).matches(&document),
            "a flag"
        );
    }

    /// A number is compared with a bound exactly, whatever its form; what
    /// is no number meets no comparison (issue 451).
    #[test]
    fn a_number_is_compared_with_a_bound() {
        let at = |value: Value| json!({ "owner": { "id": value } });
        let range = Filter::within(Doc::Owner, 3..5);
        assert!(range.matches(&at(json!(3))) && range.matches(&at(json!(4))), "from its start");
        assert!(!range.matches(&at(json!(5))) && !range.matches(&at(json!(2))), "up to its end");
        assert!(
            range.matches(&at(json!(4.5))) && !range.matches(&at(json!(2.5))),
            "a fraction too"
        );
        assert!(
            !range.matches(&at(json!(-1))) && Filter::Below(Doc::Owner, 0).matches(&at(json!(-1))),
            "a negative number below every bound"
        );
        assert!(
            Filter::within(Doc::Owner, 3..u64::MAX).matches(&at(json!(u64::MAX))),
            "open at the end"
        );
        assert!(
            !range.matches(&at(json!("4")))
                && !Filter::AtLeast(Doc::Missing, 0).matches(&at(json!(4))),
            "a number only, where there is one"
        );
        assert!(Filter::negated(Filter::AtLeast(Doc::Missing, 0)).matches(&at(json!(4))));
    }
}
