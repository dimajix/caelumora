//! The procedures of a thread's lifecycle (issue 593): opening a thread of a channel with its
//! founders, and closing it once nobody takes part in it.
//!
//! Opening holds the new thread exclusively - its id chosen before -, the thread it is opened at
//! shared, so that it is not closed meanwhile, and its founders; closing holds the thread
//! exclusively. Whoever lets someone go holds the thread they take part in shared, until they have
//! left (`participant::LetGo`), so that a thread is closed only once whoever left it has left. Both
//! open the session as opening it does; the `ChannelLoop` opens and closes the thread, enforcing
//! who may (`ChannelLoop::open_thread`, `close_thread`) - as a plugin's procedures may have it,
//! which open or close a thread within a change of their own (issue 558's sub-agents).

use super::participant::{take_into_channel, take_into_session, Entry};
use super::session::{self, lock_to_open};
use super::{Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::{require_opening, ChannelError};
use crate::kernel::Kernel;
use crate::session::{SessionError, SessionLoop};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{ChannelId, HistoryAccess, ThreadId, ThreadOrigin};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::collections::HashSet;
use std::sync::Arc;

/// Opens a thread of a channel at `origin` on behalf of `context`, starting the session where it
/// does not run, its founders - whom `context` acts for, or their own agents - taking part in it
/// alone (`open`). Holds the new thread, its id chosen before it is
/// opened, until its founders are taken in.
pub struct Open {
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
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        origin: ThreadOrigin,
        founders: Vec<ActorId>,
    ) -> Self {
        let context = context.clone();
        Self { context, session, channel, thread: ThreadId::new(), origin, founders }
    }
}

#[async_trait]
impl Procedure for Open {
    type Plan = ();
    type Output = ThreadId;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))?;
        locks.shared(Key::Thread(self.session, self.channel, self.origin.thread))?;
        for founder in &self.founders {
            locks.exclusive(Key::Participant(self.session, *founder))?;
        }
        Ok(())
    }

    /// # Errors
    /// As `session::open` and `open`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<ThreadId, SessionError> {
        let Self { context, session, channel, thread, origin, founders } = self;
        let session = session::open(kernel, &context, session).await?;
        open(log, kernel, &session, &context, channel, thread, origin, founders).await?;
        Ok(thread)
    }
}

/// Opens thread `thread` of `channel` of `session` at `origin` on behalf of `context`, with its
/// founders, `founders` - whom `context` acts for, or agents of their own -, taking part in it
/// alone, for a procedure that holds the thread, the one it is opened at and its founders (`Open`),
/// or a founder nobody knows yet, an agent just made (the sub-agents' start, issue 558). Everything
/// is asked before anything is stored: whether `context` may open it there (`require_opening`,
/// asked ahead of the channel, which asks it again as it opens it), that the channel lists no
/// founder yet, and what taking each in asks, as each is taken into the session
/// (`participant::take_into_session`); then the thread is opened (`ChannelLoop::open_thread`) and
/// each founder taken into the channel as one (`participant::take_into_channel`). Each listing and
/// the opening are taken back by `log`, the last first, if a later step fails - a founder as if
/// never taken in, the thread closed in the name of `context`, who opened it
/// (`ChannelLoop::close_thread`).
///
/// # Errors
/// `SessionError::ChannelNotFound` for a channel `context` may not read; as `require_opening`;
/// `ChannelError::AlreadyParticipant` for a founder the channel lists; as
/// `participant::take_into_session`, `ChannelLoop::open_thread` and
/// `participant::take_into_channel` otherwise.
#[expect(clippy::too_many_arguments, reason = "the thread, where, by whom, with whom, in what")]
pub async fn open(
    log: &mut Compensation,
    kernel: &Kernel,
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    channel: ChannelId,
    thread: ThreadId,
    origin: ThreadOrigin,
    founders: Vec<ActorId>,
) -> Result<(), SessionError> {
    let channel = session.channel_for(context, channel).await?;
    let open = channel.is_open(origin.thread);
    let access = kernel.access_policy();
    require_opening(access.as_ref(), context, &channel.channel(), origin, open).await?;
    // Each once, in the order named.
    let mut named = HashSet::new();
    let mut founders = founders;
    founders.retain(|founder| named.insert(*founder));
    // An actor takes part in one thread of its channel: a founder is new to it.
    let listed = channel.channel();
    if let Some(&participant) = founders.iter().find(|founder| listed.lists(**founder)) {
        let taking_part = ChannelError::AlreadyParticipant { participant, channel: channel.id() };
        return Err(anyhow::Error::from(taking_part).into());
    }
    let founding = Entry::FoundingThread;
    let into = Some(&channel);
    let mut asked = Vec::with_capacity(founders.len());
    for founder in founders {
        let taking_in = take_into_session(log, kernel, session, context, founder, founding, into);
        asked.push(taking_in.await?);
    }
    channel.open_thread(context, thread, origin).await?;
    {
        let (channel, context) = (Arc::clone(&channel), context.clone());
        log.record("opening a thread", async move {
            channel.close_thread(&context, thread).await?;
            Ok(())
        });
    }
    let line = (&channel, HistoryAccess::None, thread);
    for founder in asked {
        take_into_channel(log, session, context, founder, founding, line).await?;
    }
    Ok(())
}

/// Closes a thread of a channel on behalf of `context`, starting the session where it does not
/// run, once nobody takes part in it (`ChannelLoop::close_thread`); holds it until `run` has
/// closed it. `false` if it is not open.
pub struct Close {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
}

impl Close {
    /// Closes `thread` of `channel` of `session` on behalf of `context`.
    pub fn new(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
    ) -> Self {
        Self { context: context.clone(), session, channel, thread }
    }
}

