//! The procedures of a participant's lifecycle (issue 593): taking someone into a session - and
//! into a channel of it -, added or joining, and letting them go, removed or leaving, from the
//! session with every channel of it or from one channel.
//!
//! Each locks the participant exclusively (`Key::Participant`), its session as opening it does -
//! shared where it runs, exclusively where it starts it -, and the channels it changes shared; and
//! carries the change out by the building blocks of the `SessionLoop` and the `ChannelLoop`, which
//! ask for the proof that it holds them (`Admitted`). It holds them until `run` has carried the
//! change out: a newcomer's actor started and its join announced, a leaver's actor ended and its
//! leave announced - so that nobody takes the same participant in or lets them go meanwhile.

use super::session::lock_to_open;
use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure};
use crate::session::{SessionError, SessionLoop, SessionManager};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, HistoryAccess};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::sync::Arc;

/// Takes a participant into a session, and into a channel of it at once if one is named - added
/// by whoever may change them, or joining by their own act -, starting the session where it does
/// not run: as opening it does for who adds (`SessionManager::open_held`), in the kernel's name
/// for who may join or read it for who joins (`SessionManager::open_to_take_in`).
pub struct TakeIn {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    participant: ActorId,
    coming_in: ComingIn,
}

/// How a participant is taken in, and into which channel.
enum ComingIn {
    /// Added by whoever may change the session and the channel, given as much of its history.
    Added(Option<(ChannelId, HistoryAccess)>),
    /// Joining by their own act, where the access lists let them.
    Joining(Option<ChannelId>),
}

impl TakeIn {
    /// Takes `participant` into `session` on behalf of `context`, who may change it, and into the
    /// channel `into`, seeing as much of its history as granted
    /// (`SessionLoop::add_participant_held`).
    pub fn added(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        into: Option<(ChannelId, HistoryAccess)>,
    ) -> Self {
        Self::new(sessions, context, session, participant, ComingIn::Added(into))
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - into `session` by their
    /// own act, and into the channel `into` (`SessionLoop::join_held`).
    pub fn joining(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        into: Option<ChannelId>,
    ) -> Self {
        Self::new(sessions, context, session, participant, ComingIn::Joining(into))
    }

    fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        coming_in: ComingIn,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, participant, coming_in }
    }
}

#[async_trait]
impl Procedure for TakeIn {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Participant(self.session, self.participant))?;
        let channel = match self.coming_in {
            ComingIn::Added(into) => into.map(|(channel, _)| channel),
            ComingIn::Joining(into) => into,
        };
        if let Some(channel) = channel {
            locks.shared(Key::Channel(self.session, channel))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `SessionManager::open_held` or `open_to_take_in`, and as
    /// `SessionLoop::add_participant_held` or `join_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { sessions, context, session, participant, coming_in } = self;
        match coming_in {
            ComingIn::Added(into) => {
                let session = sessions.open_held(admitted, &context, session).await?;
                session.add_participant_held(admitted, &context, participant, into).await?;
                Ok(session)
            }
            ComingIn::Joining(into) => {
                let session = sessions.open_to_take_in(admitted, &context, session).await?;
                session.join_held(admitted, &context, participant, into).await?;
                Ok(session)
            }
        }
    }
}

/// Lets a participant go from a session with every channel of it, or from one channel - removed by
/// whoever may change them, or leaving by their own act -, starting the session where it does not
/// run, as opening it does.
pub struct LetGo {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    participant: ActorId,
    from: Option<ChannelId>,
    going_out: GoingOut,
}

/// How a participant is let go.
enum GoingOut {
    /// Removed by whoever may change the session, or the channel.
    Removed,
    /// Leaving by their own act, or their own agent taken out.
    Leaving,
}

