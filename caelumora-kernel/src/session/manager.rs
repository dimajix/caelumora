//! `SessionManager`: the sessions of the process - the register of those that run here, which it
//! starts and stops, and the lifecycles of sessions and their channels, by which the procedures of
//! a session make, delete, restore and purge them (`procedure::session`, issue 593).

use super::{store_error, SessionError, SessionLoop};
use crate::channel::Ending;
use crate::event::EventBus;
use crate::group::GroupManager;
use crate::kernel::Kernel;
use crate::lifecycle::Lifecycle;
use crate::view::{Viewer, Views};
use anyhow::Result;
use caelumora_domain::channel::{Channel, ChannelId};
use caelumora_domain::event::{Announcing, EventPublisher, GlobalEvent};
use caelumora_domain::group::GroupId;
use caelumora_domain::security::SecurityAttributes;
use caelumora_domain::session;
use caelumora_domain::session::{Session, SessionId};
use caelumora_domain::store::{StoreResult, Stored};
use caelumora_security::{ActionType, Enforcement, Policies, SecurityContext};
use caelumora_store::sessions::{SessionHold, SessionLocks};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;

/// The sessions of the process: the register of those that run here, which it starts, holds and
/// stops, and the lifecycles of sessions and their channels.
pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// What holds a session for this process against the others that share the stores (issue
    /// 554), if any do; and the holds of the sessions that run here, let go as they stop.
    locks: Option<Arc<dyn SessionLocks>>,
    holds: DashMap<SessionId, SessionHold>,
    /// What builds the loops of the sessions it starts and the views of them, and what is made
    /// beside it runs in (`kernel`).
    kernel: Arc<Kernel>,
    /// The services it works with, taken from `kernel` once (issue
    /// 229).
    policies: Policies,
    events: Arc<EventBus>,
    /// What purges the group of the participants of a channel with it (issue 348).
    groups: Arc<GroupManager>,
    /// The lifecycles of what it is the manager of, by which its sessions and their channels are
    /// made, deleted, restored and purged - by it, or by the procedures through it -, each change
    /// but a purge announced as it is stored (issue 593).
    session_lifecycle: Lifecycle<Session>,
    channel_lifecycle: Lifecycle<Channel>,
}

/// A session as [`SessionManager::find`] found it for a context that may
/// read it: running or not, each asked about alike.
#[expect(clippy::large_enum_variant, reason = "a value returned once and taken apart at once")]
pub(crate) enum Found {
    /// It runs: its loop.
    Running(Arc<SessionLoop>),
    /// It does not run: its document, from the store.
    Stored(Session),
}

impl Found {
    /// The session as it is now.
    pub(crate) fn session(&self) -> Session {
        match self {
            Found::Running(session_loop) => (*session_loop.session()).clone(),
            Found::Stored(session) => session.clone(),
        }
    }

    /// The session's attributes as they are now, without the rest of it.
    pub(crate) fn attributes(&self) -> SecurityAttributes {
        match self {
            Found::Running(session_loop) => session_loop.session().security_attributes.clone(),
            Found::Stored(session) => session.security_attributes.clone(),
        }
    }
}