#[async_trait]
impl Procedure for Close {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Thread(self.session, self.channel, self.thread))
    }

    /// # Errors
    /// As `session::open`, `SessionLoop::channel_for` and `ChannelLoop::close_thread`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { context, session, channel, thread } = self;
        let session = session::open(kernel, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        Ok(channel.close_thread(&context, thread).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{Close, Open};
    use crate::channel::testing::settled;
    use crate::channel::{require_opening, ChannelError, ChannelLoop};
    use crate::procedure::participant::LetGo;
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::testing::{Script, ScriptedAgent, ScriptedAgents, World};
    use anyhow::Error;
    use caelumora_api::actor::{agent_factory, Actor};
    use caelumora_api::library::{Library, LibraryBuilder};
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::actor::ActorState;
    use caelumora_domain::agent::PersonalityKind;
    use caelumora_domain::channel::{ThreadId, ThreadOrigin};
    use caelumora_domain::history::HistoryEntry;
    use caelumora_domain::message::MessageId;
    use caelumora_domain::store::StoreError;
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
        let opening = Open::new(&alice, id, main, at_the_line, vec![]);
        let thread = waits_for(held, &executor, opening).await.expect("opened once it is let go");

        let held = Held::key(&executor, Key::Thread(id, main, thread), false).await;
        let closing = Close::new(&alice, id, main, thread);
        assert!(waits_for(held, &executor, closing).await.unwrap(), "closed once it is let go");

        let held = Held::key(&executor, Key::Thread(id, main, line), true).await;
        let bob = ActorId::User(world.bob);
        let removing = LetGo::removed(&alice, id, bob, Some(main));
        waits_for(held, &executor, removing).await.expect("let go once its thread is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }

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
        let held = main.delegation(thread).await.expect("delegated into");
        assert_eq!((held.giver, held.recipient), (hers, helper), "by alice, to her agent");
        assert!(held.briefs.is_empty(), "its brief answered");
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
        assert!(
            main.close_thread_by_procedure(&alice, thread).await.expect("closed"),
            "its delegate with it"
        );
        main.history_once(|entries| {
            entries
                .iter()
                .any(|e| matches!(e, HistoryEntry::ThreadClosed { thread: t, .. } if *t == thread))
        })
        .await;
        assert!(main.actor(ActorId::Delegate(thread)).is_none(), "which is gone");
        assert!(main.delegation(thread).await.is_none(), "with what was delegated");
        assert!(main.delegation(line).await.is_none(), "and nothing is delegated into the line");
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
        let access = world.kernel.access_policy();
        let open = main.is_open(origin.thread);
        let document = main.channel();
        let opening = require_opening(access.as_ref(), &alice, &document, origin, open);
        opening.await.expect_err("a person branches at the line");
        let refused =
            main.close_thread_by_procedure(&alice, thread).await.expect_err("it takes part in it");
        assert!(
            matches!(channel_error(&refused), Some(ChannelError::ThreadNotEmpty { .. })),
            "open again after the restart, with its founder in it: {refused}"
        );

        session.remove_participant(&alice, helper).await.expect("let go");
        assert!(
            main.close_thread_by_procedure(&alice, thread).await.expect("closed"),
            "by whoever opened it"
        );
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
        assert!(!main.close_thread_by_procedure(&alice, thread).await.unwrap(), "closed for good");
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

    /// Scripted agents that answer, but the one named "broken", which cannot start.
    #[derive(Debug)]
    struct OneBroken;

    impl Library for OneBroken {
        fn name(&self) -> &'static str {
            "one_broken"
        }

        fn register(&self, library: &mut dyn LibraryBuilder) -> anyhow::Result<()> {
            let factory = agent_factory(|runtime| async move {
                anyhow::ensure!(runtime.agent().name.as_str() != "broken", "it cannot start");
                let answers = Script::Answers("ok".to_string());
                Ok(Box::new(ScriptedAgent::new(runtime, answers)) as Box<dyn Actor>)
            });
            library.register_agent_kind(PersonalityKind::Internal, &[], factory)
        }
    }

    /// Issue 593: a founder that cannot start after another was taken in and started has the
    /// opening taken back by its compensation - the other taken out of the channel and the session
    /// as if never taken in, no former participant, its join and leave in the history -, and the
    /// thread closed.
    #[tokio::test]
    async fn a_founder_that_cannot_start_takes_back_the_opening_and_whoever_was_founded() {
        let world = World::with(&[], &[&OneBroken], |_| {}).await;
        let personality = world.personality("helper", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &personality).await);
        let broken = ActorId::Agent(world.agent("broken", world.alice, &personality).await);
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);

        let refused = session
            .open_thread(&alice, main.id(), at_the_line(&main), vec![helper, broken])
            .await
            .expect_err("the second cannot start");
        assert!(refused.to_string().contains("cannot start"), "{refused}");
        let listed = main.channel();
        assert!(!listed.lists(helper) && !listed.lists(broken), "out of the channel");
        assert!(listed.former.iter().all(|f| f.participant.actor_id() != helper), "never in");
        assert!(
            !session.session().takes_part(helper) && !session.session().takes_part(broken),
            "and out of the session"
        );
        let entries = main.history_entries().await;
        let joined = |e: &HistoryEntry| matches!(e, HistoryEntry::ParticipantJoined { participant, .. } if *participant == helper);
        let left = |e: &HistoryEntry| matches!(e, HistoryEntry::ParticipantLeft { participant, .. } if *participant == helper);
        assert!(entries.iter().any(joined) && entries.iter().any(left), "{entries:#?}");
        let closed = |e: &HistoryEntry| matches!(e, HistoryEntry::ThreadClosed { .. });
        assert!(entries.iter().any(closed), "the thread closed again");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
}
