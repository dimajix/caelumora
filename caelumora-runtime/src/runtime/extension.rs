//! The extensions of the core (issue 516): what a plugin binds to the views of an application's
//! work - a reader, which the work reads by, or a writer, which it acts by -, each a service of
//! the plugin's made from those views, so that it acts in the work's context. A component kind
//! declares the extensions it needs (`ComponentKind::reads`, `writes`), and the runtime is not
//! built where one is bound by no plugin.

use crate::view::Views;
use caelumora_model::type_map::{ErasedArc, TypeKey};
use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Which face of a service an extension is: what a work reads by, handed out where it reads
/// (`Situation`), or what it acts by, handed out where it acts (`Turn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    Reader,
    Writer,
}

impl fmt::Display for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
        })
    }
}

/// What the plugins bind to the views of an application's work, each by the type of what it binds
/// and as the face it is.
#[derive(Default)]
pub(super) struct Extensions {
    bindings: HashMap<TypeId, (Face, Binding)>,
}

/// How a bound service is made from the views of a work; `None` where it is not offered there -
/// artifacts where no storage of content is configured.
type Binding = Arc<dyn Fn(&Views) -> Option<ErasedArc> + Send + Sync>;

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions").field("bindings", &self.bindings.len()).finish()
    }
}

impl Extensions {
    /// Binds `T` as `face` to the views of every piece of work by `bind`. A `T` bound before is
    /// replaced.
    pub(super) fn insert<T, F>(&mut self, face: Face, bind: F)
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(&Views) -> Option<Arc<T>> + Send + Sync + 'static,
    {
        let binding: Binding = Arc::new(move |views| bind(views).map(ErasedArc::of));
        self.bindings.insert(TypeId::of::<T>(), (face, binding));
    }

    /// Whether the type `key` names is bound as `face`.
    pub(super) fn binds(&self, face: Face, key: TypeKey) -> bool {
        self.bindings.get(&key.id).is_some_and(|(bound, _)| *bound == face)
    }

    /// What is bound as the type `key` names, as `face`, made from `views` - if it is bound as that
    /// face and offered there.
    pub(super) fn made(&self, face: Face, key: TypeKey, views: &Views) -> Option<ErasedArc> {
        let (bound, bind) = self.bindings.get(&key.id)?;
        (*bound == face).then(|| bind(views)).flatten()
    }
}
