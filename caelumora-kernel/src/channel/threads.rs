//! Opening and closing the threads of a running channel (issue 557): a thread is opened at a
//! message of another, with its founders, closed once nobody takes part in it any more, and built
//! again from the channel's state as the channel starts.

use super::thread::Thread;
use super::{ActorChange, Answer, ChannelError, ChannelLoop};
use anyhow::Result;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{Channel, Circle, Opening, ThreadId, ThreadOrigin, ThreadState};
use caelumora_domain::store::Stored;
use caelumora_security::{AccessPolicy, ActionType, Enforcement, SecurityContext};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tracing::info;

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

    /// Where `thread` hangs (issue 602): the message of another thread it was opened at; `None` for
    /// the line, or a thread the channel does not hold open.
    pub(crate) fn origin_of(&self, thread: ThreadId) -> Option<ThreadOrigin> {
        self.thread(thread)?.opening().map(|opening| opening.origin)
    }

    /// Whether `thread` is open in the channel: something may be said in it, and a thread opened at
    /// it. Whoever closes it holds it until `run` closed it (`procedure::thread::Close`).
    pub(crate) fn is_open(&self, thread: ThreadId) -> bool {
        self.thread(thread).is_some()
    }

    /// The threads `participant`, as the channel lists it, takes part in: where its actor runs, or
    /// else where its ranges of attendance are open, or else - one resting, a deleted agent - where
    /// it would take part again as it is restored (`rejoining`); none for one the channel does not
    /// list, or whose history cannot be read. For whoever lets them go or has them follow their
    /// agent, to hold those threads meanwhile, so that nobody closes them (issues 593, 602).
    pub(crate) async fn threads_taken_part_in(&self, participant: ActorId) -> BTreeSet<ThreadId> {
        if !self.channel().lists(participant) {
            return BTreeSet::new();
        }
        if let Some(actor) = self.actors.load().get(&participant) {
            return BTreeSet::clone(&actor.threads.load());
        }
        if let Some(threads) = self.attending().await.remove(&participant) {
            return threads;
        }
        self.rejoining(participant).await.map(|(_, threads)| threads).unwrap_or_default()
    }

    /// Opens thread `thread` of the channel at `origin`, its circle `circle`, on behalf of
    /// `context`, if they may (`require_opening`), for a procedure that holds it
    /// (`procedure::thread::open`), which asked it ahead and takes its founders in after: the
    /// thread is there at once, with a moderator of its own, and `run` announces it
    /// (`ThreadOpened`) in the name of `context`'s caller - after whatever was sent into the
    /// channel before, which `run` takes first, the message with the call that opened it among
    /// them - and before the founders' joins, which are handed to it after.
    ///
    /// # Errors
    /// As `require_opening`; as building its moderator: `SessionError::InvalidComponents` if the
    /// channel's components make none.
    pub(crate) async fn open_thread(
        &self,
        context: &SecurityContext,
        thread: ThreadId,
        origin: ThreadOrigin,
        circle: Circle,
    ) -> Result<()> {
        let open = self.is_open(origin.thread);
        let access = self.policies.access_policy();
        require_opening(access.as_ref(), context, &self.channel(), origin, open).await?;
        let opening = Opening { origin, by: context.caller_id, circle };
        let opened = Arc::new(Thread::new(self.moderator().await?, Some(opening)));
        self.threads.rcu(|threads| {
            let mut threads = HashMap::clone(threads);
            threads.insert(thread, Arc::clone(&opened));
            threads
        });
        let _ = self.request(ActorChange::Open(thread, opening, context.clone()));
        info!("Thread {} opened in channel {} by {}", thread, self.id, context.caller_id);
        Ok(())
    }

    /// Takes into `thread`, just opened, the author of the message it hangs at, as far as the
    /// thread draws them in (issue 602, `ChannelLoop::draw_in`): where its circle is open, one who
    /// takes part in the channel - so that whoever opened it at a message asks its author there.
    /// For the procedure that opened it, once its founders are in (`procedure::thread::open`); the
    /// join is announced in the name of `context`'s caller, who opened it. What `run` answers once
    /// it drew them in - nobody for a message not in the history.
    pub(crate) async fn draw_in_author(
        &self,
        context: &SecurityContext,
        thread: ThreadId,
    ) -> Answer {
        let author = match self.origin_of(thread) {
            Some(origin) => self.message(origin.message).await.map(|said| said.sender_id),
            None => None,
        };
        self.request(ActorChange::DrawIn(thread, author.into_iter().collect(), context.clone()))
    }

    /// Closes `thread` on behalf of `context` (issue 557), for a procedure that holds it
    /// (`procedure::thread::Close`), or takes back opening it (`procedure::thread::open`): nothing
    /// more is said in it. Asked of whoever opened it - the same caller - or else whoever may
    /// change the channel (`UpdateChannel`); refused while anyone takes part in it - an actor
    /// running in it, or a participant listed without one whose range of attendance is open
    /// there -, so that nobody is left in a thread closed: whoever takes part is let go first, and
    /// letting them go holds the thread until they have left (issue 593). `run` closes it,
    /// announces it (`ThreadClosed`) in the name of `context`'s caller and keeps it as closed in
    /// the channel's state, and answers then; from then on what is sent into it is not let in
    /// (`Rejection::ThreadClosed`) - unless it drew someone in since it was asked (issue 602),
    /// which `run` asks again. `false` if it is not open - never opened, closed already, or the
    /// channel's line, which is never closed -, or the channel stopped before closing it. An actor
    /// that does not end even when aborted has letting it go give up after `LEAVE_GRACE` (`left`):
    /// its leave, announced late, may then follow the closing - logged, as such an actor is a bug.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if `context` may not close it, `ThreadNotEmpty` if anyone takes
    /// part in it.
    pub async fn close_thread(&self, context: &SecurityContext, thread: ThreadId) -> Result<bool> {
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
        let attending = self.attending().await;
        let listed = self.channel();
        let resting = listed.participants.iter().any(|participant| {
            attending.get(&participant.actor_id()).is_some_and(|there| there.contains(&thread))
        });
        let not_empty = || ChannelError::ThreadNotEmpty { thread, channel: self.id };
        if self.runs_anyone_in(thread) || resting {
            return Err(not_empty().into());
        }
        let closing = self.request(ActorChange::Close(thread, context.clone()));
        if !closing.carried_out().await {
            // Someone it drew in meanwhile keeps it open (issue 602), else the channel stopped.
            return if self.runs_anyone_in(thread) { Err(not_empty().into()) } else { Ok(false) };
        }
        info!("Thread {} of channel {} is closed by {}", thread, self.id, context.caller_id);
        Ok(true)
    }

    /// Whether an actor runs in `thread` and takes part in it, its delegate aside, which goes as it
    /// is closed (issue 557): what keeps it from being closed, asked as closing is asked and again
    /// as `run` closes it, since it may have drawn someone in meanwhile (issue 602).
    pub(super) fn runs_anyone_in(&self, thread: ThreadId) -> bool {
        let delegate = ActorId::Delegate(thread);
        let actors = self.actors.load();
        actors.iter().any(|(id, actor)| *id != delegate && actor.threads.load().contains(&thread))
    }
}

