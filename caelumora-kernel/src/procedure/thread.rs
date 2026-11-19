//! The procedures of a thread's lifecycle (issue 593): opening a thread of a channel with its
//! founders, and closing it once nobody takes part in it.
//!
//! Opening holds the new thread exclusively - its id chosen before -, the thread it is opened at
//! shared, so that it is not closed meanwhile, and its founders; closing holds the thread
//! exclusively. Whoever lets someone go holds the thread they take part in shared, until they have
//! left (`participant::LetGo`), so that a thread is closed only once whoever left it has left. Both
//! open the session as opening it does, and carry the change out by the building blocks of the
//! `SessionLoop` and the `ChannelLoop`, which ask for the proof that they hold them (`Admitted`) -
//! as a plugin's procedures may, which open or close a thread within a change of their own (issue
//! 558's sub-agents).

use super::session::lock_to_open;
use super::{Admitted, Compensation, Deferred, Key, Locks, Procedure};
use crate::session::{SessionError, SessionManager};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, ThreadId, ThreadOrigin};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::sync::Arc;

/// Opens a thread of a channel at `origin` on behalf of `context`, starting the session where it
/// does not run, its founders - whom `context` acts for, or their own agents - taking part in it
/// alone (`SessionLoop::open_thread_held`). Holds the new thread, its id chosen before it is
/// opened, until its founders are taken in.
pub struct Open {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    /// Chosen before it is opened, so that it is held before anyone can find it.
    thread: ThreadId,
    origin: ThreadOrigin,
    founders: Vec<ActorId>,
}

impl Open {
    /// Opens a thread of `channel` of `session` at `origin` on behalf of `context`, with
    /// `founders`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        origin: ThreadOrigin,
        founders: Vec<ActorId>,
    ) -> Self {
        let (sessions, context, thread) = (Arc::clone(sessions), context.clone(), ThreadId::new());
        Self { sessions, context, session, channel, thread, origin, founders }
    }
}

#[async_trait]
impl Procedure for Open {
    type Plan = ();
    type Output = ThreadId;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))?;
        locks.shared(Key::Thread(self.session, self.channel, self.origin.thread))?;
        for founder in &self.founders {
            locks.exclusive(Key::Participant(self.session, *founder))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `SessionManager::open_held` and `SessionLoop::open_thread_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<ThreadId, SessionError> {
        let Self { sessions, context, session, channel, thread, origin, founders } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        session.open_thread_held(admitted, &context, channel, thread, origin, founders).await?;
        Ok(thread)
    }
}

/// Closes a thread of a channel on behalf of `context`, starting the session where it does not
/// run, once nobody takes part in it (`ChannelLoop::close_thread_held`); holds it until `run` has
/// closed it. `false` if it is not open.
pub struct Close {
    sessions: Arc<SessionManager>,
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
}

impl Close {
    /// Closes `thread` of `channel` of `session` on behalf of `context`.
    pub fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, channel, thread }
    }
}

#[async_trait]
impl Procedure for Close {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))
    }

    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_for` and
    /// `ChannelLoop::close_thread_held`.
    async fn carry_out(
        self,
        (): (),
        admitted: &Admitted<'_>,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { sessions, context, session, channel, thread } = self;
        let session = sessions.open_held(admitted, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        Ok(channel.close_thread_held(admitted, &context, thread).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{Close, Open};
    use crate::procedure::participant::LetGo;
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::testing::World;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::channel::{ThreadId, ThreadOrigin};
    use caelumora_domain::message::MessageId;
    use std::sync::Arc;

    /// Opening a thread holds the thread it is opened at, so that it is not closed meanwhile;
    /// closing one waits for whoever holds it - letting someone go from it holds it shared, so
    /// that it is closed only once they have left, and letting go waits for a closing.
    #[tokio::test]
    async fn opening_and_closing_a_thread_hold_it_and_letting_go_holds_it_too() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let (id, main) = (session.id(), session.channels()[0].id());
        let line = ThreadId::line(main);
        let at_the_line = ThreadOrigin { thread: line, message: MessageId::new() };

        let held = Held::key(&executor, Key::Thread(id, main, line), true).await;
        let opening = Open::new(&world.manager, &alice, id, main, at_the_line, vec![]);
        let thread = waits_for(held, &executor, opening).await.expect("opened once it is let go");

        let held = Held::key(&executor, Key::Thread(id, main, thread), false).await;
        let closing = Close::new(&world.manager, &alice, id, main, thread);
        assert!(waits_for(held, &executor, closing).await.unwrap(), "closed once it is let go");

        let held = Held::key(&executor, Key::Thread(id, main, line), true).await;
        let bob = ActorId::User(world.bob);
        let removing = LetGo::removed(&world.manager, &alice, id, bob, Some(main));
        waits_for(held, &executor, removing).await.expect("let go once its thread is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }
}
