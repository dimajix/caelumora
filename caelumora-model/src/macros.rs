//! Macros for defining common types in Caelumora.

/// Macro for defining a new ID type wrapping a Uuid.
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, derive_more::Display, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

        impl Into<uuid::Uuid> for $name {
            fn into(self) -> uuid::Uuid {
                self.0
            }
        }
    };
}
