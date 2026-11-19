//! The procedures of a channel's lifecycle (issue 593): creating a channel with its founders,
//! deleting and restoring it, building again the channels left out as their session started, and
//! reclassifying one.
//!
//! Each locks the channels it changes exclusively (`Key::Channel`) and the participants it takes
//! in or lets go; each but building again, which opens no session, locks its session as opening it
//! does - shared where it runs, exclusively where it starts it. Each carries the change out by the
//! building blocks of the `SessionLoop` and the `ChannelLoop`, which ask for the proof that it
//! holds them (`Admitted`). Whoever takes someone into a channel or lets them go holds it shared
//! (`participant`), so that neither crosses a channel being made, deleted, restored, built again or
//! reclassified.

use super::session::lock_to_open;
use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::ChannelLoop;
use crate::session::{ChannelRun, ChannelSpec, SessionError, SessionManager};
use async_trait::async_trait;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::iter;
use std::sync::Arc;

/// Creates a channel in a session on behalf of `context`, starting the session where it does not
/// run, with its founders - its participants, the creator first if in the session -, who join it by
/// the one way in once it is started (`SessionLoop::create_channel_held`, `found`); taken back if
/// one cannot (`SessionLoop::abandon_channel_held`). Holds the channel, its id chosen before it is
/// built, until it is complete, so that nobody takes someone into it before its founders.
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

    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::create_channel_held` and taking a founder in
    /// (`ChannelLoop::found`); the channel is taken back then.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        log: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        let Self { sessions, context, session, id, spec } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let (channel, founders) = session.create_channel_held(admitted, &context, id, spec).await?;
        {
            let session = Arc::clone(&session);
            let channel = Arc::clone(&channel);
            log.record("creating a channel", move |admitted| {
                Box::pin(async move {
                    session.abandon_channel_held(admitted, &channel).await;
                    Ok(())
                })
            });
        }
        for founder in founders {
            session.found(admitted, &context, &channel, founder).await?;
        }
        session.announce_created(&channel);
        Ok(channel)
    }
}

/// Deletes a channel of a session on behalf of `context`, starting the session where it does not
/// run (`SessionLoop::delete_channel_held`): its actors leave, its task ends, it is marked.
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

    /// `false` if there is no channel the caller reaches.
    ///
    /// # Errors
    /// As `SessionManager::open_held` and `SessionLoop::delete_channel_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, session, channel } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        Ok(session.delete_channel_held(admitted, &context, channel).await?)
    }
}

/// Restores a deleted channel of a session on behalf of `context`, starting the session where it
/// does not run, and starts the channel, letting go whom the session let go meanwhile
/// (`SessionLoop::restore_channel_held`). Holds its participants, so that nobody takes one in or
/// lets one go while it is listed again.
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

    /// # Errors
    /// As `SessionManager::open_held` and `SessionLoop::restore_channel_held`.
    async fn carry_out(
        self,
        read: Result<(), SessionError>,
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        read?;
        let Self { sessions, context, session, channel } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        Ok(session.restore_channel_held(admitted, &context, channel).await?)
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
