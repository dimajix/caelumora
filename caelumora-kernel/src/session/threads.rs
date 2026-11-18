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
    use crate::channel::testing::settled;
    use crate::channel::{ChannelError, ChannelLoop};
    use crate::testing::{Script, ScriptedAgents, World};
    use anyhow::Error;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::actor::ActorState;
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

    /// Issue 557: what is delegated into a thread its delegate gives the recipient to read there,
    /// and what the recipient answers comes back to the giver where the thread was opened; a
    /// thread delegated into takes no other giver, a brief no recipient outside the thread; and
    /// the delegate goes with its thread, which it keeps open no longer than its recipient.
    #[tokio::test]
    async fn what_is_delegated_comes_back_to_its_giver_and_its_delegate_goes_with_the_thread() {
        let answering = ScriptedAgents(Script::Answers("found it".to_string()));
        let world = World::with(&[], &[&answering], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let hers = ActorId::User(world.alice);
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let thread = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper])
            .await
            .expect("alice opens a thread with her agent");

        main.delegate(&alice, thread, helper, "look it up".to_string()).await.expect("delegated");
        let back = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == hers && message.sender_id == helper)
        };
        let entries = main.history_once(|entries| entries.iter().any(back)).await;
        let briefed = entries.iter().position(|e| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == helper && message.content.as_deref() == Some("look it up"))
        });
        let answered = entries.iter().position(
            |e| matches!(e, HistoryEntry::Said { message } if message.sender_id == helper),
        );
        let given_back = entries.iter().position(back);
        assert!(
            briefed.is_some() && briefed < answered && answered < given_back,
            "the brief given, answered, and the answer given back: {entries:?}"
        );
        let Some(HistoryEntry::GivenToRead { message, .. }) = given_back.map(|at| &entries[at])
        else {
            unreachable!("found above")
        };
        assert_eq!(message.content.as_deref(), Some("found it"), "what the turn said");
        let delegate = ActorId::Delegate(thread);
        assert!(main.actor(delegate).is_some(), "its delegate runs");
        assert!(!main.participant_ids().contains(&delegate), "and takes no part in the channel");
        assert!(main.mentions().lookup("delegate").is_empty(), "nor can it be mentioned");

        let bob = World::as_user(world.bob);
        let refused = main.delegate(&bob, thread, helper, "mine".to_string()).await.unwrap_err();
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::DelegatedOtherwise { .. })),
            "by another giver: {refused}"
        );
        let his = ActorId::User(world.bob);
        let refused = main.delegate(&alice, thread, his, "yours".to_string()).await.unwrap_err();
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::NotInThread { .. })),
            "to one outside the thread: {refused}"
        );
        let line = ThreadId::line(main.id());
        let refused = main.delegate(&alice, line, hers, "here".to_string()).await.unwrap_err();
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::ThreadNotOpen { .. })),
            "into the line, which nobody opened: {refused}"
        );

        session.remove_participant(&alice, helper).await.expect("let go");
        assert!(main.close_thread(&alice, thread).await.expect("closed"), "its delegate with it");
        main.history_once(|entries| {
            entries
                .iter()
                .any(|e| matches!(e, HistoryEntry::ThreadClosed { thread: t, .. } if *t == thread))
        })
        .await;
        assert!(main.actor(ActorId::Delegate(thread)).is_none(), "which is gone");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 557: what came of a brief that cannot reach its giver - gone from where the thread was
    /// opened - is dropped with the brief, and the brief that waited behind it is given.
    #[tokio::test]
    async fn a_brief_whose_giver_left_is_dropped_and_the_next_one_given() {
        let hanging = ScriptedAgents(Script::Hangs);
        let world = World::with(&[], &[&hanging], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let thread = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper])
            .await
            .expect("opened");
        let briefed = |text: &'static str| {
            move |e: &HistoryEntry| {
                matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                    if *recipient == helper && message.content.as_deref() == Some(text))
            }
        };
        main.delegate(&bob, thread, helper, "look it up".to_string()).await.expect("delegated");
        main.delegate(&bob, thread, helper, "and this".to_string()).await.expect("waits");
        main.history_once(|entries| entries.iter().any(briefed("look it up"))).await;

        session.leave(&bob, ActorId::User(world.bob)).await.expect("bob leaves");
        main.cancel_actor(&alice, helper).await.expect("its turn ends without an answer");
        let entries = main.history_once(|entries| entries.iter().any(briefed("and this"))).await;
        assert!(
            entries.iter().any(|e| matches!(e, HistoryEntry::EventRejected { .. })),
            "what came of the first could not reach bob: {entries:?}"
        );
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 557: a thread is cancelled on its own - the turns in it end, those of the line go on -
    /// and stays quiet - a brief into it refused, those that waited given back as not asked - until
    /// it is woken: by waking it or its agent, not by what is said in the line. A cancel of the
    /// channel reaches every thread; a thread the channel has not is neither cancelled nor woken.
    #[tokio::test]
    async fn a_thread_is_cancelled_and_woken_on_its_own() {
        let hanging = ScriptedAgents(Script::Hangs);
        let world = World::with(&[], &[&hanging], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let lead = world.agent("lead", world.alice, &personality).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let hers = ActorId::User(world.alice);
        let session = world.session_of(&[], &[lead], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let lead = ActorId::Agent(lead);
        let thread = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper])
            .await
            .expect("opened");
        main.delegate(&alice, thread, helper, "look it up".to_string()).await.expect("delegated");
        main.delegate(&alice, thread, helper, "and this".to_string()).await.expect("waits");
        let says = async |text: &str| {
            let user = main.user_actor(world.alice).expect("alice runs");
            user.send_message(Some(text.to_string()), None, Vec::new()).await.expect("said");
        };
        says("@lead, are you there?").await;
        for working in [helper, lead] {
            settled(&main.actor(working).expect("runs"), ActorState::Planning).await;
        }

        main.cancel(&alice, Some(thread)).await.expect("cancelled");
        let ended = |actor: ActorId| move |e: &HistoryEntry| matches!(e, HistoryEntry::TurnFinished { actor: a, .. } if *a == actor);
        let not_asked = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == hers
                    && message.content.as_deref() == Some("It could not be asked."))
        };
        let entries = main.history_once(|entries| entries.iter().any(not_asked)).await;
        assert!(entries.iter().any(ended(helper)), "its turn ended: {entries:?}");
        assert!(!entries.iter().any(ended(lead)), "the line's goes on: {entries:?}");
        let quiet = |refused: &Error| {
            matches!(channel_error(refused), Some(ChannelError::ThreadQuiet { .. }))
        };
        let brief = || main.delegate(&alice, thread, helper, "more".to_string());
        assert!(quiet(&brief().await.unwrap_err()), "a brief into it is refused");
        says("still there?").await;
        assert!(quiet(&brief().await.unwrap_err()), "what is said in the line wakes it not");

        main.wakeup_actor(&alice, helper).await.expect("its agent woken");
        brief().await.expect("which ends the quiet of its thread");
        main.cancel(&alice, None).await.expect("the whole channel cancelled");
        main.history_once(|entries| entries.iter().any(ended(lead))).await;
        assert!(quiet(&brief().await.unwrap_err()), "every thread with it");
        main.wakeup(&alice, Some(thread)).await.expect("woken");
        brief().await.expect("taken once it is woken");

        let elsewhere = Some(ThreadId::new());
        let not_open = |refused: Error| {
            matches!(channel_error(&refused), Some(ChannelError::ThreadNotOpen { .. }))
        };
        assert!(not_open(main.cancel(&alice, elsewhere).await.unwrap_err()), "no such thread");
        assert!(not_open(main.wakeup(&alice, elsewhere).await.unwrap_err()), "to wake either");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 557: a delegation is kept with the channel's state: started again, the channel starts
    /// its delegate, which gives its brief not again, and brings back what comes of it - here, that
    /// its recipient left.
    #[tokio::test]
    async fn a_delegate_carries_on_after_a_restart() {
        let hanging = ScriptedAgents(Script::Hangs);
        let world = World::with(&[], &[&hanging], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let hers = ActorId::User(world.alice);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let thread = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper])
            .await
            .expect("opened");
        main.delegate(&alice, thread, helper, "look it up".to_string()).await.expect("delegated");
        let briefed = |e: &HistoryEntry| matches!(e, HistoryEntry::GivenToRead { recipient, .. } if *recipient == helper);
        main.history_once(|entries| entries.iter().any(briefed)).await;

        world.manager.stop(&alice, session.id()).await.unwrap();
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        session.remove_participant(&alice, helper).await.expect("let go");
        let back = |e: &HistoryEntry| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == hers
                    && message.content.as_deref() == Some("It left before it answered."))
        };
        let entries = main.history_once(|entries| entries.iter().any(back)).await;
        assert_eq!(entries.iter().filter(|e| briefed(e)).count(), 1, "given once: {entries:?}");
        world.manager.stop(&alice, session.id()).await.unwrap();
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
