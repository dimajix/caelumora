//! Values by their type (issues 99, 296, 323): the facets a component provides, the services of the
//! kernel, the stores a backend opened - each an `Arc<T>` of a trait object or a type, found by `T`
//! (`ErasedArc`).

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

/// A type, by its id and - for messages - its name: of a facet a component
/// provides, of a service, of a store. It is made in a constant expression
/// (`of`), so that a list of types can be a constant (issue 337).
#[derive(Clone, Copy)]
pub struct TypeKey {
    pub id: TypeId,
    /// The name, found only when it is asked for: `type_name` cannot be
    /// called in a constant expression.
    name: fn() -> &'static str,
}

impl TypeKey {
    pub const fn of<T: ?Sized + 'static>() -> Self {
        Self { id: TypeId::of::<T>(), name: short_name::<T> }
    }

    /// The type's name without its path: `dyn crate::prompt::PromptProvider`
    /// is `PromptProvider`.
    pub fn name(&self) -> &'static str {
        (self.name)()
    }
}

impl Debug for TypeKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

fn short_name<T: ?Sized + 'static>() -> &'static str {
    let full = type_name::<T>();
    full.rfind("::").map_or(full, |at| &full[at + 2..])
}

/// An `Arc<T>` of a trait object or a type, with `T` forgotten until it is asked for again
/// (`get`): what is kept of a value found by its type, here and wherever else one is
/// (issue 516).
#[derive(Clone)]
pub struct ErasedArc(Arc<dyn Any + Send + Sync>);

impl ErasedArc {
    /// `value`, its type forgotten.
    pub fn of<T: ?Sized + Send + Sync + 'static>(value: Arc<T>) -> Self {
        Self(Arc::new(value))
    }

    /// The value, if it is an `Arc<T>`.
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.0.downcast_ref::<Arc<T>>()
    }
}

impl Debug for ErasedArc {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("ErasedArc")
    }
}

/// Values by their type, each an `Arc<T>` of a trait object or a type.
#[derive(Default, Debug, Clone)]
pub struct TypeMap {
    map: HashMap<TypeId, (&'static str, ErasedArc)>,
}

impl TypeMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, value: Arc<T>) {
        self.insert_of(TypeKey::of::<T>(), ErasedArc::of(value));
    }

    /// Inserts `value`, an `Arc` of the type `key` names, its type forgotten (issue 516).
    pub fn insert_of(&mut self, key: TypeKey, value: ErasedArc) {
        self.map.insert(key.id, (key.name(), value));
    }

    /// Whether it holds a value of the type `key` names.
    pub fn contains(&self, key: TypeKey) -> bool {
        self.map.contains_key(&key.id)
    }

    /// It with `value` besides.
    pub fn with<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.insert(value);
        self
    }

    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<&Arc<T>> {
        self.map.get(&TypeId::of::<T>()).and_then(|(_, erased)| erased.get::<T>())
    }

    /// It with the values of `keys` alone.
    pub fn only(&self, keys: &[TypeKey]) -> Self {
        let map = keys
            .iter()
            .filter_map(|key| self.map.get(&key.id).map(|value| (key.id, value.clone())))
            .collect();
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

    #[test]
    fn a_value_is_found_by_its_type_and_made_once() {
        let map = TypeMap::empty().with(Arc::new(7u32));
        assert_eq!(map.get::<u32>().map(|v| **v), Some(7));
        assert!(map.get::<dyn Debug + Send + Sync>().is_none(), "by the type it was put in as");
        assert_eq!(map.names(), ["u32"]);
    }

    /// A list of types can be a constant, and each is named without its path.
    #[test]
    fn a_type_key_is_made_in_a_constant() {
        trait Facet {}
        const KEYS: &[TypeKey] = &[TypeKey::of::<dyn Facet>(), TypeKey::of::<String>()];
        assert_eq!(KEYS[1].id, TypeId::of::<String>());
        assert_eq!(KEYS.iter().map(TypeKey::name).collect::<Vec<_>>(), ["Facet", "String"]);
    }
}
