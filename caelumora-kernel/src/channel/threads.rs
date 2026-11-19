//! Opening and closing the threads of a running channel (issue 557): a thread is opened at a
//! message of another, with its founders taking part in it alone, closed once nobody takes part in
//! it any more, and built again from the channel's state as the channel starts.

use super::participants::{Entry, Exit};
use super::thread::Thread;
use super::{ActorChange, ChannelError, ChannelLoop};
use crate::procedure::Admitted;
use anyhow::Result;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{
    Channel, HistoryAccess, Opening, ThreadId, ThreadOrigin, ThreadState,
};
use caelumora_domain::store::Stored;
use caelumora_security::{ActionType, SecurityContext};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tracing::{info, warn};

impl ChannelLoop {
    /// Builds the channel's open threads as it starts (`ChannelState::threads`), its line among
    /// them, each with a moderator of its own.
    ///
    /// # Errors
    /// `SessionError::InvalidComponents` if the channel's components make no moderator.
    pub(super) async fn build_threads(
        &self,
        threads: &BTreeMap<ThreadId, ThreadState>,
    ) -> Result<()> {
        let mut built = HashMap::new();
        for (id, state) in threads.iter().filter(|(_, state)| !state.closed) {
            let thread = Thread::new(self.moderator().await?, state.opening);
            built.insert(*id, Arc::new(thread));
        }
        self.threads.store(Arc::new(built));
        Ok(())
    }

    /// Whether `thread` is open in the channel: something may be said in it, and a thread opened at
    /// it. Whoever closes it holds it until `run` closed it (`procedure::thread::Close`).
    pub(super) fn is_open(&self, thread: ThreadId) -> bool {
        self.thread(thread).is_some()
    }

    /// The thread `participant`, as the channel lists it, takes part in: where its actor runs, or
    /// else where its range of attendance is open, or else - one resting, a deleted agent - where
    /// it would take part again as it is restored (`rejoining`); `None` for one the channel does
    /// not list, or whose history cannot be read. For whoever lets them go or has them follow their
    /// agent, to hold that thread meanwhile, so that nobody closes it (issue 593).
    pub(crate) async fn thread_taken_part_in(&self, participant: ActorId) -> Option<ThreadId> {
        if !self.channel().lists(participant) {
            return None;
        }
        if let Some(actor) = self.actors.load().get(&participant) {
            return Some(actor.thread);
        }
        if let Some(&thread) = self.attending().await.get(&participant) {
            return Some(thread);
        }
        self.rejoining(participant).await.ok().map(|(_, thread)| thread)
    }

    /// Nothing if `context` may open a thread of the channel at `origin`: asked first, whether they
    /// may open one at all (`OpenThread`); then that a person opens one only at the channel's line
    /// (issue 557: a person branches once, deeper threads come from agents); then that the thread
    /// `origin` names is open. Whether its message is in that thread is not asked: a tool opens a
    /// thread while the message with its call may still be on its way into the channel, which
    /// records it before the thread's opening all the same (`open_thread_held`).
    ///
    /// # Errors
    /// `ChannelError::Forbidden`, `NotAtTheLine` or `ThreadNotOpen`.
    pub(crate) async fn require_opening(
        &self,
        context: &SecurityContext,
        origin: ThreadOrigin,
    ) -> Result<()> {
        self.require(
            context,
            |attributes| ActionType::OpenThread(attributes),
            "opening a thread in",
        )
        .await?;
        if matches!(context.caller_id, ActorId::User(_)) && origin.thread != ThreadId::line(self.id)
        {
            return Err(ChannelError::NotAtTheLine { channel: self.id }.into());
        }
        if !self.is_open(origin.thread) {
            let thread = origin.thread;
            return Err(ChannelError::ThreadNotOpen { thread, channel: self.id }.into());
        }
        Ok(())
    }

