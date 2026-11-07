//! The extensions of the kernel (issue 516): what a plugin, or the kernel itself, binds to the
//! session of an agent's work as the work reads it - a reader, which the work reads by, or a
//! writer, which it acts by -, each a service made from that reading, so that it acts in the work's
//! context. A component kind declares the extensions it needs (`ComponentKind::reads`, `writes`),
//! and the kernel is not built where one is bound by none. A writer may commit what a turn staged
//! of a kind (`Committing`, issue 518), which is handed to it when the turn completes.

use crate::view::SessionView;
use anyhow::{bail, Result};
use caelumora_api::component::Face;
use caelumora_api::staging::{CommitError, CommitResult, Committing};
use caelumora_domain::message::MessageId;
use caelumora_domain::turn::{StagedChange, StagedChanges};
use caelumora_domain::type_map::{ErasedArc, TypeKey};
use futures::future;
use futures::future::BoxFuture;
use futures::FutureExt;
use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

/// What the plugins and the kernel bind to an agent's work, each by the type of what it binds and
/// as the face it is, once.
#[derive(Default)]
pub(super) struct Extensions {
    bindings: HashMap<TypeId, (Face, Binding)>,
    /// What commits each kind a turn stages, by the name of the kind: the writer bound as the type
    /// the key names, and how it is handed the changes.
    committers: HashMap<&'static str, (TypeKey, Committer)>,
}

/// How a bound service is made from the session of a work as the work reads it - its views; `None`
/// where it is not offered there - artifacts where no storage of content is configured, a
/// workspace the work may not use.
type Binding = Arc<dyn Fn(SessionView) -> BoxFuture<'static, Option<ErasedArc>> + Send + Sync>;

/// How a writer bound for a kind, made for a work, is handed what a turn staged of that kind.
type Committer = Arc<
    dyn Fn(ErasedArc, &StagedChanges, MessageId) -> BoxFuture<'static, CommitResult<()>>
        + Send
        + Sync,
>;

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions")
            .field("bindings", &self.bindings.len())
            .field("committers", &self.committers.keys().collect::<Vec<_>>())
            .finish()
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

    /// Lets the writer `T` commit what a turn stages as `C`: handed it when the turn completes
    /// (`commit`).
    ///
    /// # Errors
    /// If no writer is bound as `T`, or what is staged as `C` is committed already.
    pub(super) fn commits<C, T>(&mut self) -> Result<()>
    where
        C: StagedChange,
        T: ?Sized + Committing<C> + 'static,
    {
        let key = TypeKey::of::<T>();
        if !self.binds(Face::Writer, key) {
            bail!("No writer {} is bound to commit what is staged as '{}'", key.name(), C::KIND);
        }
        if self.committers.contains_key(C::KIND) {
            bail!("What is staged as '{}' is committed already", C::KIND);
        }
        let committer: Committer = Arc::new(|made, staged, after| {
            let Some(writer) = made.get::<T>().cloned() else {
                return future::ready(Err(CommitError::NotOffered(C::KIND.to_string()))).boxed();
            };
            match staged.of::<C>() {
                Ok(changes) => async move { writer.commit(changes, after).await }.boxed(),
                Err(source) => {
                    let unreadable = CommitError::Unreadable { kind: C::KIND.to_string(), source };
                    future::ready(Err(unreadable)).boxed()
                }
            }
        });
        self.committers.insert(C::KIND, (key, committer));
        Ok(())
    }

    /// Hands what a turn that ended with message `after` staged as `kind` in `staged` to the
    /// writer that commits that kind, made from `session` as the turn's work reads it.
    ///
    /// # Errors
    /// `NotOffered` if no writer commits `kind`, or none is offered there; `Unreadable` if the
    /// changes are not what it commits; what the writer reports.
    pub(super) async fn commit(
        &self,
        session: SessionView,
        staged: &StagedChanges,
        kind: &str,
        after: MessageId,
    ) -> CommitResult<()> {
        let not_offered = || CommitError::NotOffered(kind.to_string());
        let (key, committer) = self.committers.get(kind).ok_or_else(not_offered)?;
        let made = self.made(Face::Writer, *key, session).await.ok_or_else(not_offered)?;
        committer(made, staged, after).await
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
