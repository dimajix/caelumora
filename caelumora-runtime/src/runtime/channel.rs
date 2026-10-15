use crate::channel::{ChannelLoop, Participation};
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::ComponentRegistry;
use crate::controller::{ControllerManager, SessionController};
use crate::session::SessionLoop;
use crate::recorder::Recorder;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::ChannelId;
use caelumora_model::session::SessionId;
use caelumora_model::security::SecurityManager;
use caelumora_model::store::StateStore;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::{Arc, OnceLock, Weak};
use super::{ActorRuntime, RuntimeManager, Services};

/// What a channel runs with, made by its session (`SessionLoop::channel_runtime`) as the channel
/// is built: the process-wide services of the `RuntimeManager` - among them the controllers of its
/// users and agents (issue 121) - the user commands of its session (issue 28), the session's
/// controller (issue 13) and its loop (issue 361), and a handle to the channel itself. It is the
/// only source of `ActorRuntime`s, so every actor lives in exactly one channel and shares its
/// commands and handle.
#[derive(Debug, Clone)]
pub struct ChannelRuntime {
    pub(super) runtime_manager: Arc<RuntimeManager>,
    pub(super) command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
    /// The session the channel belongs to, live: what its agents' tool
    /// calls are guarded by (`ToolExecutor`). Held, not reached through
    /// `session_loop`: the guard must not lapse.
    session: Arc<SessionController>,
    /// The loop of that session, for the channel alone (`ChannelLoop::session_loop`) - never
    /// handed to the actors (issue 123). Weak, since the session owns its channels; dangling for a
    /// channel of no running session, in tests.
    session_loop: Weak<SessionLoop>,
    pub(super) channel: ChannelHandle,
}

/// The channel its actors run in, reachable from what they are built with
/// once it exists: set by `ChannelLoop::new` as soon as the channel does,
/// after its actors were built. Weak, because the channel owns its actors
/// and they hold this handle: a strong reference would keep a stopped
/// channel alive forever.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChannelHandle(Arc<OnceLock<Weak<ChannelLoop>>>);

impl ChannelHandle {
    /// The channel, or `None` while it is still being built or once it is
    /// gone.
    pub(crate) fn get(&self) -> Option<Arc<ChannelLoop>> {
        self.0.get().and_then(Weak::upgrade)
    }
}

impl ChannelRuntime {
    /// A runtime for a channel of the session `session` holds and `session_loop` runs, whose user
    /// commands are `command_executor`.
    pub fn new(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session: Arc<SessionController>, session_loop: Weak<SessionLoop>) -> Self {
        Self { runtime_manager, command_executor, session, session_loop, channel: ChannelHandle::default() }
    }

    /// The runtime of `actor` in the channel `channel_id` of session
    /// `session_id` - the channel this runtime is for, which is not attached
    /// yet while its actors are built.
    pub fn new_actor_runtime(&self, actor: ActorId, session_id: SessionId, channel_id: ChannelId) -> ActorRuntime {
        ActorRuntime { runtime: self.clone(), actor, session_id, channel_id }
    }

    /// Makes `channel` reachable from the runtimes of its actors. Called once
    /// by `ChannelLoop::new`, as soon as the channel exists.
    pub(crate) fn attach(&self, channel: &Arc<ChannelLoop>) {
        let _ = self.channel.0.set(Arc::downgrade(channel));
    }

    /// The controller of the channel's session.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> { &self.session }

    /// The loop of the channel's session, for the channel (`ChannelLoop::session_loop`); `None`
    /// once the session is gone, or for a channel of none, in tests.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> { self.session_loop.upgrade() }
}

/// What a channel is built with (`ChannelLoop::new`), each read once there into what the channel
/// holds or builds (issue 360): the runtime it is given offers the services by name rather than
/// handing out the `RuntimeManager` behind it (issue 191), and `ChannelLoop::new` takes no service
/// beside it.
impl ChannelRuntime {
    pub(crate) fn security_manager(&self) -> &Arc<dyn SecurityManager> { self.runtime_manager.security_manager() }

    pub(crate) fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { self.runtime_manager.confidentiality() }

    pub(crate) fn services(&self) -> &Services { self.runtime_manager.services() }

    pub(crate) fn component_registry(&self) -> &Arc<ComponentRegistry> { self.runtime_manager.component_registry() }

    pub(crate) fn state_store(&self) -> &Arc<dyn StateStore> { self.runtime_manager.state_store() }

    pub(crate) fn controller_manager(&self) -> &Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    pub(crate) fn participation(&self) -> &Arc<Participation> { self.runtime_manager.participation() }

    pub(crate) fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }
}

#[cfg(test)]
impl ChannelRuntime {
    /// A runtime for a channel of the session `session_id` that is not
    /// running - in tests: its session holds nothing but its id, and no
    /// rules, and has no loop.
    pub(crate) fn outside_a_session(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session_id: SessionId) -> Self {
        let session = runtime_manager.controller_manager().new_session_controller(crate::session::testing::bare_session(session_id));
        Self::new(runtime_manager, command_executor, session, Weak::new())
    }

    /// The runtime of the same channel built again, as after a restart -
    /// in tests: the same services, commands and session, and no channel
    /// attached yet.
    pub(crate) fn restarted(&self) -> Self {
        Self::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.command_executor), Arc::clone(&self.session), Weak::clone(&self.session_loop))
    }
}
