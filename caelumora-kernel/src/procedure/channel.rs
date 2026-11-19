//! The procedures of a channel's lifecycle (issue 593): creating a channel with its founders,
//! deleting and restoring it, building again the channels left out as their session started, and
//! reclassifying one.
//!
//! Each locks the channels it changes exclusively (`Key::Channel`) and the participants it takes in
//! or lets go; each but building again, which opens no session, locks its session as opening it
//! does - shared where it runs, exclusively where it starts it. Each carries the change out by the
//! building blocks of the `SessionLoop` and the `ChannelLoop`, which ask for the proof that it
//! holds them (`Admitted`): asked first by the objects' own questions, which store nothing, then
//! one step after the other, each that can be set back undone by its compensation if a later one
//! fails - not what deleting a channel deleted of its scope, nor whom restoring it let go. Whoever
//! takes someone into a channel or lets them go holds it shared (`participant`), so that neither
//! crosses a channel being made, deleted, restored, built again or reclassified.

use super::participant::{take_in, Entry};
use super::session::lock_to_open;
use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::ChannelLoop;
use crate::session::{ChannelRun, ChannelSpec, SessionError, SessionManager};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, HistoryAccess, ThreadId};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::removal::Removal;
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use caelumora_store::Deleted;
use std::iter;
use std::sync::Arc;

/// Creates a channel in a session on behalf of `context`, starting the session where it does not
/// run, with its founders - its participants, the creator first if in the session -, who join it by
/// the one way in once it is started (`participant::take_in`); taken back if one cannot. Holds the
/// channel, its id chosen before it is built, until it is complete, so that nobody takes someone
/// into it before its founders.
pub struct Create {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    /// Chosen before it is built, so that it is held before anyone can find it.
    id: ChannelId,
    spec: ChannelSpec,
}

impl Create {
    /// Creates the channel `spec` asks for in `session` on behalf of `context`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        spec: ChannelSpec,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, id: ChannelId::new(), spec }
    }
}

#[async_trait]
impl Procedure for Create {
    type Plan = ();
    type Output = Arc<ChannelLoop>;
    type Error = SessionError;

    /// The founders it may take in: the creator and those `spec` names.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.id))?;
        let founders =
            iter::once(self.context.initiator_id).chain(self.spec.participants.iter().copied());
        for founder in founders {
            locks.exclusive(Key::Participant(self.session, founder))?;
        }
        Ok(())
    }

    /// Asked before anything is stored (`SessionLoop::channel_asked_for`); then stored, its group
    /// made, listed in the session last - which asks whether `context` may change it - and
    /// started, each step taken back by `log`, the last first, if a later one fails: stopped, taken
    /// off the session's list and purged with its group, as if it had never been made. Its
    /// founders join it by the one way in once it is started (`participant::take_in`, issue 420),
    /// so that its history records them as anyone who joins later; it is announced as created once
    /// they joined.
    ///
    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_asked_for`, the building blocks that
    /// store, list and start it, and taking a founder in (`participant::take_in`).
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        let Self { sessions, context, session, id, spec } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let (mut document, founders) = session.channel_asked_for(&context, id, spec).await?;
        session.store_channel_held(admitted, &context, &mut document).await?;
        {
            let session = Arc::clone(&session);
            log.record("storing a channel", move |admitted| {
                Box::pin(async move { session.purge_channel_held(admitted, id).await })
            });
        }
        session.keep_channel_group_held(admitted, &document).await?;
        session.list_channel_held(admitted, &context, id).await?;
        {
            // Its creation was permitted: taken back in the kernel's name.
            let session = Arc::clone(&session);
            log.record("listing a channel", move |admitted| {
                Box::pin(async move {
                    session.unlist_channel_held(admitted, &SecurityContext::system(), id).await
                })
            });
        }
        let channel = session.run_channel_held(admitted, &document).await?;
        {
            let session = Arc::clone(&session);
            log.record("starting a channel", move |admitted| {
                Box::pin(async move {
                    session.stop_channel_held(admitted, id).await;
                    Ok(())
                })
            });
        }
        let into = Some((&channel, HistoryAccess::None, ThreadId::line(channel.id())));
        for founder in founders {
            take_in(admitted, log, &session, &context, founder, Entry::Founding, into).await?;
        }
        session.announce_created(&channel);
        Ok(channel)
    }
}

/// Deletes a channel of a session on behalf of `context`, starting the session where it does not
/// run: it is marked, its actors leave, its task ends.
pub struct Delete {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
}

impl Delete {
    /// Deletes channel `channel` of `session` on behalf of `context`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
    ) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), session, channel }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))
    }

    /// Asked first (`SessionLoop::channel_to_delete`); then taken off the session's list - which
    /// asks whether `context` may change it, so that a refused deletion leaves the channel running
    /// and listed -, marked, what lies in its scope deleted as far as `context` may (issue 320),
    /// and the group of its participants, in the kernel's name (issue 348); all of it carries the
    /// channel's mark and comes back with it (issues 178, 469). If any of it fails, the channel is
    /// marked no more and listed again, in the kernel's name - without what was
    /// deleted of its scope, but with its group -, and runs on. Then it is stopped and its
    /// deletion announced. `false` if there is no channel the caller reaches.
    ///
    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_to_delete` and the building blocks
    /// that unlist and mark it and delete its scope and group - `SessionError::ScopeNotEmpty` if
    /// something of its scope is left that `context` may not delete.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, session, channel: id } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let Some(channel) = session.channel_to_delete(&context, id).await? else {
            return Ok(false);
        };
        session.unlist_channel_held(admitted, &context, id).await?;
        {
            // Its deletion was permitted: put back in the kernel's name.
            let session = Arc::clone(&session);
            log.record("taking a channel off its session's list", move |admitted| {
                Box::pin(async move {
                    session.list_channel_held(admitted, &SecurityContext::system(), id).await
                })
            });
        }
        let removal = Removal::now(context.caller_id);
        session.mark_channel_deleted_held(admitted, &context, id, &removal).await?;
        {
            let session = Arc::clone(&session);
            log.record("marking a channel deleted", move |admitted| {
                Box::pin(async move {
                    let system = SecurityContext::system();
                    session.unmark_channel_held(admitted, &system, id).await.map(drop)
                })
            });
        }
        session.delete_scope_held(admitted, &context, id, &removal).await?;
        session.delete_channel_group_held(admitted, id, &removal).await?;
        session.stop_channel_held(admitted, id).await;
        session.announce_deleted(&channel, &context);
        Ok(true)
    }
}