impl SessionManager {
    /// The session manager of `kernel`, which knows it from then on (`Kernel::sessions`).
    pub fn new(kernel: Arc<Kernel>) -> Arc<Self> {
        let stores = kernel.entity_stores();
        let events: Arc<dyn EventPublisher> = Arc::<EventBus>::clone(kernel.event_bus());
        let manager = Arc::new(Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            locks: kernel.session_locks().map(Arc::clone),
            holds: DashMap::new(),
            policies: kernel.policies().clone(),
            events: Arc::clone(kernel.event_bus()),
            groups: Arc::clone(kernel.group_manager()),
            session_lifecycle: Lifecycle::new(&stores.sessions, Arc::clone(&events)),
            channel_lifecycle: Lifecycle::new(&stores.channels, events),
            kernel,
        });
        manager.kernel.know_sessions(&manager);
        manager
    }

    /// Starts `session`, stored already, with `channels` here, for a procedure that holds it
    /// exclusively: its loop built, registered and run in a task of its own, and announced as
    /// `session::STARTED`; held by `hold` - the session held for this process where others share
    /// the stores (`hold`, issue 554) - as long as it runs.
    pub(crate) async fn start(
        &self,
        session: Session,
        channels: Vec<Channel>,
        hold: Option<SessionHold>,
    ) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let session_loop = SessionLoop::new(session, channels, Arc::clone(&self.kernel)).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));
        if let Some(hold) = hold {
            self.holds.insert(id, hold);
        }

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            let Err(e) = loop_for_task.run().await;
            tracing::error!("Session loop for {} failed: {}", id, e);
        });
        self.handles.insert(id, handle);

        self.events.announce(
            GlobalEvent::custom::<Session>(&id, session::STARTED, &()),
            &*session_loop.session(),
        );
        Ok(session_loop)
    }

    /// Session `id` held for this process against the others that share the stores (issue 554),
    /// for what a procedure that holds it does with it: `None` where it needs no hold of its own -
    /// it runs here, held as long as it runs, or no other process shares the stores.
    ///
    /// # Errors
    /// `SessionError::RunsElsewhere` if another process holds it; whatever asking reports.
    pub(crate) fn hold(&self, id: SessionId) -> Result<Option<SessionHold>> {
        let Some(locks) = &self.locks else { return Ok(None) };
        if self.holds.contains_key(&id) {
            return Ok(None);
        }
        match locks.try_hold(id)? {
            Some(hold) => Ok(Some(hold)),
            None => Err(SessionError::RunsElsewhere(id).into()),
        }
    }

    /// Session `id` as `context` may read it: its loop if it runs, its
    /// document from the store if not. A running session is asked about
    /// like a stored one - `GetSession` with its attributes - so that it
    /// being in memory is no way around the policy (issue 11).
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context`
    /// may not read it, alike whether it runs.
    pub(crate) async fn find(&self, context: &SecurityContext, id: SessionId) -> Result<Found> {
        let running = self.sessions.get(&id).map(|entry| entry.clone());
        match running {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                self.policies
                    .access_policy()
                    .require(context, ActionType::get(Session::KIND, &attributes), |_| {
                        SessionError::NotFound(id)
                    })
                    .await?;
                Ok(Found::Running(session_loop))
            }
            None => Ok(Found::Stored(
                self.session_lifecycle.store().get(context, id).await.map_err(store_error(id))?,
            )),
        }
    }

    /// The sessions that run.
    pub(crate) fn running_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|entry| *entry.key()).collect()
    }

    /// Stops session `id` if it runs, for a procedure that holds it exclusively. Its actors are
    /// ended first as `ending` says - told to stop, to be resumed when it runs again, or to leave
    /// for good (`SessionLoop::end_actors`) - and given `STOP_GRACE` to end by themselves, whoever
    /// did not aborted then, before the session's task is aborted (issue 377); once it has ended,
    /// and every channel's task is aborted with it, every channel it started is announced stopped,
    /// then the session (issue 593), and its hold for this process is let go. Nobody finds it
    /// running meanwhile. What runs in its channels besides - the commands - is the caller's to
    /// end.
    pub(crate) async fn stop_running(
        &self,
        id: SessionId,
        ending: Ending,
    ) -> Option<Arc<SessionLoop>> {
        let handle = self.handles.remove(&id).map(|(_, handle)| handle);
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            session_loop.end_actors(ending).await;
        }
        if let Some(handle) = handle {
            handle.abort();
            // Ended, so that no channel starts after it is announced stopped.
            let _ = handle.await;
        }
        self.holds.remove(&id);
        if let Some(session_loop) = &removed {
            session_loop.announce_channels_stopped();
            let session = session_loop.session();
            self.events
                .announce(GlobalEvent::custom::<Session>(&id, session::STOPPED, &()), &*session);
        }
        removed
    }

    /// Removes the document of channel `id` for good, with the group of its participants, in the
    /// kernel's name - marked deleted first if it is not: a channel of a session being purged,
    /// which is hidden with the session, or one whose creation failed -, since only what was
    /// deleted is purged (issue 164). Announces nothing. `false` if there was none.
    ///
    /// # Errors
    /// Whatever the store reports.
    pub(crate) async fn purge_channel(&self, id: ChannelId) -> StoreResult<bool> {
        self.groups.purge_participants(GroupId::of_channel(id)).await?;
        self.channel_lifecycle.purge_for_good(id).await
    }

    /// The root of the views (issue 163): every session `viewer` may
    /// read, and from there their channels and participants, running or not.
    pub fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::new(&self.kernel, Arc::<SessionManager>::clone(self), viewer)
    }

    /// The kernel it starts its sessions in - for an undo that repeats procedures, and so holds it
    /// (`procedure::session::record_creation`).
    pub(crate) fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }

    /// The lifecycle of its sessions: each made, deleted, restored and purged by the procedures of
    /// a session through it, and announced as it is (`procedure::session`).
    pub(crate) fn session_lifecycle(&self) -> &Lifecycle<Session> {
        &self.session_lifecycle
    }

    /// The lifecycle of the channels of its sessions: each made, deleted and restored by the
    /// procedures of a channel through it, and announced as it is (`procedure::channel`).
    pub(crate) fn channel_lifecycle(&self) -> &Lifecycle<Channel> {
        &self.channel_lifecycle
    }

    /// The loop of session `id`, if it runs.
    pub(crate) fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|entry| entry.clone())
    }
}
