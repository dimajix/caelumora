//! What a condition of a query names and compares (issue 293): a field of a kind - a path into the
//! document as it is serialized (`Field`, `FieldPath`, `NoField`) - and a value in the forms serde
//! writes (`FieldValue`), made without serde, so that making a condition cannot fail.

#[cfg(test)]
use serde::Serialize;
use serde_json::Value;
use std::fmt::Debug;
use uuid::Uuid;

/// A field of a kind that a query may name: where it lies in the document
/// as it is serialized. A field used inside `Filter::Any` lies within an
/// element of the collection, its path relative to it - the element itself
/// for an empty path.
pub trait Field: Copy + Debug + Send + Sync + 'static {
    fn path(self) -> &'static [&'static str];
}

/// A field by its path alone (issue 447): how a filter crosses the seam to a
/// backend's storage, which knows no kind's field type (`Filter::by_path`).
pub type FieldPath = &'static [&'static str];

impl Field for FieldPath {
    fn path(self) -> &'static [&'static str] {
        self
    }
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
    /// A flag.
    Bool(bool),
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
            FieldValue::Uuid(id) => {
                found.as_str().and_then(|text| Uuid::parse_str(text).ok()) == Some(*id)
            }
            FieldValue::Text(text) => found.as_str() == Some(text.as_str()),
            FieldValue::Bool(flag) => found.as_bool() == Some(*flag),
            FieldValue::Tagged(tag, value) => {
                found.get(tag).is_some_and(|inner| value.matches(inner))
            }
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

impl From<bool> for FieldValue {
    fn from(flag: bool) -> Self {
        FieldValue::Bool(flag)
    }
}

/// For the tests of a type that converts into a `FieldValue`: each of
/// `samples`, converted, meets exactly the serialized samples equal to it -
/// which holds the conversion, written by hand, to serde's tags.
#[cfg(test)]
pub(crate) fn meets_exactly_its_equals<T>(samples: &[T])
where
    T: Clone + PartialEq + Debug + Serialize + Into<FieldValue>,
{
    for value in samples {
        let converted: FieldValue = value.clone().into();
        for other in samples {
            let serialized = serde_json::to_value(other).expect("a sample serializes");
            assert_eq!(
                converted.matches(&serialized),
                value == other,
                "{value:?} against {other:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A value meets what serde writes: an id the string of its UUID, a
    /// variant an object holding its key - among others, if it has them.
    #[test]
    fn a_value_meets_the_forms_serde_writes() {
        let id = Uuid::new_v4();
        assert!(FieldValue::from(id).matches(&json!(id.to_string())));
        assert!(
            FieldValue::from(id).matches(&json!(id.to_string().to_uppercase())),
            "a UUID, not its spelling"
        );
        assert!(!FieldValue::from(id).matches(&json!(Uuid::new_v4().to_string())));
        assert!(!FieldValue::from(id).matches(&json!({ "User": id.to_string() })));
        assert!(FieldValue::from("GlobalScope").matches(&json!("GlobalScope")));
        assert!(!FieldValue::from("GlobalScope").matches(&json!({ "GlobalScope": null })));
        let user = FieldValue::tagged("User", id);
        assert!(user.matches(&json!({ "User": id.to_string() })));
        assert!(!user.matches(&json!({ "Agent": id.to_string() })), "another variant");
        assert!(!user.matches(&json!(id.to_string())));
        let channel = FieldValue::tagged("ChannelScope", FieldValue::tagged("channel_id", id));
        let scope = json!({
            "ChannelScope": {
                "session_id": Uuid::new_v4().to_string(),
                "channel_id": id.to_string()
            }
        });
        assert!(channel.matches(&scope), "containment");
        assert!(!channel.matches(&json!({ "SessionScope": { "session_id": id.to_string() } })));
        let users = FieldValue::variant("UserScope");
        assert!(
            users.matches(&json!({ "UserScope": { "user_id": id.to_string() } })),
            "a variant, whatever it holds"
        );
        assert!(!users.matches(&json!({ "SessionScope": { "session_id": id.to_string() } })));
        assert!(
            FieldValue::variant("GlobalScope").matches(&json!("GlobalScope")),
            "also one that holds nothing"
        );
        assert!(!FieldValue::variant("GlobalScope").matches(&json!("UserScope")));
    }
}