/// Nothing if `context` may open a thread of `channel` at `origin`, the thread `origin` names open
/// as `open` says - asked by `ChannelLoop::open_thread` as it opens it, and ahead by
/// `procedure::thread::open`, before its founders are taken into the session: first whether they
/// may open one at all (`OpenThread`, asked of `access`); then that a person opens one only at the
/// channel's line (issue 557: a person branches once, deeper threads come from agents); then that
/// the thread `origin` names is open. Whether its message is in that thread is not asked: a tool
/// opens a thread while the message with its call may still be on its way into the channel, which
/// records it before the thread's opening all the same.
///
/// # Errors
/// `ChannelError::Forbidden`, `NotAtTheLine` or `ThreadNotOpen`.
pub(crate) async fn require_opening(
    access: &dyn AccessPolicy,
    context: &SecurityContext,
    channel: &Channel,
    origin: ThreadOrigin,
    open: bool,
) -> Result<()> {
    let id = channel.id;
    access
        .require(context, ActionType::OpenThread(&channel.security_attributes), |refused| {
            ChannelError::Forbidden { action: "opening a thread in", channel: id, refused }
        })
        .await?;
    if matches!(context.caller_id, ActorId::User(_)) && origin.thread != ThreadId::line(id) {
        return Err(ChannelError::NotAtTheLine { channel: id }.into());
    }
    if !open {
        return Err(ChannelError::ThreadNotOpen { thread: origin.thread, channel: id }.into());
    }
    Ok(())
}
