//! The follower of the kernel (issues 593, 598): an entry point that carries out, for the running
//! sessions, what follows from events outside any procedure - the running channels following
//! whether their agents exist (`participant::Follow`), and a thread whose delegation was let go
//! closed with the agent it owns (`thread::Close`) -, which every program that runs sessions starts
//! beside its `SessionManager`: without it, a deleted agent keeps running in the channels of a
//! running session until the session starts again, and a sub-agent nobody ends stays.

use super::participant::Follow;
use super::thread::Close;
use super::{Procedure, ProcedureExecutor};
use crate::session::{SessionError, SessionManager};
use caelumora_domain::agent::Agent;
use caelumora_domain::channel::{self, Channel, ChannelId, ThreadId};
use caelumora_domain::event::{EntityChange, GlobalEvent};
use caelumora_domain::session::SessionId;
use caelumora_security::SecurityContext;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::task::AbortOnDropHandle;
use tracing::warn;

/// Follows, for the whole kernel, what the running sessions are to follow: carries out a `Follow`
/// for every agent deleted or restored, and a `thread::Close` in the kernel's name for every
/// delegation let go in a running session (issue 598) - a consequence of what its giver did with
/// its own sub-agent; a session stopped meanwhile announces it again as it starts. Each in a task
/// of its own, so that it misses no event while one waits. After it lost events, it follows every
/// running session again: each its agents, and each thread of theirs whose delegation is let go.
/// Stops receiving when dropped; a procedure under way ends by itself.
pub struct SessionFollower {
    _following: AbortOnDropHandle<()>,
}

impl SessionFollower {
    /// Follows the sessions `sessions` runs, from now on.
    pub fn spawn(sessions: &Arc<SessionManager>) -> Self {
        let sessions = Arc::clone(sessions);
        let mut events = sessions.kernel().event_bus().subscribe_all().into_receiver();
        let following = tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => follow(&sessions, &event),
                    Err(RecvError::Lagged(lost)) => {
                        warn!("The follower lost {lost} events and follows every session again");
                        tokio::spawn(follow_again(Arc::clone(&sessions)));
                    }
                    // Not while the kernel holds the bus.
                    Err(RecvError::Closed) => return,
                }
            }
        });
        Self { _following: AbortOnDropHandle::new(following) }
    }
}

/// Carries out what `event` asks to be followed in `sessions`, if anything: an agent deleted or
/// restored, a delegation let go in a running session.
fn follow(sessions: &SessionManager, event: &GlobalEvent) {
    let procedures = sessions.kernel().procedures();
    if let Some((agent, EntityChange::Deleted | EntityChange::Restored)) = event.about::<Agent>() {
        carry_out(procedures, Follow::agent(agent), format!("following agent {agent}"));
    }
    if let Some(((session, channel), change)) = event.about::<Channel>() {
        let thread = change.data::<ThreadId>(channel::DELEGATION_LET_GO);
        if let Some(thread) = thread.filter(|_| sessions.running(session).is_some()) {
            close(procedures, session, channel, thread);
        }
    }
}

/// Follows every running session of `sessions` again, as after events were lost: its agents, and
/// each thread of its running channels whose delegation is let go.
async fn follow_again(sessions: Arc<SessionManager>) {
    let procedures = sessions.kernel().procedures();
    for session in sessions.running_ids() {
        carry_out(procedures, Follow::session(session), format!("following session {session}"));
        let Some(running) = sessions.running(session) else { continue };
        for channel in running.channels() {
            for thread in channel.delegations_let_go().await {
                close(procedures, session, channel.id(), thread);
            }
        }
    }
}

/// Closes `thread` of `channel` in `session` with the agent it owns, in the kernel's name.
fn close(
    procedures: &Arc<ProcedureExecutor>,
    session: SessionId,
    channel: ChannelId,
    thread: ThreadId,
) {
    let closing = Close::new(&SecurityContext::system(), session, channel, thread);
    carry_out(procedures, closing, format!("closing thread {thread} of channel {channel}"));
}

/// Carries `procedure` out in a task of its own, logging what it fails with as `what` it was.
fn carry_out<P>(procedures: &Arc<ProcedureExecutor>, procedure: P, what: String)
where
    P: Procedure<Error = SessionError> + 'static,
{
    let procedures = Arc::clone(procedures);
    tokio::spawn(async move {
        if let Err(error) = procedures.run(procedure).await {
            warn!("The follower failed {what}: {error:#}");
        }
    });
}