/// Restores a deleted channel of a session on behalf of `context`, starting the session where it
/// does not run, and starts the channel, letting go whom the session let go meanwhile. Holds its
/// participants, so that nobody takes one in or lets one go while it is listed again.
pub struct Restore {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
}

impl Restore {
    /// Restores the deleted channel `channel` of `session` on behalf of `context`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
    ) -> Self {
        Self { sessions: Arc::clone(sessions), context: context.clone(), session, channel }
    }
}

#[async_trait]
impl Procedure for Restore {
    /// What reading the deleted channel's participants failed at, answered as it is carried out.
    type Plan = Result<(), SessionError>;
    type Output = Arc<ChannelLoop>;
    type Error = SessionError;

    /// Its participants as the deleted channel lists them, in a session that runs; one it starts
    /// it holds exclusively, the participants with it.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))?;
        let Some(running) = self.sessions.running(self.session) else { return Ok(Ok(())) };
        let participants = match running.participants_of_deleted(self.channel).await {
            Ok(participants) => participants,
            Err(error) => return Ok(Err(error.into())),
        };
        for participant in participants {
            locks.exclusive(Key::Participant(self.session, participant))?;
        }
        Ok(Ok(()))
    }

    /// Asked first (`SessionLoop::channel_to_restore`); then its mark taken away, the group of its
    /// participants restored with it in the kernel's name (issue 348) - once it runs, its
    /// participants reach it, whatever of its scope comes back -, who was let go from the session
    /// while it was deleted (issue 590) let go from it, in the kernel's name and removed by it,
    /// before it is built, listed in the session again - which asks whether `context` may change
    /// it - and started from its stored state. If any of that fails, it is deleted again with its
    /// group, marked as it was, so that it is neither half back nor lost, and a restoring tried
    /// again finds what went with it. What was deleted with it comes back last (issue 178); it is
    /// announced as restored.
    ///
    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_to_restore` and the building blocks
    /// that unmark, list and start it - `StoreError::NameTaken` if its name was taken since (issue
    /// 250).
    async fn carry_out(
        self,
        read: Result<(), SessionError>,
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        read?;
        let Self { sessions, context, session, channel: id } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let (Deleted { removal, .. }, gone) = session.channel_to_restore(&context, id).await?;
        let document = session.unmark_channel_held(admitted, &context, id).await?;
        {
            // Its restoring was permitted: deleted again in the kernel's name.
            let (session, removal) = (Arc::clone(&session), removal.clone());
            log.record("taking a channel's mark away", move |admitted| {
                Box::pin(async move {
                    let system = SecurityContext::system();
                    session.mark_channel_deleted_held(admitted, &system, id, &removal).await
                })
            });
        }
        session.restore_channel_group_held(admitted, id, &removal).await?;
        {
            let (session, removal) = (Arc::clone(&session), removal.clone());
            log.record("restoring a channel's group", move |admitted| {
                Box::pin(
                    async move { session.delete_channel_group_held(admitted, id, &removal).await },
                )
            });
        }
        let document = session
            .let_go_where_not_running_held(admitted, document, &gone, ActorId::System)
            .await?;
        session.list_channel_held(admitted, &context, id).await?;
        {
            let session = Arc::clone(&session);
            log.record("listing a channel", move |admitted| {
                Box::pin(async move {
                    session.unlist_channel_held(admitted, &SecurityContext::system(), id).await
                })
            });
        }
        let channel = session.run_channel_held(admitted, &document).await?;
        session.restore_scope_held(admitted, &context, id, &removal).await;
        session.announce_restored(&document, &context);
        Ok(channel)
    }
}

/// Builds again the channels of a running session left out as it started, their components not
/// to be read (issue 463, `SessionLoop::retry_unavailable_held`); nothing for a session that does
/// not run. Asked by whoever opens a session, after opening it - the REST server, as every
/// request reaches the session (`ensure_session`).
pub struct Start {
    sessions: Arc<SessionManager>,
    session: SessionId,
}

impl Start {
    /// Builds again the channels of `session` left out as it started.
    pub fn left_out(sessions: &Arc<SessionManager>, session: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), session }
    }
}

#[async_trait]
impl Procedure for Start {
    /// Whether there is anything to build again.
    type Plan = bool;
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<bool, Deferred> {
        let Some(running) = self.sessions.running(self.session) else { return Ok(false) };
        let left_out: Vec<ChannelId> = running
            .listed_channels()
            .iter()
            .filter(|(_, run)| matches!(run, ChannelRun::Unavailable(_)))
            .map(|(channel, _)| channel.id)
            .collect();
        for channel in &left_out {
            locks.exclusive(Key::Channel(self.session, *channel))?;
        }
        Ok(!left_out.is_empty())
    }

    async fn carry_out(
        self,
        any: bool,
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        if let (true, Some(running)) = (any, self.sessions.running(self.session)) {
            running.retry_unavailable_held(admitted).await;
        }
        Ok(())
    }
}

/// Gives a channel of a session another classification on behalf of `context`, starting the
/// session where it does not run (`ChannelLoop::reclassify_held`): who takes someone in is asked
/// whether they are cleared for the classification, and raising it whether every participant is -
/// holding the channel, neither crosses the other.
pub struct Reclassify {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    classification: Classification,
}

impl Reclassify {
    /// Gives channel `channel` of `session` `classification`, on behalf of `context`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        classification: Classification,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, channel, classification }
    }
}

#[async_trait]
impl Procedure for Reclassify {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))
    }

    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_for` and
    /// `ChannelLoop::reclassify_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { sessions, context, session, channel, classification } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        Ok(channel.reclassify_held(admitted, &context, classification).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{Create, Delete, Reclassify, Restore};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::session::testing::spec;
    use crate::testing::World;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use std::sync::Arc;

    /// Reclassifying, deleting and restoring a channel hold it exclusively - each waits for whoever
    /// takes someone into it or lets them go, who holds it shared -; restoring holds its
    /// participants, and creating one its founders.
    #[tokio::test]
    async fn the_procedures_of_a_channel_hold_it_and_the_participants_they_change() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let (id, main) = (session.id(), session.channels()[0].id());
        let bob = ActorId::User(world.bob);

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let internal = Classification::general(Confidentiality::Internal);
        let reclassifying = Reclassify::new(&world.manager, &alice, id, main, internal);
        waits_for(held, &executor, reclassifying).await.expect("reclassified once let go");

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let deleting = Delete::new(&world.manager, &alice, id, main);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let restoring = Restore::new(&world.manager, &alice, id, main);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");
        executor.run(Delete::new(&world.manager, &alice, id, main)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let restoring = Restore::new(&world.manager, &alice, id, main);
        waits_for(held, &executor, restoring).await.expect("restored once bob is let go");

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let creating = Create::new(&world.manager, &alice, id, spec("side", vec![bob]));
        waits_for(held, &executor, creating).await.expect("created once its founder is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }
}
