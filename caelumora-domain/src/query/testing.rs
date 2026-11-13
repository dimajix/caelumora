//! What the tests of the kinds whose values a query compares share.

use crate::query::FieldValue;
use serde::Serialize;
use std::fmt::Debug;

/// For the tests of a type that converts into a `FieldValue`: each of
/// `samples`, converted, meets exactly the serialized samples equal to it -
/// which holds the conversion, written by hand, to serde's tags.
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
