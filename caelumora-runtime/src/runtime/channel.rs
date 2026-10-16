use crate::actor::AgentKinds;
use crate::channel::{ChannelLoop, Participation};
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::ComponentRegistry;
use crate::controller::{ChannelController, ControllerManager, SessionController};
use crate::session::SessionLoop;
use crate::recorder::Recorder;
use anyhow::Result;
use caelumora_model::actor::ActorId;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::channel::Channel;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::session::Session;
use caelumora_model::security::AccessPolicy;
use caelumora_model::store::StateStore;
use caelumora_model::store::history::HistoryStore;
use caelumora_model::confidentiality::ConfidentialityPolicy;
use std::sync::{Arc, OnceLock, Weak};
use super::{ActorRuntime, RuntimeManager, Services};

/// What a channel runs with, made by its session (`SessionLoop::channel_runtime`) as the channel
/// is built: the process-wide services of the `RuntimeManager` - among them the controllers of its
/// users and agents (issue 121) - the user commands of its session (issue 28), the session's
/// controller (issue 13) and its loop (issue 361), the channel's controller (issue 374) and a
/// handle to the channel's loop. It is the only source of `ActorRuntime`s, so every actor lives in
/// exactly one channel and shares its controllers, commands and handle.
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
    /// The channel, live: what the channel's loop is built from and holds,
    /// and what its actors read it by. Held, not reached through
    /// `channel_loop`, for the same reason as `session`.
    channel: Arc<ChannelController>,
    pub(super) channel_loop: ChannelHandle,
}

/// The loop of the channel its actors run in, reachable from what they are
/// built with once it exists: set by `ChannelLoop::new` as soon as the loop
/// does, after its actors were built. Weak, because the channel owns its
/// actors and they hold this handle: a strong reference would keep a stopped
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
    /// A runtime for the channel `channel` holds, of the session `session` holds and
    /// `session_loop` runs, whose user commands are `command_executor`.
    pub fn new(runtime_manager: Arc<RuntimeManager>, command_executor: Arc<CommandExecutor<dyn ChannelCommandProvider>>, session: Arc<SessionController>, channel: Arc<ChannelController>, session_loop: Weak<SessionLoop>) -> Self {
        Self { runtime_manager, command_executor, session, session_loop, channel, channel_loop: ChannelHandle::default() }
    }

    /// The runtime of `actor` in the channel this runtime is for, whose loop
    /// is not attached yet while its actors are built.
    pub fn new_actor_runtime(&self, actor: ActorId) -> ActorRuntime {
        ActorRuntime { runtime: self.clone(), actor, session_id: self.session.id(), channel_id: self.channel.id() }
    }

    /// Makes `channel` reachable from the runtimes of its actors. Called once
    /// by `ChannelLoop::new`, as soon as the channel exists.
    pub(crate) fn attach(&self, channel: &Arc<ChannelLoop>) {
        let _ = self.channel_loop.0.set(Arc::downgrade(channel));
    }

    /// The controller of the channel's session.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> { &self.session }

    /// The controller of the channel (issue 374).
    pub(crate) fn channel_controller(&self) -> &Arc<ChannelController> { &self.channel }

    /// The loop of the channel's session, for the channel (`ChannelLoop::session_loop`); `None`
    /// once the session is gone, or for a channel of none, in tests.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> { self.session_loop.upgrade() }
}

/// What a channel is built with (`ChannelLoop::new`), each read once there into what the channel
/// holds or builds (issue 360): the runtime it is given offers the services by name rather than
/// handing out the `RuntimeManager` behind it (issue 191), and `ChannelLoop::new` takes no service
/// beside it.
impl ChannelRuntime {
    pub(crate) fn access_policy(&self) -> &Arc<dyn AccessPolicy> { self.runtime_manager.access_policy() }

    pub(crate) fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { self.runtime_manager.confidentiality() }

    pub(crate) fn services(&self) -> &Services { self.runtime_manager.services() }

    pub(crate) fn component_registry(&self) -> &Arc<ComponentRegistry> { self.runtime_manager.component_registry() }

    pub(crate) fn agent_kinds(&self) -> &Arc<AgentKinds> { self.runtime_manager.agent_kinds() }

    pub(crate) fn state_store(&self) -> &Arc<dyn StateStore> { self.runtime_manager.state_store() }
    pub(crate) fn history_store(&self) -> &Arc<dyn HistoryStore> { self.runtime_manager.history_store() }

    pub(crate) fn controller_manager(&self) -> &Arc<ControllerManager> { self.runtime_manager.controller_manager() }

    pub(crate) fn participation(&self) -> &Arc<Participation> { self.runtime_manager.participation() }

    pub(crate) fn new_recorder(&self) -> Result<Box<dyn Recorder>> { self.runtime_manager.new_recorder() }
}

#[cfg(any(test, feature = "test-support"))]
impl ChannelRuntime {
    /// A runtime for `channel` of `session`, which is not running - in
    /// tests: it has no loop and no user commands (`bare_session` and
    /// `bare_channel` for ones that hold nothing but their ids).
    pub fn outside_a_session(runtime_manager: Arc<RuntimeManager>, session: Session, channel: Channel) -> Self {
        let command_executor = Arc::new(CommandExecutor::empty(Arc::clone(runtime_manager.access_policy())));
        let session = runtime_manager.controller_manager().new_session_controller(session);
        let channel = runtime_manager.controller_manager().new_channel_controller(channel);
        Self::new(runtime_manager, command_executor, session, channel, Weak::new())
    }

    /// The runtime of the same channel built again, as after a restart -
    /// in tests: the same services, commands and session, the channel as it
    /// is stored now in a controller of its own, and no loop attached yet.
    pub fn restarted(&self) -> Self {
        let channel = self.runtime_manager.controller_manager().new_channel_controller(Channel::clone(&self.channel.load()));
        Self::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.command_executor), Arc::clone(&self.session), channel, Weak::clone(&self.session_loop))
    }
}
