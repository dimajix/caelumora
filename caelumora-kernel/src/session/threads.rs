//! Opening a thread of one of the session's channels (issue 557): its founders taken into the
//! session and the channel, through the session, which holds each participant's lock first (issue
//! 590).

use super::SessionLoop;
use crate::channel::{ChannelError, Entry};
use anyhow::Result;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, ThreadId, ThreadOrigin};
use caelumora_domain::session::Session;
use caelumora_security::SecurityContext;

impl SessionLoop {
    /// Opens a thread of `channel` at `origin` on behalf of `context` (issue 557), with its
    /// founders, `founders` - whom `context` acts for, or agents of their own -, taking part in it
    /// alone, and returns its id. Asked first whether `context` may open it there
    /// (`ChannelLoop::require_opening`), then of each founder - held in one order, as wherever
    /// several are (`changing`) - that it takes no part in the channel yet, since an actor takes
    /// part in one thread of its channel. Each is taken into the session, unless in it already, as
    /// a founder: asked whether it is `context`'s own and can be taken in, and stored in the
    /// kernel's name, opening the thread being permitted; then the channel opens the thread with
    /// them (`ChannelLoop::open_thread`). One that cannot be taken in, into the session or the
    /// channel, rolls back those this call took into the session (`roll_back_taking_in`).
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` for a channel `context` may not read; as
    /// `ChannelLoop::require_opening`; `ChannelError::AlreadyParticipant` for a founder taking
    /// part in the channel; `StoreError::PermissionDenied` for one not `context`'s own; as
    /// `take_in` and `ChannelLoop::open_thread` otherwise.
    pub async fn open_thread(
        &self,
        context: &SecurityContext,
        channel: ChannelId,
        origin: ThreadOrigin,
        founders: Vec<ActorId>,
    ) -> Result<ThreadId> {
        let channel = self.channel_for(context, channel).await?;
        channel.require_opening(context, origin).await?;
        let mut founders = founders;
        founders.sort_by_key(ActorId::id);
        founders.dedup();
        let _changing = self.changing.lock_all(founders.iter().copied(), ActorId::id).await;
        let listed = channel.channel();
        if let Some(&participant) = founders.iter().find(|founder| listed.lists(**founder)) {
            return Err(
                ChannelError::AlreadyParticipant { participant, channel: channel.id() }.into()
            );
        }
        let mut taken: Vec<(ActorId, Option<Session>)> = Vec::with_capacity(founders.len());
        for founder in &founders {
            match self.take_in(context, *founder, Some(&channel), Entry::Founding).await {
                Ok(before) => taken.push((*founder, before)),
                Err(error) => {
                    self.roll_back_founding(&taken).await;
                    return Err(error);
                }
            }
        }
        match channel.open_thread(context, origin, &founders).await {
            Ok(thread) => Ok(thread),
            Err(error) => {
                self.roll_back_founding(&taken).await;
                Err(error)
            }
        }
    }

    /// Rolls back taking the founders of a thread into the session, `taken` with the session as it
    /// was before each (`roll_back_taking_in`), the last first.
    async fn roll_back_founding(&self, taken: &[(ActorId, Option<Session>)]) {
        for (founder, before) in taken.iter().rev() {
            self.roll_back_taking_in(*founder, before.as_ref()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::{ChannelError, ChannelLoop};
    use crate::testing::{Script, ScriptedAgents, World};
    use anyhow::Error;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::channel::{ThreadId, ThreadOrigin};
    use caelumora_domain::history::HistoryEntry;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::store::StoreError;
    use std::sync::Arc;

    /// At the line of `channel`, at a message the test names.
    fn at_the_line(channel: &ChannelLoop) -> ThreadOrigin {
        ThreadOrigin { thread: ThreadId::line(channel.id()), message: MessageId::new() }
    }

    fn channel_error(error: &Error) -> Option<&ChannelError> {
        error.downcast_ref::<ChannelError>()
    }

    /// Issue 557: a thread is opened with its founders taking part in it alone - taken into the
    /// session and the channel, its opening recorded before their joins -, is there again with
    /// them after a restart, and is closed once they were let go.
    #[tokio::test]
    async fn a_thread_is_opened_with_its_founders_and_closed_once_they_are_let_go() {
        let answering = ScriptedAgents(Script::Answers("ok".to_string()));
        let world = World::with(&[], &[&answering], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);

        let thread = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper])
            .await
            .expect("alice opens a thread with her agent");
        assert!(session.session().takes_part(helper), "taken into the session");
        assert!(main.channel().lists(helper), "and into the channel");
        let joined = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::ParticipantJoined { participant, .. }
                if *participant == helper)
        };
        let entries = main.history_once(|entries| entries.iter().any(joined)).await;
        let opened = entries.iter().position(
            |e| matches!(e, HistoryEntry::ThreadOpened { thread: t, .. } if *t == thread),
        );
        let joined = entries.iter().position(joined);
        assert!(opened.is_some() && opened < joined, "opened, then joined: {entries:?}");
        assert_eq!(
            main.attendance(helper).await.last().map(|at| at.thread),
            Some(thread),
            "it takes part in the thread"
        );

        world.manager.stop(&alice, session.id()).await.unwrap();
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        let origin = ThreadOrigin { thread, message: MessageId::new() };
        main.require_opening(&alice, origin).await.expect_err("a person branches at the line");
        let refused = main.close_thread(&alice, thread).await.expect_err("it takes part in it");
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::ThreadNotEmpty { .. })),
            "open again after the restart, with its founder in it: {refused}"
        );

        session.remove_participant(&alice, helper).await.expect("let go");
        assert!(main.close_thread(&alice, thread).await.expect("closed"), "by whoever opened it");
        let closed = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::ThreadClosed { thread: t, .. }
                if *t == thread)
        };
        let entries = main.history_once(|entries| entries.iter().any(closed)).await;
        let left = entries.iter().position(|e| {
            matches!(e, HistoryEntry::ParticipantLeft { participant, .. } if *participant == helper)
        });
        let closed = entries.iter().position(closed);
        assert!(left.is_some() && left < closed, "closed once it has left: {entries:?}");
        world.manager.stop(&alice, session.id()).await.unwrap();
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        assert!(!main.close_thread(&alice, thread).await.unwrap(), "closed for good");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 557: a founder is whom the opener acts for, or their own agent, and new to the
    /// channel; one refused takes back those taken into the session before it. A person opens a
    /// thread at the line alone.
    #[tokio::test]
    async fn a_thread_is_founded_by_the_openers_own_who_take_no_part_in_the_channel_yet() {
        let answering = ScriptedAgents(Script::Answers("ok".to_string()));
        let world = World::with(&[], &[&answering], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let carol = ActorId::User(world.carol);

        let refused = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper, carol])
            .await
            .expect_err("carol is not alice's own");
        assert!(
            matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_))),
            "{refused}"
        );
        assert!(
            !session.session().takes_part(helper) && !session.session().takes_part(carol),
            "nobody stays taken in"
        );

        let alice_id = ActorId::User(world.alice);
        let refused = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![alice_id])
            .await
            .expect_err("alice takes part in the channel");
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::AlreadyParticipant { .. })),
            "{refused}"
        );

        let elsewhere = ThreadOrigin { thread: ThreadId::new(), message: MessageId::new() };
        let refused = session
            .open_thread(&alice, main.id(), elsewhere, vec![helper])
            .await
            .expect_err("a person branches at the line");
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::NotAtTheLine { .. })),
            "{refused}"
        );
        assert!(!session.session().takes_part(helper), "nothing taken in");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
}
