//! Values by their type (issues 99, 296, 323): the facets a component
//! provides, the services of the runtime, the stores a backend opened -
//! each an `Arc<T>` of a trait object or a type, found by `T`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// A type, by its id and - for messages - its name: of a facet a component
/// provides, of a service, of a store.
#[derive(Debug, Clone, Copy)]
pub struct TypeKey {
    pub id: TypeId,
    pub name: &'static str,
}

impl TypeKey {
    pub fn of<T: ?Sized + 'static>() -> Self {
        // The trait's name without its path: `dyn crate::context::PromptProvider`
        // is shown as `PromptProvider`.
        let full = std::any::type_name::<T>();
        let name = full.rfind("::").map_or(full, |at| &full[at + 2..]);
        Self { id: TypeId::of::<T>(), name }
    }
}

/// Values by their type, each an `Arc<T>` of a trait object or a type.
#[derive(Default, Debug, Clone)]
pub struct TypeMap {
    map: HashMap<TypeId, (&'static str, Arc<dyn Any + Send + Sync>)>,
}

impl TypeMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, value: Arc<T>) {
        let key = TypeKey::of::<T>();
        self.map.insert(key.id, (key.name, Arc::new(value) as Arc<dyn Any + Send + Sync>));
    }

    /// It with `value` besides.
    pub fn with<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.insert(value);
        self
    }

    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|(_, boxed)| boxed.downcast_ref::<Arc<T>>())
    }

    /// The value of `T`, made by `make` and kept if there is none yet.
    ///
    /// # Errors
    /// Whatever `make` fails with; nothing is kept then.
    pub fn get_or_try_insert<T: ?Sized + Send + Sync + 'static, E>(&mut self, make: impl FnOnce() -> Result<Arc<T>, E>) -> Result<Arc<T>, E> {
        if let Some(value) = self.get::<T>() {
            return Ok(Arc::clone(value));
        }
        let value = make()?;
        self.insert(Arc::clone(&value));
        Ok(value)
    }

    /// It with the values of `keys` alone.
    pub fn only(&self, keys: &[TypeKey]) -> Self {
        let map = keys.iter().filter_map(|key| self.map.get(&key.id).map(|value| (key.id, value.clone()))).collect();
        Self { map }
    }

    /// The names of the values' types, sorted - for messages.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.map.values().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Debug;

    #[test]
    fn a_value_is_found_by_its_type_and_made_once() {
        let mut map = TypeMap::empty().with(Arc::new(7u32));
        assert_eq!(map.get::<u32>().map(|v| **v), Some(7));
        assert!(map.get::<dyn Debug + Send + Sync>().is_none(), "by the type it was put in as");
        let made = map.get_or_try_insert::<String, ()>(|| Ok(Arc::new("first".to_string()))).unwrap();
        let again = map.get_or_try_insert::<String, ()>(|| unreachable!("it is there")).unwrap();
        assert!(Arc::ptr_eq(&made, &again));
        assert!(map.get_or_try_insert::<u8, &str>(|| Err("cannot")).is_err());
        assert!(map.get::<u8>().is_none(), "nothing kept of a failure");
        assert_eq!(map.names(), ["String", "u32"]);
    }
}