    /// Opens thread `thread` of the channel at `origin` on behalf of `context`, with `founders` -
    /// each a participant of the session, whom `context` acts for or an agent of their own -
    /// taking part in it alone. For `SessionLoop::open_thread_held` alone, which asked
    /// `require_opening` and holds the session's locks of the founders first (issue 590), for a
    /// procedure that holds the thread until its founders joined (issue 593).
    ///
    /// The thread is there at once, with a moderator of its own, and `run` announces it
    /// (`ThreadOpened`) in the name of `context`'s caller - after whatever was sent into the
    /// channel before, which `run` takes first, the message with the call that opened it among
    /// them - and before the founders' joins. They are taken in one after the other, as founders
    /// are: given what is said in the thread from their join on, stored in the kernel's name. If
    /// one cannot be, the opening is rolled back (`roll_back_opening`).
    ///
    /// # Errors
    /// What taking a founder in reports (`take_in`).
    pub(crate) async fn open_thread_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        thread: ThreadId,
        origin: ThreadOrigin,
        founders: &[ActorId],
    ) -> Result<()> {
        let opening = Opening { origin, by: context.caller_id };
        let opened = Arc::new(Thread::new(self.moderator().await?, Some(opening)));
        self.threads.rcu(|threads| {
            let mut threads = HashMap::clone(threads);
            threads.insert(thread, Arc::clone(&opened));
            threads
        });
        let _ = self.request(ActorChange::Open(thread, opening, context.clone()));
        let mut founded = Vec::with_capacity(founders.len());
        for founder in founders {
            let founding =
                self.take_in(context, *founder, HistoryAccess::None, Entry::Founding, thread).await;
            if let Err(error) = founding {
                self.roll_back_opening(context, thread, &founded).await;
                return Err(error);
            }
            founded.push(*founder);
        }
        info!("Thread {} opened in channel {} by {}", thread, self.id, context.caller_id);
        Ok(())
    }

    /// Rolls back opening `thread`, once one of its founders could not be taken in: those that
    /// were, `founded`, are let go as they are let go with the session - in the kernel's name, kept
    /// as removed by `context`'s caller -, and the thread is closed. What cannot be rolled back is
    /// logged and stays.
    async fn roll_back_opening(
        &self,
        context: &SecurityContext,
        thread: ThreadId,
        founded: &[ActorId],
    ) {
        for founder in founded {
            if let Err(undo) = self.let_go(context, *founder, Exit::WithTheSession).await {
                warn!(
                    "{} founded thread {} of channel {}, which could not be opened, and was not \
                     let go again: {}",
                    founder, thread, self.id, undo
                );
            }
        }
        let _ = self.request(ActorChange::Close(thread, context.clone())).carried_out().await;
    }

    /// Closes `thread` on behalf of `context` (issue 557), for a procedure that holds it
    /// (`procedure::thread::Close`): nothing more is said in it. Asked of whoever opened it - the
    /// same caller - or else whoever may change the channel (`UpdateChannel`); refused while anyone
    /// takes part in it - an actor running in it, or a participant listed without one whose range
    /// of attendance is open there -, so that nobody is left in a thread closed: whoever takes part
    /// is let go first, and letting them go holds the thread until they have left (issue 593).
    /// `run` closes it, announces it (`ThreadClosed`) in the name of `context`'s caller and keeps
    /// it as closed in the channel's state, and answers then; from then on what is sent into it is
    /// not let in (`Rejection::ThreadClosed`). `false` if it is not open - never opened, closed
    /// already, or the channel's line, which is never closed. An actor that does not end even when
    /// aborted has letting it go give up after `LEAVE_GRACE` (`left`): its leave, announced late,
    /// may then follow the closing - logged, as such an actor is a bug.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if `context` may not close it, `ThreadNotEmpty` if anyone takes
    /// part in it.
    pub async fn close_thread_held(
        &self,
        _: &Admitted<'_>,
        context: &SecurityContext,
        thread: ThreadId,
    ) -> Result<bool> {
        let Some(open) = self.thread(thread) else { return Ok(false) };
        let Some(opening) = open.opening() else { return Ok(false) };
        if opening.by != context.caller_id {
            self.require(
                context,
                |attributes| ActionType::update(Channel::KIND, attributes),
                "closing a thread of",
            )
            .await?;
        }
        // Its delegate goes with it (issue 557).
        let delegate = ActorId::Delegate(thread);
        let running =
            self.actors.load().iter().any(|(id, actor)| actor.thread == thread && *id != delegate);
        let attending = self.attending().await;
        let listed = self.channel();
        let resting = listed
            .participants
            .iter()
            .any(|participant| attending.get(&participant.actor_id()) == Some(&thread));
        if running || resting {
            return Err(ChannelError::ThreadNotEmpty { thread, channel: self.id }.into());
        }
        let _ = self.request(ActorChange::Close(thread, context.clone())).carried_out().await;
        info!("Thread {} of channel {} is closed by {}", thread, self.id, context.caller_id);
        Ok(true)
    }
}