impl LetGo {
    /// Lets `participant` go on behalf of `context`, who may change the session - from it and every
    /// channel of it (`SessionLoop::remove_participant_held`) -, or with `from` the channel - from
    /// it alone (`ChannelLoop::remove_participant_held`).
    pub fn removed(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self::new(sessions, context, session, participant, from, GoingOut::Removed)
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - out by their own act:
    /// out of the session and every channel of it (`SessionLoop::leave_held`), or with `from` out
    /// of that channel alone (`ChannelLoop::leave_held`).
    pub fn leaving(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self::new(sessions, context, session, participant, from, GoingOut::Leaving)
    }

    fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
        going_out: GoingOut,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, participant, from, going_out }
    }
}

#[async_trait]
impl Procedure for LetGo {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    /// The channel it is let go from, or every channel of the session - also one that does not
    /// run, whose document letting go changes, and one that lists it only once it was founded
    /// into it meanwhile; a session it starts is locked exclusively, its channels with it.
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Participant(self.session, self.participant))?;
        let channels = match (self.from, self.sessions.running(self.session)) {
            (Some(channel), _) => vec![channel],
            (None, Some(running)) => running.session().channels.clone(),
            (None, None) => Vec::new(),
        };
        for channel in channels {
            locks.shared(Key::Channel(self.session, channel))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_for` for the channel named, and as
    /// the building block that lets them go.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { sessions, context, session, participant, from, going_out } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        match (from, going_out) {
            (None, GoingOut::Removed) => {
                session.remove_participant_held(admitted, &context, participant).await?
            }
            (None, GoingOut::Leaving) => {
                session.leave_held(admitted, &context, participant).await?
            }
            (Some(channel), going_out) => {
                let channel = session.channel_for(&context, channel).await?;
                match going_out {
                    GoingOut::Removed => {
                        channel.remove_participant_held(admitted, &context, participant).await?
                    }
                    GoingOut::Leaving => {
                        channel.leave_held(admitted, &context, participant).await?
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LetGo, TakeIn};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::testing::World;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::channel::HistoryAccess;
    use std::sync::Arc;

    /// Taking someone in and letting them go hold the participant exclusively, and the channel
    /// they are taken into or let go from shared - each waits for whoever holds either, and two
    /// changes of one participant do not cross.
    #[tokio::test]
    async fn taking_in_and_letting_go_hold_the_participant_and_its_channel() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[], &[], &[]).await;
        let (id, channel) = (session.id(), session.channels()[0].id());
        let bob = ActorId::User(world.bob);

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let into = Some((channel, HistoryAccess::All));
        let adding = TakeIn::added(&world.manager, &alice, id, bob, into);
        waits_for(held, &executor, adding).await.expect("taken in once it is let go");

        let held = Held::key(&executor, Key::Channel(id, channel), true).await;
        let removing = LetGo::removed(&world.manager, &alice, id, bob, Some(channel));
        waits_for(held, &executor, removing).await.expect("let go once it is let go");

        let held = Held::key(&executor, Key::Channel(id, channel), true).await;
        let adding = TakeIn::added(&world.manager, &alice, id, bob, into);
        waits_for(held, &executor, adding).await.expect("taken in again once it is let go");

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let removing = LetGo::removed(&world.manager, &alice, id, bob, None);
        waits_for(held, &executor, removing).await.expect("let go from the session as well");
        world.manager.stop(&alice, id).await.unwrap();
    }

    /// Taking someone into a running session holds it shared, as opening it does; one it starts
    /// it holds exclusively.
    #[tokio::test]
    async fn taking_in_holds_a_running_session_shared_and_one_it_starts_exclusively() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let id = world.session_of(&[], &[], &[]).await.id();

        let held = Held::key(&executor, Key::Session(id), false).await;
        let adding = TakeIn::added(&world.manager, &alice, id, ActorId::User(world.bob), None);
        executor.run(adding).await.expect("taken in beside it");
        held.release().await;

        world.manager.stop(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), false).await;
        let adding = TakeIn::added(&world.manager, &alice, id, ActorId::User(world.carol), None);
        waits_for(held, &executor, adding).await.expect("started and taken in once it is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }
}
