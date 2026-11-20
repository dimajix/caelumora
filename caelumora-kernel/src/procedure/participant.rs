//! The procedures of a participant's lifecycle (issue 593): taking someone into a session - and
//! into a channel of it -, added or joining, letting them go, removed or leaving, from the session
//! with every channel of it or from one channel, and following an agent's deletion and restoring
//! in the channels that list it.
//!
//! Each locks the participant exclusively (`Key::Participant`) and the channels it changes shared.
//! Taking in and letting go lock the session as opening it does, shared where it runs, exclusively
//! where it starts it; following, which opens none, locks the running sessions shared with their
//! participants. Each carries the change out by the building blocks of the `SessionLoop` and the
//! `ChannelLoop`, and holds what it locked until `run` has carried the change out - an actor
//! started and its join announced, an actor ended and its leave announced -, so that nobody takes
//! the same participant in or lets them go meanwhile.

use super::session::lock_to_open;
use super::{Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::{ChannelError, ChannelLoop, ResolvedParticipant};
use crate::session::{SessionError, SessionLoop, SessionManager};
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::{ChannelId, HistoryAccess, Participant, ThreadId};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use futures::future;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::info;

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
    /// channel `into`, seeing as much of its history as granted (`take_in`).
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
    /// own act, and into the channel `into` (`take_in`).
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

    /// Who adds is asked first whether they may change the session - unless the one taken in is
    /// in it already and a channel is named - and then the channel (issues 46, 221); who joins is
    /// asked by `take_in`, as their own act, the channel reached as joining reaches it
    /// (`SessionLoop::channel_to_join`).
    ///
    /// # Errors
    /// As `SessionManager::open_held` or `open_to_take_in`; `StoreError::PermissionDenied` if
    /// whoever adds may not change the session or the channel; as `SessionLoop::channel_for` or
    /// `channel_to_join`, and as `take_in`.
    async fn carry_out(
        self,
        (): (),
        log: &mut Compensation,
    ) -> Result<Arc<SessionLoop>, SessionError> {
        let Self { sessions, context, session, participant, coming_in } = self;
        let (session, entry, into) = match coming_in {
            ComingIn::Added(into) => {
                let session = sessions.open_held(&context, session).await?;
                if into.is_none() || !session.session().takes_part(participant) {
                    session.require_changing(&context).await?;
                }
                let channel = match into {
                    Some((id, history)) => {
                        let channel = session.channel_for(&context, id).await?;
                        channel.require_changing(&context).await?;
                        Some((channel, history))
                    }
                    None => None,
                };
                (session, Entry::Added, channel)
            }
            ComingIn::Joining(into) => {
                let session = sessions.open_to_take_in(&context, session).await?;
                let channel = match into {
                    Some(id) => Some((session.channel_to_join(id)?, HistoryAccess::All)),
                    None => None,
                };
                (session, Entry::Joining, channel)
            }
        };
        let line = into.as_ref().map(|(c, history)| (c, *history, ThreadId::line(c.id())));
        take_in(log, &session, &context, participant, entry, line).await?;
        Ok(session)
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
    pub(crate) fn storing(self, context: &SecurityContext) -> SecurityContext {
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
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self::new(sessions, context, session, participant, from, Exit::Removed)
    }

    /// Takes `participant` - whom `context` acts for, or their own agent - out by their own act:
    /// out of the session and every channel of it (`let_go_from_session`), or with `from` out of
    /// that channel alone (`let_go_from_channel`).
    pub fn leaving(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
    ) -> Self {
        Self::new(sessions, context, session, participant, from, Exit::Leaving)
    }

    fn new(
        sessions: &Arc<SessionManager>,
        context: &SecurityContext,
        session: SessionId,
        participant: ActorId,
        from: Option<ChannelId>,
        exit: Exit,
    ) -> Self {
        let sessions = Arc::clone(sessions);
        Self { sessions, context: context.clone(), session, participant, from, exit }
    }
}

#[async_trait]
impl Procedure for LetGo {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    /// A session it starts is locked exclusively, its channels with it; one that runs as letting
    /// go needs it (`lock_letting_go`).
    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(&self.sessions, locks, self.session)?;
        let running = self.sessions.running(self.session);
        lock_letting_go(locks, running.as_deref(), self.session, self.participant, self.from).await
    }

    /// # Errors
    /// As `SessionManager::open_held`, `SessionLoop::channel_for` for the channel named, and as
    /// `let_go_from_channel` or `let_go_from_session`.
    async fn carry_out(self, (): (), _: &mut Compensation) -> Result<(), SessionError> {
        let Self { sessions, context, session, participant, from, exit } = self;
        let session = sessions.open_held(&context, session).await?;
        match from {
            Some(channel) => {
                let channel = session.channel_for(&context, channel).await?;
                let_go_from_channel(&channel, &context, participant, exit).await
            }
            None => let_go_from_session(&session, &context, participant, exit).await,
        }
    }
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
    pub(crate) fn storing(self, context: &SecurityContext) -> SecurityContext {
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
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    participant: ActorId,
    entry: Entry,
    into: Option<(&Arc<ChannelLoop>, HistoryAccess, ThreadId)>,
) -> Result<(), SessionError> {
    let channel = into.map(|(channel, ..)| channel);
    let resolved = take_into_session(log, session, context, participant, entry, channel).await?;
    let Some(line) = into else { return Ok(()) };
    take_into_channel(log, context, resolved, entry, line).await
}

/// The first part of taking `participant` in (`take_in`): everything is asked of them once, each
/// object asking its own - whose participant they are, the session first
/// (`SessionLoop::newcomer`), then `channel` (`ChannelLoop::require_entry`), then anything in their
/// name: whether they can run and may take part in `channel` (`SessionLoop::require_to_take_in`,
/// issue 562) -, and they are listed in `session` unless in it already
/// (`SessionLoop::list_participant_held`), the listing taken back by `log` if a later step fails.
/// Answers them resolved, to be taken into `channel` with (`take_into_channel`). For
/// `thread::open` on its own as well, which asks every founder before the thread is there.
///
/// # Errors
/// As `SessionLoop::newcomer`, `ChannelLoop::require_entry`, `SessionLoop::require_to_take_in`
/// and `SessionLoop::list_participant_held`.
pub(crate) async fn take_into_session(
    log: &mut Compensation,
    session: &Arc<SessionLoop>,
    context: &SecurityContext,
    participant: ActorId,
    entry: Entry,
    channel: Option<&Arc<ChannelLoop>>,
) -> Result<ResolvedParticipant, SessionError> {
    let resolved = session.newcomer(context, participant, channel, entry).await?;
    match channel {
        Some(channel) => channel.require_entry(context, &resolved, entry).await?,
        // Without a channel there is no thread, and nobody is taken in as its founder.
        None if entry == Entry::FoundingThread => {
            return Err(anyhow::anyhow!("{participant} founds a thread of no channel").into());
        }
        None => {}
    }
    session.require_to_take_in(context, &resolved, channel).await?;
    let listed = session.list_participant_held(context, &resolved, entry).await?;
    if let Some(before) = listed {
        let session = Arc::clone(session);
        log.record("taking someone into a session", async move {
            session.undo_listing_held(participant, &before).await
        });
    }
    Ok(resolved)
}

/// The second part of taking someone in (`take_in`), once they were asked and are in the session,
/// as `resolved`: listed in the channel of `line`, seeing as much of its history as granted, the
/// listing taken back by `log` if a later step fails; then their actor is started to take part in
/// its thread, and waited for - but a founder's, which starts with the channel or the thread it
/// founds: what `run` is handed after it comes after its start.
///
/// # Errors
/// As `ChannelLoop::history_start`, `list_participant_held` and `start_participant_held`;
/// `ChannelError::Stopped` if the channel stopped before it started the actor.
pub(crate) async fn take_into_channel(
    log: &mut Compensation,
    context: &SecurityContext,
    resolved: ResolvedParticipant,
    entry: Entry,
    (channel, history, thread): (&Arc<ChannelLoop>, HistoryAccess, ThreadId),
) -> Result<(), SessionError> {
    let participant = resolved.actor_id();
    let from = if entry.founds() { None } else { channel.history_start(history).await? };
    let storing = entry.storing(context);
    let before = channel.list_participant_held(&storing, &resolved).await?;
    {
        let channel = Arc::clone(channel);
        log.record("taking someone into a channel", async move {
            channel.undo_listing_held(participant, &before).await
        });
    }
    let started = channel.start_participant_held(resolved, thread, from, context).await?;
    if !entry.founds() && !started.carried_out().await {
        let stopped = ChannelError::Stopped { participant, channel: channel.id() };
        return Err(anyhow::Error::from(stopped).into());
    }
    info!("{} joined channel {}", participant, channel.id());
    Ok(())
}

/// Lets `participant` go from `session` and every channel of it on behalf of `context`, by the way
/// `exit` says (issue 590), for a procedure that holds what letting them go needs
/// (`lock_letting_go`) - `LetGo`, or the sub-agents' end (issue 558). Everything is asked first,
/// each object asking its own: of whoever removes them whether they may change the session
/// (`SessionLoop::require_changing`), before anything of the one let go; then that they take part,
/// and whether they may leave by their own act (`SessionLoop::require_exit`). Out of the session
/// first, so that nobody takes them into one of its channels from then on
/// (`SessionLoop::unlist_participant_held`); then out of every channel of it in the kernel's name,
/// kept as removed by `context`'s caller - the running ones all at once, each until its actor ended
/// (`ChannelLoop::let_go_held`), one that does not run (issue 463) in its stored document
/// (`SessionLoop::let_go_where_unavailable_held`). A deleted channel lets them go as it is restored
/// (`procedure::channel::Restore`). Nothing is rolled back - taking them out of a channel ended
/// their turn -: a channel that fails leaves the error to the caller, and asking again, for one out
/// of the session and not yet out of each of its channels, finishes it.
///
/// # Errors
/// As `SessionLoop::require_changing` and `require_exit`; whatever storing the session or a
/// channel reports.
pub async fn let_go_from_session(
    session: &SessionLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<(), SessionError> {
    if exit == Exit::Removed {
        session.require_changing(context).await?;
    }
    session.require_exit(context, participant, exit).await?;
    session.unlist_participant_held(&exit.storing(context), participant).await?;
    // Letting them go from the session was permitted; storing in its channels is the kernel's.
    let system = SecurityContext::system();
    let mut running = Vec::new();
    for channel in session.session().channels.clone() {
        match session.channel(channel) {
            Some(channel_loop) => running.push(channel_loop),
            None => {
                let unavailable =
                    session.let_go_where_unavailable_held(context, channel, participant);
                unavailable.await?;
            }
        }
    }
    let letting_go = running.iter().map(|c| c.let_go_held(context, &system, participant));
    future::join_all(letting_go).await.into_iter().collect::<anyhow::Result<Vec<()>>>()?;
    info!(
        session = %session.id(),
        participant = %participant,
        by = %context.caller_id,
        "Let go from the session"
    );
    Ok(())
}

/// Lets `participant` go from `channel` alone on behalf of `context`, by the way `exit` says
/// (issues 46, 562), for a procedure that holds what letting them go needs (`lock_letting_go`):
/// asked as the channel asks it (`ChannelLoop::require_exit`), then taken out until its actor ended
/// (`ChannelLoop::let_go_held`), stored in the name the way says (`Exit::storing`) - whoever
/// removes them asked as `UpdateChannel` as it is stored.
///
/// # Errors
/// As `ChannelLoop::require_exit`; whatever storing the channel reports.
pub async fn let_go_from_channel(
    channel: &ChannelLoop,
    context: &SecurityContext,
    participant: ActorId,
    exit: Exit,
) -> Result<(), SessionError> {
    channel.require_exit(context, participant, exit).await?;
    channel.let_go_held(context, &exit.storing(context), participant).await?;
    Ok(())
}

/// Locks what letting `participant` go from `session` needs, `running` where it runs: the
/// participant exclusively; the channel `from`, or every channel of the session - also one that
/// does not run, whose document letting go changes, and one that lists them only once they were
/// founded into it meanwhile - shared; and in each the thread they take part in shared, until they
/// have left it, so that nobody closes it before (`thread::Close`). What `LetGo` locks, and a
/// plugin's procedure that lets someone go within a change of its own (issue 558's sub-agents).
/// Asks nothing; of a session that does not run, the participant and the channel named alone.
///
/// # Errors
/// `Deferred` if anything of it is held otherwise.
pub async fn lock_letting_go(
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
        if let Some(thread) = running.thread_taken_part_in(participant).await {
            locks.shared(Key::Thread(session, channel, thread))?;
        }
    }
    Ok(())
}

/// Has the running channels follow whether an agent exists (issue 314): a deleted agent rests in
/// them, its actor stopped; a restored one runs again (`SessionLoop::follow_agents_held`). Holds
/// the agents it follows in each running session (`Key::Participant`) - an agent in every one, so
/// that whoever takes it in meanwhile is followed after - and the sessions' channels shared.
pub struct Follow {
    sessions: Arc<SessionManager>,
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
    pub fn agent(sessions: &Arc<SessionManager>, agent: AgentId) -> Self {
        Self { sessions: Arc::clone(sessions), followed: Followed::Agent(agent) }
    }

    /// Every agent of running session `session`, after its follower lost events.
    pub fn session(sessions: &Arc<SessionManager>, session: SessionId) -> Self {
        Self { sessions: Arc::clone(sessions), followed: Followed::Session(session) }
    }
}

#[async_trait]
impl Procedure for Follow {
    /// The sessions it follows agents in, each with the agents it holds there.
    type Plan = Vec<(SessionId, HashSet<AgentId>)>;
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, locks: &mut Locks<'_>) -> Result<Self::Plan, Deferred> {
        let ids = match self.followed {
            Followed::Session(session) => vec![session],
            Followed::Agent(_) => self.sessions.running_ids(),
        };
        let mut followed = Vec::new();
        for id in ids {
            let Some(running) = self.sessions.running(id) else { continue };
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
                // The thread each takes part in: a deleted agent's actor leaves it (`LetGo`).
                let Some(channel_loop) = running.channel(*channel) else { continue };
                for agent in &agents {
                    let participant = ActorId::Agent(*agent);
                    if let Some(thread) = channel_loop.thread_taken_part_in(participant).await {
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
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        for (id, agents) in followed {
            if let Some(running) = self.sessions.running(id) {
                running.follow_agents_held(Some(&agents)).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Follow, LetGo, TakeIn};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::testing::{Script, ScriptedAgents, World};
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
        waits_for(held, &executor, Follow::agent(&world.manager, helper)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(without, agent), true).await;
        waits_for(held, &executor, Follow::agent(&world.manager, helper)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(with, agent), true).await;
        waits_for(held, &executor, Follow::session(&world.manager, with)).await.unwrap();
        world.manager.stop(&alice, with).await.unwrap();
        world.manager.stop(&alice, without).await.unwrap();
    }
}
