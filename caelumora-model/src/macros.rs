//! Macros for defining common types in Caelumora.

/// Macro for defining a new ID type wrapping a Uuid.
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        // Written out rather than derived, so that a crate using the macro
        // - a plugin's (issue 323) - needs no `derive_more`.
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                text.parse().map(Self)
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
