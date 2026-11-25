//! The procedures of a participant's lifecycle (issue 593): taking someone into a session - and
//! into a channel of it -, or a participant of a channel into a further thread of it (issue 602),
//! added or joining, letting them go, removed or leaving, from the session with every channel of
//! it, from one channel or from one thread, and following an agent's deletion and restoring in the
//! channels that list it.
//!
//! Each locks the participant exclusively (`Key::Participant`) and the channels it changes shared.
//! Taking in and letting go lock the session as opening it does, shared where it runs, exclusively
//! where it starts it; following, which opens none, locks the running sessions shared with their
//! participants. Each asks what the change asks - whoever acts across the session and its channels
//! enforces it, as `Participation` and the access policy decide (SECURITY.md) -, stores it by the
//! controllers of the session and the channels, which announce it, and has the loops start and end
//! the actors (`ChannelLoop::add_actor`, `remove_actor`); it holds what it locked until `run` has
//! carried the change out - an actor started and its join announced, an actor ended and its leave
//! announced -, so that nobody takes the same participant in or lets them go meanwhile.

use super::session::{self, lock_to_open};
use super::{Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::{ChannelError, ChannelLoop, ResolvedParticipant};
use crate::kernel::Kernel;
use crate::session::{ChannelRun, SessionError, SessionLoop};
use anyhow::{Error, Result};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, Participant, ThreadId};
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreError, Stored};
use caelumora_security::{ActionType, Enforcement, SecurityContext, JOIN};
use futures::future;
use std::collections::{BTreeSet, HashSet};
use std::slice;
use std::sync::Arc;
use tracing::{info, warn};

/// Takes a participant into a session, and into a channel of it at once if one is named - added
/// by whoever may change them, or joining by their own act -, starting the session where it does
/// not run: as opening it does for who adds (`session::open`), in the kernel's name
/// for who may join or read it for who joins (`session::open_to_take_in`).
pub struct TakeIn {
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
    /// channel `into`, seeing as much of its history as granted (`take_in`).
    pub fn added(
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        into: Option<(ChannelId, HistoryAccess)>,
    ) -> Self {
        Self { context: context.clone(), session, participant, coming_in: ComingIn::Added(into) }
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - into `session` by their
    /// own act, and into the channel `into` (`take_in`).
    pub fn joining(
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        into: Option<ChannelId>,
    ) -> Self {
        Self { context: context.clone(), session, participant, coming_in: ComingIn::Joining(into) }
    }
}

#[async_trait]
impl Procedure for TakeIn {
    type Plan = ();
    type Output = Arc<SessionLoop>;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
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

    /// Who adds is asked first whether they may change the session (`UpdateSession`) - unless the
    /// one taken in is in it already and a channel is named - and then the channel
    /// (`UpdateChannel`, issues 46, 198, 221), before anything of the one taken in is looked at;
    /// who joins is asked by `take_in`, as their own act, the channel reached as joining reaches
    /// it: running, not asked whether they may read it - joining it decides.
    ///
    /// # Errors
    /// As `session::open` or `open_to_take_in`; `StoreError::PermissionDenied` if
    /// whoever adds may not change the session or the channel; as `SessionLoop::channel_for`;
    /// `SessionError::ChannelNotFound` or `ChannelUnavailable` for a channel to join that the
    /// session does not list or that does not run (issue 463); as `take_in`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { context, session, participant, coming_in } = self;
        let access = kernel.access_policy();
        let (session, entry, into) = match coming_in {
            ComingIn::Added(into) => {
                let session = session::open(kernel, &context, session).await?;
                let document = session.session();
                if into.is_none() || !document.takes_part(participant) {
                    let changing = ActionType::update(Session::KIND, &document.security_attributes);
                    access.require(&context, changing, StoreError::PermissionDenied).await?;
                }
                let channel = match into {
                    Some((id, history)) => {
                        let channel = session.channel_for(&context, id).await?;
                        let attributes = channel.security_attributes();
                        let changing = ActionType::update(Channel::KIND, &attributes);
                        access.require(&context, changing, StoreError::PermissionDenied).await?;
                        Some((channel, history))
                    }
                    None => None,
                };
                (session, Entry::Added, channel)
            }
            ComingIn::Joining(into) => {
                let session = session::open_to_take_in(kernel, &context, session).await?;
                let channel = match into {
                    Some(id) => Some((channel_to_join(&session, id)?, HistoryAccess::All)),
                    None => None,
                };
                (session, Entry::Joining, channel)
            }
        };
        let line = into.as_ref().map(|(c, history)| (c, *history, ThreadId::line(c.id())));
        take_in(log, kernel, &session, &context, participant, entry, line).await?;
        Ok(session)
    }
}

/// The channel `id` of `session` if it runs, for whoever joins it: not asked whether they may read
/// it - joining it decides (`require_entry`).
///
/// # Errors
/// `SessionError::ChannelNotFound` if the session lists no such channel,
/// `SessionError::ChannelUnavailable` if it does not run (issue 463).
fn channel_to_join(session: &SessionLoop, id: ChannelId) -> Result<Arc<ChannelLoop>, SessionError> {
    let (_, run) = session
        .listed_channel(id)
        .ok_or(SessionError::ChannelNotFound { session: session.id(), channel: id })?;
    Ok(session.running(id, run)?)
}

/// Lets a participant go from a session with every channel of it, or from one channel - removed by
/// whoever may change them, or leaving by their own act -, starting the session where it does not
/// run, as opening it does.
pub struct LetGo {
    context: SecurityContext,
    session: SessionId,
    participant: ActorId,
    from: Option<ChannelId>,
    exit: Exit,
}

/// How someone goes out of a session or a channel: the one way out, asked for in two ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Removed by whoever may change the session, or the channel (`UpdateSession`,
    /// `UpdateChannel`).
    Removed,
    /// By their own act, themselves or their own agent (`LeaveSession`, `LeaveChannel`, issues
    /// 562, 590).
    Leaving,
}

impl Exit {
    /// In whose name the one who goes out is stored, on behalf of `context`: whoever removes them;
    /// the kernel's for one who leaves, once that was permitted.
    fn storing(self, context: &SecurityContext) -> SecurityContext {
        match self {
            Exit::Removed => context.clone(),
            Exit::Leaving => SecurityContext::system(),
        }
    }
}

impl LetGo {
    /// Lets `participant` go on behalf of `context`, who may change the session - from it and every
    /// channel of it (`let_go_from_session`) -, or with `from` the channel - from it alone
    /// (`let_go_from_channel`).
    pub fn removed(
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self { context: context.clone(), session, participant, from, exit: Exit::Removed }
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - out by their own act:
    /// out of the session and every channel of it (`let_go_from_session`), or with `from` out of
    /// that channel alone (`let_go_from_channel`).
    pub fn leaving(
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self { context: context.clone(), session, participant, from, exit: Exit::Leaving }
    }
}

#[async_trait]
impl Procedure for LetGo {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    /// A session it starts is locked exclusively, its channels with it; one that runs as letting
    /// go needs it (`lock_letting_go`).
    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        let running = kernel.session_manager().running(self.session);
        lock_letting_go(locks, running.as_deref(), self.session, self.participant, self.from).await
    }

    /// # Errors
    /// As `session::open`, `SessionLoop::channel_for` for the channel named, and as
    /// `let_go_from_channel` or `let_go_from_session`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { context, session, participant, from, exit } = self;
        let session = session::open(kernel, &context, session).await?;
        match from {
            Some(channel) => {
                let channel = session.channel_for(&context, channel).await?;
                let_go_from_channel(kernel, &channel, &context, participant, exit).await
            }
            None => let_go_from_session(kernel, &session, &context, participant, exit).await,
        }
    }
}

