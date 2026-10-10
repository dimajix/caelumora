//! Macros for defining common types in Caelumora.

/// Macro for defining a new ID type wrapping a Uuid.
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, derive_more::Display, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(uuid: uuid::Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl From<$name> for $crate::store::query::FieldValue {
            fn from(id: $name) -> Self {
                $crate::store::query::FieldValue::Uuid(id.0)
            }
        }
    };
}
