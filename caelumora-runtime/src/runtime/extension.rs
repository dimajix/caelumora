//! The extensions of the core (issue 516): what a plugin, or the core itself, binds to the session
//! of an application's work as the work reads it - a reader, which the work reads by, or a writer,
//! which it acts by -, each a service made from that reading, so that it acts in the work's
//! context. A component kind declares the extensions it needs (`ComponentKind::reads`, `writes`),
//! and the runtime is not built where one is bound by none.

use crate::view::SessionView;
use anyhow::{bail, Result};
use caelumora_model::type_map::{ErasedArc, TypeKey};
use futures::future::BoxFuture;
use futures::FutureExt;
use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
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

/// What the plugins and the core bind to an application's work, each by the type of what it binds
/// and as the face it is, once.
#[derive(Default)]
pub(super) struct Extensions {
    bindings: HashMap<TypeId, (Face, Binding)>,
}

/// How a bound service is made from the session of a work as the work reads it - its views; `None`
/// where it is not offered there - artifacts where no storage of content is configured, a
/// workspace the work may not use.
type Binding = Arc<dyn Fn(SessionView) -> BoxFuture<'static, Option<ErasedArc>> + Send + Sync>;

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions").field("bindings", &self.bindings.len()).finish()
    }
}

impl Extensions {
    /// Binds `T` as `face` to every piece of work by `bind`, given the work's session as the work
    /// reads it.
    ///
    /// # Errors
    /// If `T` is bound already: what one binds no other replaces.
    pub(super) fn insert<T, F, Fut>(&mut self, face: Face, bind: F) -> Result<()>
    where
        T: ?Sized + Send + Sync + 'static,
        F: Fn(SessionView) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<Arc<T>>> + Send + 'static,
    {
        let key = TypeKey::of::<T>();
        if self.bindings.contains_key(&key.id) {
            bail!("The {face} {} is bound already", key.name());
        }
        let binding: Binding =
            Arc::new(move |session| bind(session).map(|made| made.map(ErasedArc::of)).boxed());
        self.bindings.insert(key.id, (face, binding));
        Ok(())
    }

    /// Whether the type `key` names is bound as `face`.
    pub(super) fn binds(&self, face: Face, key: TypeKey) -> bool {
        self.bindings.get(&key.id).is_some_and(|(bound, _)| *bound == face)
    }

    /// What is bound as the type `key` names, as `face`, made from `session` as a work reads it -
    /// if it is bound as that face and offered there.
    pub(super) async fn made(
        &self,
        face: Face,
        key: TypeKey,
        session: SessionView,
    ) -> Option<ErasedArc> {
        let (bound, bind) = self.bindings.get(&key.id)?;
        if *bound != face {
            return None;
        }
        bind(session).await
    }
}