/// Takes a participant of a channel into a further thread of it (issue 602) - added by whoever may
/// change the channel, or joining by their own act -, starting the session where it does not run,
/// as opening it does.
pub struct TakeIntoThread {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
    participant: ActorId,
    entry: ThreadEntry,
}

impl TakeIntoThread {
    /// Takes `participant` into `thread` of `channel` of `session` on behalf of `context`, who may
    /// change the channel (`take_into_thread`).
    pub fn added(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Self {
        let context = context.clone();
        Self { context, session, channel, thread, participant, entry: ThreadEntry::Added }
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - into `thread` of
    /// `channel` of `session` by their own act (`take_into_thread`).
    pub fn joining(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Self {
        let context = context.clone();
        Self { context, session, channel, thread, participant, entry: ThreadEntry::Joining }
    }
}

#[async_trait]
impl Procedure for TakeIntoThread {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        lock_thread_change(locks, self.session, self.channel, self.thread, self.participant)
    }

    /// # Errors
    /// As `session::open`, `SessionLoop::channel_for` and `take_into_thread`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { context, session, channel, thread, participant, entry } = self;
        let session = session::open(kernel, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        require_thread_entry(kernel, &channel, &context, participant, entry).await?;
        take_into_thread(log, &channel, &context, participant, thread).await
    }
}

/// Lets a participant of a channel go from one thread of it alone (issue 602) - removed by whoever
/// may change the channel, or leaving by their own act -, staying in its other threads; starting
/// the session where it does not run, as opening it does.
pub struct LetGoFromThread {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
    participant: ActorId,
    exit: Exit,
}

impl LetGoFromThread {
    /// Lets `participant` go from `thread` of `channel` of `session` on behalf of `context`, who
    /// may change the channel (`let_go_from_thread`).
    pub fn removed(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Self {
        let context = context.clone();
        Self { context, session, channel, thread, participant, exit: Exit::Removed }
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - out of `thread` of
    /// `channel` of `session` by their own act (`let_go_from_thread`).
    pub fn leaving(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        thread: ThreadId,
        participant: ActorId,
    ) -> Self {
        let context = context.clone();
        Self { context, session, channel, thread, participant, exit: Exit::Leaving }
    }
}

#[async_trait]
impl Procedure for LetGoFromThread {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        lock_thread_change(locks, self.session, self.channel, self.thread, self.participant)
    }

    /// # Errors
    /// As `session::open`, `SessionLoop::channel_for` and `let_go_from_thread`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { context, session, channel, thread, participant, exit } = self;
        let session = session::open(kernel, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        let_go_from_thread(kernel, &channel, &context, participant, thread, exit).await
    }
}

/// Locks what taking `participant` into `thread` of `channel` of `session`, or letting them go from
/// it, needs (issue 602): the participant exclusively, so that nobody takes them in or lets them go
/// meanwhile; the channel and the thread shared, so that nobody closes the thread before they have
/// joined or left it.
///
/// # Errors
/// `Deferred` if anything of it is held otherwise.
fn lock_thread_change(
    locks: &mut Locks<'_>,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
    participant: ActorId,
) -> Result<(), Deferred> {
    locks.exclusive(Key::Participant(session, participant))?;
    locks.shared(Key::Channel(session, channel))?;
    locks.shared(Key::Thread(session, channel, thread))
}

/// How someone comes into a session and its channels: the one way in, asked for in four ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Added by whoever may change the session, or the channel (`UpdateSession`, `UpdateChannel`).
    Added,
    /// By their own act, where the session or the channel lets them join (`JOIN`).
    Joining,
    /// As a founder of a channel just created (issue 420), named by whoever created it: creating
    /// it (`CreateChannel`) was what was asked.
    Founding,
    /// As a founder of a thread just opened (issue 557), named by whoever opened it: opening it
    /// (`OpenThread`) was what was asked, and the founder is whom they act for, or an agent of
    /// their own.
    FoundingThread,
}

impl Entry {
    /// Whether it founds a channel or a thread: given what is said from its join on, and started
    /// with what it founds rather than waited for.
    fn founds(self) -> bool {
        matches!(self, Entry::Founding | Entry::FoundingThread)
    }

    /// In whose name the one who comes in is stored, on behalf of `context`: whoever adds them;
    /// the kernel's for one who joins or founds, once that was permitted.
    fn storing(self, context: &SecurityContext) -> SecurityContext {
        match self {
            Entry::Added => context.clone(),
            Entry::Joining | Entry::Founding | Entry::FoundingThread => SecurityContext::system(),
        }
    }
}

/// Takes `participant` into `session` on behalf of `context` - unless in it already - and, with
/// `into`, into that channel, seeing as much of its history as granted, taking part in that thread,
/// by the way `entry` says; what that way asks of whoever takes them in is asked before (issue
/// 593): first into the session (`take_into_session`), then into the channel
/// (`take_into_channel`).
///
/// # Errors
/// As `take_into_session` and `take_into_channel`.
pub(crate) async fn take_in(
    log: &mut Compensation,
    kernel: &Kernel,
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    participant: ActorId,
    entry: Entry,
    into: Option<(&Arc<ChannelLoop>, HistoryAccess, ThreadId)>,
) -> Result<(), SessionError> {
    let channel = into.map(|(channel, ..)| channel);
    let resolved =
        take_into_session(log, kernel, session, context, participant, entry, channel).await?;
    let Some(line) = into else { return Ok(()) };
    take_into_channel(log, session, context, resolved, entry, line).await
}

/// The first part of taking `participant` in (`take_in`): everything is asked of them once, in
/// this order - whose participant they are, the session first (`newcomer`), then `channel`
/// (`require_entry`), then anything in their name: whether they can run and may take part in
/// `channel` (`Participation::require_admitted`, issue 562) -, and they are listed in `session`
/// unless in it already, stored in the name the way `entry` says and announced by the session's
/// controller, the listing taken back by `log` if a later step fails - the session as it was,
/// their invitation given back (issue 590). Answers them resolved, to be taken into `channel`
/// with (`take_into_channel`). For `thread::open` on its own as well, which asks every founder
/// before the thread is there.
///
/// # Errors
/// As `newcomer` and `require_entry`; `ChannelError::OutOfScope` or `NotCleared`,
/// `StoreError::PermissionDenied` for an agent that cannot run; `SessionError::CannotTakeIn` for
/// an internal actor; what storing the session reports.
pub(crate) async fn take_into_session(
    log: &mut Compensation,
    kernel: &Kernel,
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    participant: ActorId,
    entry: Entry,
    channel: Option<&Arc<ChannelLoop>>,
) -> Result<ResolvedParticipant, SessionError> {
    let resolved = newcomer(kernel, session, context, participant, channel, entry).await?;
    match channel {
        Some(channel) => require_entry(kernel, channel, context, &resolved, entry).await?,
        // Without a channel there is no thread, and nobody is taken in as its founder.
        None if entry == Entry::FoundingThread => {
            return Err(anyhow::anyhow!("{participant} founds a thread of no channel").into());
        }
        None => {}
    }
    let classified = channel.map(|channel| channel.channel());
    let admitting = slice::from_ref(&resolved);
    kernel.participation().require_admitted(context, classified.as_deref(), admitting).await?;
    if !session.session().takes_part(participant) {
        let listed = Participant::of(participant).ok_or(SessionError::CannotTakeIn(participant))?;
        let before = session.controller().add_participant(&entry.storing(context), listed).await?;
        info!(
            session = %session.id(),
            participant = %participant,
            by = %context.caller_id,
            "Taken into the session"
        );
        let session = Arc::clone(session);
        log.record("taking someone into a session", async move {
            Ok(session.controller().roll_back_participant_addition(participant, &before).await?)
        });
    }
    Ok(resolved)
}

/// `actor`, to be taken into `session` - and into `channel` if one is named - on behalf of
/// `context` by the way `entry` says, read and resolved, once what the session asks of them is
/// asked: that they are not in it already, unless a channel is named; by their own act, where
/// joining, whether its access list lets them join (`JOIN`), unless they are in it. Asked before
/// the channel (`require_entry`) and anything in their name: whose participant it is first (issue
/// 562). Whoever takes them in reads them first, a user to find out that they exist (issue 120) -
/// but a founder of a channel just created, whom the session lists -, and the session follows one
/// not in it from then on (`SessionLoop::follow`). Stores nothing.
///
/// # Errors
/// `SessionError::AlreadyParticipant`, `CannotTakeIn` - also for a user or agent that no longer
/// exists; `ChannelError::NotInSession` for a founder of a channel the session does not list;
/// `StoreError::PermissionDenied` if they may not join; whatever reading them reports.
async fn newcomer(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    actor: ActorId,
    channel: Option<&Arc<ChannelLoop>>,
    entry: Entry,
) -> Result<ResolvedParticipant> {
    let document = session.session();
    let listed = document.participants.iter().find(|p| p.actor_id() == actor).cloned();
    if listed.is_some() && channel.is_none() {
        let id = session.id();
        return Err(SessionError::AlreadyParticipant { session: id, participant: actor }.into());
    }
    let in_session = listed.is_some();
    let participant = match (entry, listed) {
        // A founder of a channel just created is one of the session's, named by its creator:
        // creating the channel with them was what was asked (issue 420).
        (Entry::Founding, Some(listed)) => listed,
        (Entry::Founding, None) => {
            let channel = channel.ok_or_else(|| anyhow::anyhow!("{actor} founds no channel"))?;
            let outside = ChannelError::NotInSession { participant: actor, channel: channel.id() };
            return Err(outside.into());
        }
        _ => {
            let participant = Participant::of(actor).ok_or(SessionError::CannotTakeIn(actor))?;
            let stores = kernel.entity_stores();
            match participant {
                Participant::User(id) => drop(stores.users.get(context, id).await?),
                Participant::Agent(id) => drop(stores.agents.get(context, id).await?),
            }
            participant
        }
    };
    if !in_session {
        // Followed before it is resolved, so that nothing of it is missed until it is listed - its
        // deletion above all, which its actor follows once it is taken in (`Follow`). One refused,
        // or taken out of the session again by the compensation of taking it in, is followed for
        // nothing, which is harmless: its channels look only at whom they list.
        session.follow(&participant);
    }
    let channel_id = channel.map(|channel| channel.id());
    let controllers = kernel.controller_manager();
    let resolving =
        ResolvedParticipant::resolve(controllers, &participant, session.id(), channel_id);
    let resolved = resolving.await?.ok_or(SessionError::CannotTakeIn(actor))?;
    if !in_session && entry == Entry::Joining {
        let joining = ActionType::custom(Session::KIND, &JOIN, &document.security_attributes);
        let refusal = StoreError::PermissionDenied;
        kernel.participation().require_own_act(&resolved, context, joining, refusal).await?;
    }
    Ok(resolved)
}

/// Nothing if the one to take in, `resolved`, comes into `channel` by the way `entry` says as the
/// channel lets them, asked in `context`, whoever takes them in: by their own act, where joining,
/// whether the channel's access list lets them join (`JOIN`); where founding a thread of it,
/// whether they are whom `context` acts for or an agent of their own (issue 557), opening the
/// thread being permitted. Nothing for one added or founding the channel: changing the channel
/// was asked before (`TakeIn`), or creating it. Whose participant it is, asked after the session
/// asked its own (`newcomer`) and before anything in their name (issue 562) - refused as the
/// session refuses joining it, since taking in goes through the session.
///
/// # Errors
/// `StoreError::PermissionDenied` if they may not.
async fn require_entry(
    kernel: &Kernel,
    channel: &ChannelLoop,
    context: &SecurityContext,
    resolved: &ResolvedParticipant,
    entry: Entry,
) -> Result<()> {
    let attributes = channel.security_attributes();
    let participation = kernel.participation();
    let refusal = StoreError::PermissionDenied;
    match entry {
        Entry::Joining => {
            let joining = ActionType::custom(Channel::KIND, &JOIN, &attributes);
            participation.require_own_act(resolved, context, joining, refusal).await?;
        }
        Entry::FoundingThread => {
            let founding = ActionType::OpenThread(&attributes);
            participation.require_own(resolved, context, founding, refusal)?;
        }
        Entry::Added | Entry::Founding => {}
    }
    Ok(())
}

/// The second part of taking someone in (`take_in`), once they were asked and are in `session`,
/// as `resolved`: listed in the channel of `line`, seeing as much of its history as granted -
/// refused for one whose actor runs in it, or one the session does not list; one listed whose
/// actor does not run, left out as the channel started, is taken in again (issue 568) -, stored
/// in the name the way `entry` says and announced by the channel's controller; then their actor is
/// started to take part in its thread (`ChannelLoop::add_actor`), and waited for - but a
/// founder's, which starts with the channel or the thread it founds: what `run` is handed after it
/// comes after its start. Taken back by `log` if a later step fails: the channel is again as it
/// was, their invitation given back, and then the actor leaves, waited for - stored first, as
/// letting someone go stores first (`let_go_out_of`), so that whoever follows the participants by
/// the leave the channel tells, the approver among them, reads them gone (issue 603). An actor
/// that cannot be started takes the channel back at once.
///
/// # Errors
/// As `ChannelLoop::history_start` and `add_actor`; `ChannelError::AlreadyParticipant` or
/// `NotInSession`; what storing the channel reports - a `StoreError` if whoever stores it may not
/// change it; `ChannelError::Stopped` if the channel stopped before it started the actor.
pub(crate) async fn take_into_channel(
    log: &mut Compensation,
    session: &SessionLoop,
    context: &SecurityContext,
    resolved: ResolvedParticipant,
    entry: Entry,
    (channel, history, thread): (&Arc<ChannelLoop>, HistoryAccess, ThreadId),
) -> Result<(), SessionError> {
    let participant = resolved.actor_id();
    let from = if entry.founds() { None } else { channel.history_start(history).await? };
    let refusal = |error: ChannelError| SessionError::from(Error::from(error));
    if channel.runs(participant) {
        return Err(refusal(ChannelError::AlreadyParticipant {
            participant,
            channel: channel.id(),
        }));
    }
    let listed =
        session.session().participants.iter().find(|p| p.actor_id() == participant).cloned();
    let Some(listed) = listed else {
        return Err(refusal(ChannelError::NotInSession { participant, channel: channel.id() }));
    };
    let before = channel.controller().add_participant(&entry.storing(context), listed).await?;
    let taken_back = {
        let channel = Arc::clone(channel);
        async move { channel.controller().roll_back_participant_addition(participant, &before).await }
    };
    let threads = BTreeSet::from([thread]);
    let started = match channel.add_actor(resolved, threads, from, Some(context)).await {
        Ok(started) => started,
        Err(e) => {
            if let Err(undone) = taken_back.await {
                warn!(
                    "Taking {} into channel {} cannot be undone: {}",
                    participant,
                    channel.id(),
                    undone
                );
            }
            return Err(e.into());
        }
    };
    {
        let channel = Arc::clone(channel);
        log.record("taking someone into a channel", async move {
            let undone = taken_back.await;
            channel.remove_actor(participant, &SecurityContext::system()).await;
            Ok(undone?)
        });
    }
    if !entry.founds() && !started.carried_out().await {
        return Err(refusal(ChannelError::Stopped { participant, channel: channel.id() }));
    }
    info!("{} joined channel {}", participant, channel.id());
    Ok(())
}

/// Lets `participant` go from `session` and every channel of it on behalf of `context`, by the way
/// `exit` says (issue 590), for a procedure that holds what letting them go needs
/// (`lock_letting_go`) - `LetGo`, or closing a thread with the agents it owns (`thread::Close`,
/// issue 598). Everything is asked first:
/// of whoever removes them whether they may change the session (`UpdateSession`), before anything
/// of the one let go; then that they take part, and whether they may leave by their own act
/// (`require_exit_from_session`). Out of the session first, so that nobody takes them into one of
/// its channels from then on - nothing for one out of it already, as a letting go that failed in a
/// channel left them; then out of every channel of it in the kernel's name, kept as removed by
/// `context`'s caller - the running ones all at once, each until its actor ended
/// (`let_go_out_of`), one that does not run (issue 463) in its stored document and the copy the
/// session holds, so that it is built without them (`let_go_where_not_running`). A deleted channel
/// lets them go as it is restored (`procedure::channel::Restore`). Nothing is rolled back - taking
/// them out of a channel ended their turn -: a channel that fails leaves the error to the caller,
/// and asking again, for one out of the session and not yet out of each of its channels, finishes
/// it.
///
/// # Errors
/// `StoreError::PermissionDenied` if whoever removes them may not change the session; as
/// `require_exit_from_session`; whatever storing the session or a channel reports.
pub(crate) async fn let_go_from_session(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<(), SessionError> {
    if exit == Exit::Removed {
        let attributes = &session.session().security_attributes;
        let changing = ActionType::update(Session::KIND, attributes);
        kernel.access_policy().require(context, changing, StoreError::PermissionDenied).await?;
    }
    require_exit_from_session(kernel, session, context, participant, exit).await?;
    match session.controller().remove_participant(&exit.storing(context), participant).await {
        Ok(_) | Err(StoreError::NotFound { .. }) => {}
        Err(error) => return Err(error.into()),
    }
    // Letting them go from the session was permitted; storing in its channels is the kernel's.
    let system = SecurityContext::system();
    let mut running = Vec::new();
    for channel in session.session().channels.clone() {
        match session.listed_channel(channel) {
            Some((_, ChannelRun::Running(channel_loop))) => running.push(channel_loop),
            // Not while it is built again: the procedure holds the channel, which building it
            // again holds too (`procedure::channel::Start`).
            Some((held, ChannelRun::Unavailable(cause))) if held.lists(participant) => {
                let held = Channel::clone(&held);
                let by = context.caller_id;
                let left = let_go_where_not_running(kernel, held, &[participant], by).await?;
                session.hold_unavailable(left, cause);
            }
            Some(_) | None => {}
        }
    }
    let letting_go = running.iter().map(|c| let_go_out_of(c, context, &system, participant));
    future::join_all(letting_go).await.into_iter().collect::<Result<Vec<()>>>()?;
    info!(
        session = %session.id(),
        participant = %participant,
        by = %context.caller_id,
        "Let go from the session"
    );
    Ok(())
}

/// Nothing if `participant` may go out of `session` on behalf of `context` by the way `exit` says
/// (`let_go_from_session`), which asked whoever removes them whether they may change the session
/// before: they take part in it, or in a channel of it - one let go from the session whom a channel
/// failed to let go -; by their own act, whether leaving (`LeaveSession`) is permitted in the
/// participant's own context, an agent's by its owner, as leaving a channel is
/// (`require_exit_from_channel`). One that cannot be resolved, a deleted agent, cannot leave:
/// whoever may change the session lets it go.
///
/// # Errors
/// `SessionError::NotAParticipant`; `StoreError::PermissionDenied` if they may not leave; what
/// resolving the participant reports (`ResolvedParticipant::resolve`).
async fn require_exit_from_session(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<()> {
    let listed = |participants: &[Participant]| {
        participants.iter().find(|p| p.actor_id() == participant).cloned()
    };
    let document = session.session();
    let found = listed(&document.participants).or_else(|| {
        session.listed_channels().iter().find_map(|(channel, _)| listed(&channel.participants))
    });
    let Some(leaver) = found else {
        return Err(SessionError::NotAParticipant { session: session.id(), participant }.into());
    };
    if exit == Exit::Leaving {
        let leaving = ActionType::LeaveSession(&document.security_attributes);
        let controllers = kernel.controller_manager();
        let resolving = ResolvedParticipant::resolve(controllers, &leaver, session.id(), None);
        let Some(resolved) = resolving.await? else {
            return Err(StoreError::PermissionDenied(leaving.refused()).into());
        };
        let refusal = StoreError::PermissionDenied;
        kernel.participation().require_own_leave(&resolved, context, leaving, refusal).await?;
    }
    Ok(())
}

/// Lets `participant` go from `channel` alone on behalf of `context`, by the way `exit` says
/// (issues 46, 562), for a procedure that holds what letting them go needs (`lock_letting_go`):
/// asked as the channel asks it (`require_exit_from_channel`), then taken out until its actor ended
/// (`let_go_out_of`), stored in the name the way says (`Exit::storing`) - whoever removes them
/// asked as `UpdateChannel` as it is stored.
///
/// # Errors
/// As `require_exit_from_channel`; whatever storing the channel reports.
pub async fn let_go_from_channel(
    kernel: &Kernel,
    channel: &ChannelLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<(), SessionError> {
    require_exit_from_channel(kernel, channel, context, participant, exit).await?;
    let_go_out_of(channel, context, &exit.storing(context), participant).await?;
    Ok(())
}

/// Nothing if `participant` may go out of `channel` on behalf of `context` by the way `exit` says
/// (`let_go_from_channel`): the channel lists them; by their own act, whether leaving
/// (`LeaveChannel`) is permitted in the participant's own context, an agent's by its owner, as
/// joining is (`require_entry`). Whether it could run is not asked: an agent whose personality was
/// withdrawn is taken out by its owner. One that cannot be resolved, a deleted agent resting in the
/// channel, cannot leave: whoever may change the channel removes it - asked as the channel is
/// stored (`let_go_out_of`).
///
/// # Errors
/// `ChannelError::NotAParticipant`; `ChannelError::Forbidden` if they may not leave; what
/// resolving the participant reports (`ResolvedParticipant::resolve`).
async fn require_exit_from_channel(
    kernel: &Kernel,
    channel: &ChannelLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<()> {
    let listed = listed_in(channel, participant)?;
    if exit == Exit::Leaving {
        let id = channel.id();
        let resolved = resolve_in(kernel, channel, &listed).await?;
        let attributes = channel.security_attributes();
        let leaving = ActionType::LeaveChannel(&attributes);
        let refusal = |refused| ChannelError::Forbidden { action: "leaving", channel: id, refused };
        let Some(resolved) = resolved else {
            return Err(refusal(leaving.refused()).into());
        };
        kernel.participation().require_own_leave(&resolved, context, leaving, refusal).await?;
    }
    Ok(())
}

/// Takes `participant` out of the running `channel` on behalf of `context`, stored in the name of
/// `storing`, once letting them go was asked; nothing for one it does not list (any more). Stored
/// first, kept as a former participant removed by `context`'s caller - asked as `UpdateChannel` of
/// `storing` - and announced by the channel's controller on the event bus (issue 73); then what
/// they take part in of what runs ended, until their actor ended (`ChannelLoop::remove_actor`). A
/// participant without an actor - a deleted agent resting in the channel, one that cannot run - is
/// removed from the document (issue 314), and restored it does not come back here.
///
/// # Errors
/// What storing the channel reports - a `StoreError` if `storing` may not change it.
async fn let_go_out_of(
    channel: &ChannelLoop,
    context: &SecurityContext,
    storing: &SecurityContext,
    participant: ActorId,
) -> Result<()> {
    if !channel.channel().lists(participant) {
        return Ok(());
    }
    channel.controller().remove_participant(storing, participant, context.caller_id).await?;
    channel.remove_actor(participant, context).await;
    Ok(())
}

/// How a participant of a channel comes into a further thread of it (issue 602): the one way in,
/// asked for in three ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadEntry {
    /// Added by whoever may change the channel (`UpdateChannel`).
    Added,
    /// By their own act, where they may read the channel (`GetChannel`).
    Joining,
    /// As a founder of a thread just opened, named by whoever opened it: opening it
    /// (`OpenThread`) was what was asked, and the founder is whom they act for, or an agent of
    /// their own.
    Founding,
}

/// Takes `participant`, who takes part in `channel`, into its thread `thread` as well on behalf of
/// `context` (issue 602), once taking them in was asked (`require_thread_entry`), for a procedure
/// that holds them and the thread (`lock_thread_change`) - `TakeIntoThread`, or `thread::open` for
/// a founder who takes part in the channel already: their actor taken into the thread
/// (`ChannelLoop::take_into_thread`) and waited for until its join is announced; taken back by
/// `log` if a later step fails - let go from the thread again, in the kernel's name. The channel's
/// document is not changed: it lists who takes part in the channel, not where.
///
/// # Errors
/// As `ChannelLoop::take_into_thread`; `ChannelError::Stopped` if the channel stopped, or their
/// actor ended, before it announced the join.
pub(crate) async fn take_into_thread(
    log: &mut Compensation,
    channel: &Arc<ChannelLoop>,
    context: &SecurityContext,
    participant: ActorId,
    thread: ThreadId,
) -> Result<(), SessionError> {
    let joined = channel.take_into_thread(participant, thread, context).await?;
    {
        let channel = Arc::clone(channel);
        log.record("taking someone into a thread", async move {
            let system = SecurityContext::system();
            channel.let_go_from_thread(participant, thread, &system)?.carried_out().await;
            Ok(())
        });
    }
    if !joined.carried_out().await {
        let stopped = ChannelError::Stopped { participant, channel: channel.id() };
        return Err(Error::from(stopped).into());
    }
    info!("{} joined thread {} of channel {}", participant, thread, channel.id());
    Ok(())
}

/// Nothing if `participant`, whom `channel` lists, may be taken into a thread of it on behalf of
/// `context` by the way `entry` says (`take_into_thread`), asked before anything is stored: added,
/// whether `context` may change the channel (`UpdateChannel`), before anything of the participant
/// is looked at; by their own act, whether they may
/// read the channel (`GetChannel`) in the participant's own context, an agent's by its owner, as
/// joining is asked (`require_entry`); founding a thread, whether they are whom `context` acts for
/// or an agent of their own, opening the thread being permitted (`OpenThread`).
///
/// # Errors
/// `ChannelError::NotAParticipant` for one the channel does not list, or that cannot be resolved -
/// a deleted agent resting in it; `StoreError::PermissionDenied` if they may not.
pub(crate) async fn require_thread_entry(
    kernel: &Kernel,
    channel: &ChannelLoop,
    context: &SecurityContext,
    participant: ActorId,
    entry: ThreadEntry,
) -> Result<()> {
    let attributes = channel.security_attributes();
    let refusal = StoreError::PermissionDenied;
    if entry == ThreadEntry::Added {
        let changing = ActionType::update(Channel::KIND, &attributes);
        kernel.access_policy().require(context, changing, refusal).await?;
    }
    let not_taking_part = || ChannelError::NotAParticipant { participant, channel: channel.id() };
    let resolved = resolve_in(kernel, channel, &listed_in(channel, participant)?).await?;
    let resolved = resolved.ok_or_else(not_taking_part)?;
    let participation = kernel.participation();
    match entry {
        ThreadEntry::Added => {}
        ThreadEntry::Joining => {
            let reading = ActionType::get(Channel::KIND, &attributes);
            participation.require_own_act(&resolved, context, reading, refusal).await?;
        }
        ThreadEntry::Founding => {
            let founding = ActionType::OpenThread(&attributes);
            participation.require_own(&resolved, context, founding, refusal)?;
        }
    }
    Ok(())
}

/// `participant` as `channel` lists them: who is asked whether they may come into a thread of it,
/// or go out of it or of the channel.
///
/// # Errors
/// `ChannelError::NotAParticipant` for one the channel does not list.
fn listed_in(channel: &ChannelLoop, participant: ActorId) -> Result<Participant> {
    let document = channel.channel();
    let listed = document.participants.iter().find(|p| p.actor_id() == participant).cloned();
    listed
        .ok_or_else(|| ChannelError::NotAParticipant { participant, channel: channel.id() }.into())
}

/// `listed`, a participant of `channel`, resolved (`ResolvedParticipant::resolve`); `None` for one
/// that cannot be resolved - a deleted agent resting in the channel.
///
/// # Errors
/// What resolving them reports.
async fn resolve_in(
    kernel: &Kernel,
    channel: &ChannelLoop,
    listed: &Participant,
) -> Result<Option<ResolvedParticipant>> {
    let controllers = kernel.controller_manager();
    ResolvedParticipant::resolve(controllers, listed, channel.session_id(), Some(channel.id()))
        .await
}

/// Lets `participant` go from `thread` of `channel` alone on behalf of `context`, by the way `exit`
/// says (issue 602), for a procedure that holds them and the thread (`lock_thread_change`): asked
/// as leaving the channel is asked (`require_exit_from_channel`) - whoever removes them, whether
/// they may change the channel (`UpdateChannel`) -, then their actor let go from the thread
/// (`ChannelLoop::let_go_from_thread`), its turn there ended, and waited for until its leave is
/// announced. Nothing is rolled back.
///
/// # Errors
/// As `require_exit_from_channel` and `ChannelLoop::let_go_from_thread`;
/// `StoreError::PermissionDenied` if whoever removes them may not change the channel;
/// `ChannelError::Stopped` if the channel stopped, or their actor ended, before it announced the
/// leave.
pub(crate) async fn let_go_from_thread(
    kernel: &Kernel,
    channel: &ChannelLoop,
    context: &SecurityContext,
    participant: ActorId,
    thread: ThreadId,
    exit: Exit,
) -> Result<(), SessionError> {
    if exit == Exit::Removed {
        let attributes = channel.security_attributes();
        let changing = ActionType::update(Channel::KIND, &attributes);
        kernel.access_policy().require(context, changing, StoreError::PermissionDenied).await?;
    }
    require_exit_from_channel(kernel, channel, context, participant, exit).await?;
    let left = channel.let_go_from_thread(participant, thread, context)?;
    if !left.carried_out().await {
        let stopped = ChannelError::Stopped { participant, channel: channel.id() };
        return Err(Error::from(stopped).into());
    }
    info!("{} left thread {} of channel {}", participant, thread, channel.id());
    Ok(())
}

/// `channel`, which does not run, with `participants` let go by `by` (issue 590): stored in the
/// kernel's name - letting them go from the session was permitted - by a controller of its own,
/// which keeps each as a former participant, lets the group of its participants follow and
/// announces it, as for a running channel (`ChannelController::remove_participant`): one left out
/// as the session opened (`let_go_from_session`), or one restored - before it is built, for the
/// document of a deleted channel cannot be changed (`procedure::channel::Restore`).
///
/// # Errors
/// Whatever storing the channel reports.
pub(crate) async fn let_go_where_not_running(
    kernel: &Kernel,
    channel: Channel,
    participants: &[ActorId],
    by: ActorId,
) -> Result<Channel> {
    if participants.is_empty() {
        return Ok(channel);
    }
    let controller = kernel.controller_manager().new_channel_controller(channel);
    for participant in participants {
        controller.remove_participant(&SecurityContext::system(), *participant, by).await?;
    }
    Ok(Channel::clone(&controller.load()))
}

/// Locks what letting `participant` go from `session` needs, `running` where it runs: the
/// participant exclusively; the channel `from`, or every channel of the session - also one that
/// does not run, whose document letting go changes, and one that lists them only once they were
/// founded into it meanwhile - shared; and in each the threads they take part in shared, until they
/// have left them, so that nobody closes one before (`thread::Close`). What `LetGo` locks, and
/// `thread::Close` for each agent the thread owns (issue 598).
/// Asks nothing; of a session that does not run, the participant and the channel named alone.
///
/// # Errors
/// `Deferred` if anything of it is held otherwise.
pub(crate) async fn lock_letting_go(
    locks: &mut Locks<'_>,
    running: Option<&SessionLoop>,
    session: SessionId,
    participant: ActorId,
    from: Option<ChannelId>,
) -> Result<(), Deferred> {
    locks.exclusive(Key::Participant(session, participant))?;
    let channels = match (from, running) {
        (Some(channel), _) => vec![channel],
        (None, Some(running)) => running.session().channels.clone(),
        (None, None) => Vec::new(),
    };
    for channel in channels {
        locks.shared(Key::Channel(session, channel))?;
        let Some(running) = running.and_then(|running| running.channel(channel)) else {
            continue;
        };
        for thread in running.threads_taken_part_in(participant).await {
            locks.shared(Key::Thread(session, channel, thread))?;
        }
    }
    Ok(())
}

/// Has the running channels follow whether an agent exists (issue 314): a deleted agent rests in
/// them, its actor stopped; a restored one runs again (`SessionLoop::follow_agents`). Holds
/// the agents it follows in each running session (`Key::Participant`) - an agent in every one, so
/// that whoever takes it in meanwhile is followed after - and the sessions' channels shared.
pub struct Follow {
    followed: Followed,
}

/// What a `Follow` follows.
enum Followed {
    /// One agent, in every running session.
    Agent(AgentId),
    /// Every agent of one running session.
    Session(SessionId),
}

impl Follow {
    /// Agent `agent` in every running session whose channels list it.
    pub fn agent(agent: AgentId) -> Self {
        Self { followed: Followed::Agent(agent) }
    }

    /// Every agent of running session `session`, after its follower lost events.
    pub fn session(session: SessionId) -> Self {
        Self { followed: Followed::Session(session) }
    }
}

#[async_trait]
impl Procedure for Follow {
    /// The sessions it follows agents in, each with the agents it holds there.
    type Plan = Vec<(SessionId, HashSet<AgentId>)>;
    type Output = ();
    type Error = SessionError;

    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        let sessions = kernel.session_manager();
        let ids = match self.followed {
            Followed::Session(session) => vec![session],
            Followed::Agent(_) => sessions.running_ids(),
        };
        let mut followed = Vec::new();
        for id in ids {
            let Some(running) = sessions.running(id) else { continue };
            let agents: HashSet<AgentId> = match self.followed {
                Followed::Agent(agent) => HashSet::from([agent]),
                Followed::Session(_) => running
                    .listed_channels()
                    .iter()
                    .flat_map(|(channel, _)| channel.participants.iter())
                    .filter_map(|participant| match participant {
                        Participant::Agent(agent) => Some(*agent),
                        Participant::User(_) => None,
                    })
                    .collect(),
            };
            for agent in &agents {
                locks.exclusive(Key::Participant(id, ActorId::Agent(*agent)))?;
            }
            for channel in &running.session().channels {
                locks.shared(Key::Channel(id, *channel))?;
                // The threads each takes part in: a deleted agent's actor leaves them (`LetGo`).
                let Some(channel_loop) = running.channel(*channel) else { continue };
                for agent in &agents {
                    let participant = ActorId::Agent(*agent);
                    for thread in channel_loop.threads_taken_part_in(participant).await {
                        locks.shared(Key::Thread(id, *channel, thread))?;
                    }
                }
            }
            followed.push((id, agents));
        }
        Ok(followed)
    }

    /// Nothing fails: what cannot be read or run is logged and left as it is.
    async fn carry_out(
        self,
        followed: Self::Plan,
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let sessions = kernel.session_manager();
        for (id, agents) in followed {
            if let Some(running) = sessions.running(id) {
                running.follow_agents(Some(&agents)).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Follow, LetGo, TakeIn};
    use crate::channel::ChannelLoop;
    use crate::component::BUNDLE_REF_KIND;
    use crate::event::SubscriptionTopic;
    use crate::kernel::Kernel;
    use crate::procedure::channel::Start;
    use crate::procedure::follower::SessionFollower;
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::session::testing::*;
    use crate::session::ChannelSpec;
    use crate::session::{SessionError, SessionLoop};
    use crate::testing::{attributes, Script, ScriptedAgents, World};
    use crate::view::Viewer;
    use anyhow::Error;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::testing::main_channel;
    use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, Participant};
    use caelumora_domain::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
    use caelumora_domain::event::{Announcing, GlobalEvent};
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::history::{HistoryEntry, Reader};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::{AclEntry, SecurityAttributes};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::{Session, SessionUpdateEvent};
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{User, UserId};
    use caelumora_domain::value::name::ScopedName;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

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
        let adding = TakeIn::added(&alice, id, bob, into);
        waits_for(held, &executor, adding).await.expect("taken in once it is let go");

        let held = Held::key(&executor, Key::Channel(id, channel), true).await;
        let removing = LetGo::removed(&alice, id, bob, Some(channel));
        waits_for(held, &executor, removing).await.expect("let go once it is let go");

        let held = Held::key(&executor, Key::Channel(id, channel), true).await;
        let adding = TakeIn::added(&alice, id, bob, into);
        waits_for(held, &executor, adding).await.expect("taken in again once it is let go");

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let removing = LetGo::removed(&alice, id, bob, None);
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
        let adding = TakeIn::added(&alice, id, ActorId::User(world.bob), None);
        executor.run(adding).await.expect("taken in beside it");
        held.release().await;

        world.manager.stop(&alice, id).await.unwrap();
        let held = Held::key(&executor, Key::Session(id), false).await;
        let adding = TakeIn::added(&alice, id, ActorId::User(world.carol), None);
        waits_for(held, &executor, adding).await.expect("started and taken in once it is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }

    /// Following an agent holds its part in every running session - also one that does not list
    /// it yet, so that whoever takes it in meanwhile is followed after -; following a session after
    /// lost events holds every agent of it.
    #[tokio::test]
    async fn following_holds_the_agent_in_every_running_session() {
        let world = World::with(&[], &[&ScriptedAgents(Script::Hangs)], |_| {}).await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let with = world.shared_session(&[helper], &[]).await.id();
        let without = world.shared_session(&[], &[]).await.id();
        let agent = ActorId::Agent(helper);

        let held = Held::key(&executor, Key::Participant(with, agent), true).await;
        waits_for(held, &executor, Follow::agent(helper)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(without, agent), true).await;
        waits_for(held, &executor, Follow::agent(helper)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(with, agent), true).await;
        waits_for(held, &executor, Follow::session(with)).await.unwrap();
        world.manager.stop(&alice, with).await.unwrap();
        world.manager.stop(&alice, without).await.unwrap();
    }

    fn as_user(user: &User) -> SecurityContext {
        SecurityContext::new(ActorId::User(user.id), Default::default(), Default::default())
    }

    fn refused_by_the_store(error: &Error) -> bool {
        matches!(error.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_)))
    }

    #[tokio::test]
    async fn whoever_may_change_the_session_takes_a_user_into_it_and_a_channel_at_once() {
        let kernel = owned_kernel(Default::default());
        let manager = kernel.session_manager();
        let stores = kernel.entity_stores();
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(stores, &[&alice, &carol]).await;
        record_users(stores, &[&bob]).await;
        let session = create(manager, &as_user(&alice), session).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        let bob_id = ActorId::User(bob.id);
        assert!(
            kernel.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_err(),
            "bob reads nothing of it before"
        );
        let mut events = kernel.event_bus().subscribe_all();

        let refused = session
            .add_participant(&as_user(&carol), bob_id, Some((main.id(), HistoryAccess::All)))
            .await
            .expect_err("a participant may not");
        assert!(refused_by_the_store(&refused), "{refused}");
        assert!(
            !session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "nothing was stored"
        );

        session
            .add_participant(&as_user(&alice), bob_id, Some((main.id(), HistoryAccess::All)))
            .await
            .expect("the owner may");
        assert!(
            session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "in the session"
        );
        assert!(main.participants().iter().any(|m| m.id == bob_id), "and in the channel");
        let members = stores
            .groups
            .get(&SecurityContext::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(members.contains(&bob_id), "the group of its participants follows");
        assert!(
            kernel.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_ok(),
            "so he reads it now"
        );
        assert!(events.drain().await.iter().any(|e| matches!(e.about::<Session>(), Some((id, change)) if id == session.id()
            && matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::ParticipantJoined(p)) if p.actor_id() == bob_id))), "announced");

        let again = session
            .add_participant(&as_user(&alice), bob_id, None)
            .await
            .expect_err("in it already");
        assert!(
            matches!(
                again.downcast_ref::<SessionError>(),
                Some(SessionError::AlreadyParticipant { .. })
            ),
            "{again}"
        );
        let approver = session
            .add_participant(&as_user(&alice), ActorId::System, None)
            .await
            .expect_err("only users and agents");
        assert!(
            matches!(
                approver.downcast_ref::<SessionError>(),
                Some(SessionError::CannotTakeIn(ActorId::System))
            ),
            "{approver}"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Alice's session with carol in its channel "main" and in two that do not run once it is
    /// opened again: "unavailable", whose policy bob shared with alice and withdrew, and "deleted";
    /// her joins recorded in both (`out_of_service`).
    struct OutOfService {
        session: Arc<SessionLoop>,
        unavailable: ChannelId,
        deleted: ChannelId,
        /// Bob's policy, as withdrawn.
        withdrawn: ComponentBundle,
    }

    /// Issue 593: building again the channels left out holds each of them exclusively, so that
    /// nobody lets someone go from one meanwhile - letting go holds them shared.
    #[tokio::test]
    async fn building_again_the_channels_left_out_holds_them() {
        let world = World::new().await;
        let OutOfService { session, unavailable, .. } = out_of_service(&world).await;
        let executor = Arc::clone(world.kernel.procedures());
        let held = Held::key(&executor, Key::Channel(session.id(), unavailable), false).await;
        let starting = Start::left_out(session.id());
        waits_for(held, &executor, starting).await.expect("built again once it is let go");
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    async fn out_of_service(world: &World) -> OutOfService {
        let bundles = &world.kernel.entity_stores().component_bundles;
        let bobs = World::as_user(world.bob);
        let policy = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("policy", "broadcast_policy")],
        };
        let name = ScopedName::new("bobs-policy", Scope::GlobalScope).unwrap();
        let mut shared =
            ComponentBundle::new(name, policy, attributes(Some(world.bob), &[world.alice]));
        bundles.create(&bobs, &mut shared).await.expect("bob shares a policy with alice");
        let session = world.session_of(&[world.bob, world.carol], &[], &[]).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        let with_carol = |name| ChannelSpec { participants: vec![carol], ..ChannelSpec::new(name) };
        let by_bundle = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params: json!({ "name": "bobs-policy" }),
                ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
            }],
        };
        let spec = ChannelSpec { components: by_bundle, ..with_carol("unavailable") };
        let unavailable = session.create_channel(&alice, spec).await.unwrap().id();
        let deleted = session.create_channel(&alice, with_carol("deleted")).await.unwrap().id();
        for channel in [unavailable, deleted] {
            assert!(carol_attending(world, channel, true).await, "her join is recorded");
        }
        assert!(session.delete_channel(&alice, deleted).await.unwrap(), "deleted");
        let unshared = attributes(Some(world.bob), &[]);
        let mut withdrawn = ComponentBundle { security_attributes: unshared, ..shared };
        bundles.update(&bobs, &mut withdrawn).await.expect("bob withdraws it");
        world.manager.stop(&alice, session.id()).await.unwrap();
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        assert!(session.channel(unavailable).is_none(), "does not run");
        OutOfService { session, unavailable, deleted, withdrawn }
    }

    /// Whether carol takes part in the history of `channel` (`HistoryStore::attending`), once she
    /// does as `open` says.
    async fn carol_attending(world: &World, channel: ChannelId, open: bool) -> bool {
        let carol = ActorId::User(world.carol);
        let history = world.kernel.history_store();
        time::timeout(Duration::from_secs(5), async {
            loop {
                if history.attending(channel).await.unwrap().contains_key(&carol) == open {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// Issue 590: one let go from the session is let go from every channel of it - a running one,
    /// one that does not run, in its document and in the copy the session holds, and a deleted one
    /// as it is restored -, the leave is announced, and they are not taken into a channel again but
    /// through the session.
    #[tokio::test]
    async fn one_let_go_from_the_session_is_let_go_from_every_channel_of_it() {
        let world = World::new().await;
        let OutOfService { session, unavailable, deleted, .. } = out_of_service(&world).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        let main = Arc::clone(&session.channels()[0]);
        let topics = vec![SubscriptionTopic::All];
        let mut hers = world.kernel.event_bus().subscribe(World::as_user(world.carol), topics);

        session.remove_participant(&alice, carol).await.expect("the owner lets her go");

        assert!(!session.session().takes_part(carol), "out of the session");
        assert!(!main.channel().lists(carol), "out of a running channel");
        let channels = world.entity_store::<Channel>();
        let stored = channels.get(&World::system(), unavailable).await.unwrap();
        assert!(!stored.lists(carol), "out of the document of one that does not run");
        let (held, _) = session.listed_channel(unavailable).expect("listed");
        assert!(!held.lists(carol), "and of the copy the session holds");
        let members = world
            .entity_store::<Group>()
            .get(&World::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(!members.contains(&carol), "the group of its participants follows");
        let left = |event: &GlobalEvent| {
            let update = event.about::<Session>().and_then(|(_, c)| c.update());
            matches!(update, Some(SessionUpdateEvent::ParticipantLeft(id)) if id == carol)
        };
        assert!(hers.drain().await.iter().any(left), "announced, to her as well");

        let restored = session.restore_channel(&alice, deleted).await.expect("restored").channel();
        assert!(!restored.lists(carol), "let go from a deleted one as it is restored");
        let former = restored.former.iter().find(|f| f.participant.actor_id() == carol);
        let former = former.expect("kept as a former participant there");
        assert_eq!(former.left.by, ActorId::System, "removed by the kernel, as it is restored");
        let again = session.remove_participant(&alice, carol).await.expect_err("let go already");
        assert!(
            matches!(again.downcast_ref(), Some(SessionError::NotAParticipant { .. })),
            "{again}"
        );
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: a channel that did not run as someone was let go from its session records their
    /// leave as it is built (reconciling), once and in the kernel's name - one that comes back as
    /// its components can be read again, and one restored -, so that their attendance ends there.
    #[tokio::test]
    async fn a_channel_that_did_not_run_records_the_leave_as_it_is_built() {
        let world = World::new().await;
        let OutOfService { session, unavailable, deleted, withdrawn } =
            out_of_service(&world).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        session.remove_participant(&alice, carol).await.expect("the owner lets her go");

        let reshared = attributes(Some(world.bob), &[world.alice]);
        let mut shared = ComponentBundle { security_attributes: reshared, ..withdrawn };
        let bundles = &world.kernel.entity_stores().component_bundles;
        bundles.update(&World::as_user(world.bob), &mut shared).await.expect("shared again");
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        assert!(session.channel(unavailable).is_some(), "runs again");
        session.restore_channel(&alice, deleted).await.expect("restored");

        let history = world.kernel.history_store();
        for channel in [unavailable, deleted] {
            assert!(carol_attending(&world, channel, false).await, "her attendance ends");
            let page = history.page(channel, &Reader::Channel, Position::START, None, 1000, None);
            let page = page.await.unwrap();
            let left: Vec<ActorId> = page
                .entries()
                .filter_map(|entry| match entry {
                    HistoryEntry::ParticipantLeft { participant, by, .. }
                        if *participant == carol =>
                    {
                        Some(*by)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(left, [ActorId::System], "once, in the kernel's name");
        }
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: one an owner takes into the session and a channel, whose actor cannot start -
    /// no agent runs in this world -, is rolled back out of the session as well, the group of its
    /// participants following, and the leave announced.
    #[tokio::test]
    async fn one_the_channel_cannot_take_in_is_rolled_back_out_of_the_session() {
        let world = World::new().await;
        let hers = world.personality("hers", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &hers).await);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let alice = World::as_user(world.alice);
        let mut events = world.kernel.event_bus().subscribe_all();

        let into = Some((main.id(), HistoryAccess::All));
        session.add_participant(&alice, helper, into).await.expect_err("it cannot start");

        assert!(!session.session().takes_part(helper), "rolled back out of the session");
        assert!(!main.channel().lists(helper), "and out of the channel");
        let members = world
            .entity_store::<Group>()
            .get(&World::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(!members.contains(&helper), "the group of its participants follows");
        let left = |event: &GlobalEvent| {
            let update = event.about::<Session>().and_then(|(_, c)| c.update());
            matches!(update, Some(SessionUpdateEvent::ParticipantLeft(id)) if id == helper)
        };
        assert!(events.drain().await.iter().any(left), "announced");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: a session hears none of its own announcements - its controller holds every
    /// change of it live already, and whom it takes in it follows where it takes them in -, so
    /// that a join announced after the leave that followed it does not list the one let go again.
    #[tokio::test]
    async fn a_join_heard_of_after_the_leave_does_not_list_the_one_let_go_again() {
        let world = World::new().await;
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let alice = World::as_user(world.alice);
        let bob = ActorId::User(world.bob);
        session.remove_participant(&alice, bob).await.expect("the owner lets him go");

        let joined = SessionUpdateEvent::ParticipantJoined(Participant::User(world.bob));
        let joined = GlobalEvent::updated::<Session>(&session.id(), &joined);
        world.kernel.event_bus().announce(joined, &*session.session());
        // Time for the session to hear of it.
        time::sleep(Duration::from_millis(100)).await;

        assert!(!session.session().takes_part(bob), "still let go");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: one let go from the session whom a channel failed to let go - nothing rolls them
    /// back into the session - is let go from it by asking again: by whoever may change it, since
    /// by their own act they no longer reach the session, as letting go opens it (issue 593).
    #[tokio::test]
    async fn asking_again_finishes_letting_go_where_a_channel_failed() {
        let world = World::new().await;
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let bob = ActorId::User(world.bob);
        // Out of the session, as letting go leaves one whose channel failed to store.
        session.controller().remove_participant(&World::system(), bob).await.unwrap();
        assert!(main.channel().lists(bob), "still in its channel");

        let unreached = session.leave(&World::as_user(world.bob), bob).await.expect_err("unread");
        assert!(matches!(unreached.downcast_ref(), Some(SessionError::NotFound(_))), "{unreached}");
        let alice = World::as_user(world.alice);
        session.remove_participant(&alice, bob).await.expect("she finishes it");
        assert!(!main.channel().lists(bob), "out of the channel too");
        let again = session.remove_participant(&alice, bob).await.expect_err("nowhere");
        assert!(
            matches!(again.downcast_ref(), Some(SessionError::NotAParticipant { .. })),
            "{again}"
        );
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn whom_the_access_list_lets_join_takes_themselves_in_and_uses_up_an_invitation() {
        let kernel = owned_kernel(Default::default());
        let manager = kernel.session_manager();
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let (bob_id, carol_id) = (ActorId::User(bob.id), ActorId::User(carol.id));
        let invited = |mut attributes: SecurityAttributes| {
            attributes.acl.push(AclEntry::invitation(bob_id));
            attributes
        };
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(kernel.entity_stores(), &[&alice]).await;
        record_users(kernel.entity_stores(), &[&bob, &carol]).await;
        session.security_attributes = invited(session.security_attributes);
        let channel = Channel {
            security_attributes: invited(main_channel(&session).security_attributes),
            ..main_channel(&session)
        };
        let session = manager.create(&as_user(&alice), session, vec![channel]).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);

        let uninvited = session
            .join(&as_user(&carol), carol_id, Some(main.id()))
            .await
            .expect_err("carol is not let in");
        // Neither invited nor reading it, she does not find it, as joining opens it (issue 593).
        assert!(matches!(uninvited.downcast_ref(), Some(SessionError::NotFound(_))), "{uninvited}");
        let for_another = session
            .join(&as_user(&bob), carol_id, None)
            .await
            .expect_err("bob joins as himself alone");
        assert!(refused_by_the_store(&for_another), "{for_another}");

        session.join(&as_user(&bob), bob_id, Some(main.id())).await.expect("bob is invited");
        assert!(
            session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "in the session"
        );
        assert!(main.participants().iter().any(|m| m.id == bob_id), "and in the channel");
        assert!(
            !session.session().security_attributes.acl.contains(&AclEntry::invitation(bob_id)),
            "his invitation is used up"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Deletes `agent` and waits until its actor stopped in `channel`: the
    /// session follower heard of the deletion.
    async fn deleted_stops_in(
        kernel: &Kernel,
        ctx: &SecurityContext,
        agent: AgentId,
        channel: &ChannelLoop,
    ) {
        assert!(kernel.agent_manager().delete(ctx, agent).await.expect("deleted"));
        let stopped = async {
            while channel.participant_ids().contains(&ActorId::Agent(agent)) {
                time::sleep(Duration::from_millis(10)).await
            }
        };
        time::timeout(Duration::from_secs(2), stopped)
            .await
            .expect("its actor stops: the follower heard of it");
    }

    #[tokio::test]
    async fn an_agent_taken_in_later_is_followed_as_one_taken_in_from_the_start() {
        let kernel = test_kernel();
        let manager = kernel.session_manager();
        // Followed as the programs follow them (issue 593).
        let _followed = SessionFollower::spawn(&kernel);
        let helper = stored_agent(
            kernel.entity_stores(),
            "helper",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let actor = ActorId::Agent(helper.id);
        let (session, ctx, _) = running_session_of_a_user(manager).await;
        let main = Arc::clone(&session.channels()[0]);
        let mut events = kernel.event_bus().subscribe_all();
        // Running, with the participants it had as it started.
        time::sleep(Duration::from_millis(50)).await;

        session
            .add_participant(&ctx, actor, Some((main.id(), HistoryAccess::All)))
            .await
            .expect("taken in");
        assert!(main.participant_ids().contains(&actor), "it runs in the channel");
        // Announced, and followed by the session follower as any agent.
        let announced = |events: &[GlobalEvent]| {
            events.iter().any(|e| matches!(e.about::<Session>().and_then(|(_, change)| change.update::<SessionUpdateEvent>()),
            Some(SessionUpdateEvent::ParticipantJoined(p)) if p.actor_id() == actor))
        };
        assert!(announced(&events.drain().await), "announced");
        time::sleep(Duration::from_millis(50)).await;

        deleted_stops_in(&kernel, &ctx, helper.id, &main).await;
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
