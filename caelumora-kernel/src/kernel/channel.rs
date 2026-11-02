//! `ChannelRuntime`: what a channel is built with (issues 360, 361), and the one source of the
//! runtimes of its participants.

use super::Acting;
use super::{Kernel, ParticipantRuntime};
use crate::actor::AgentKinds;
use crate::attachment::AttachmentManager;
use crate::channel::{ChannelLoop, Participation, ResolvedParticipant};
use crate::command::CommandExecutor;
use crate::component::ComponentRegistry;
use crate::controller::{ChannelController, ControllerManager, SessionController};
use crate::session::SessionLoop;
use caelumora_api::component::Services;
use caelumora_model::actor::ActorId;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::channel::Channel;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::security::Policies;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::session::Session;
use caelumora_model::store::history::HistoryStore;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::store::transcript::TranscriptStore;
use caelumora_model::store::StateStore;
use std::sync::{Arc, OnceLock, Weak};

/// What a channel runs with, made by its session (`SessionLoop::channel_runtime`) as the channel
/// is built: the process-wide services of the `Kernel` - among them the controllers of its
/// users and agents (issue 121) - the user commands of its session (issue 28), the session's
/// controller (issue 13) and its loop (issue 361), the channel's controller (issue 374) and a
/// handle to the channel's loop. It is the only source of `ParticipantRuntime`s, so every actor
/// lives in exactly one channel and shares its controllers, commands and handle.
#[derive(Debug, Clone)]
pub struct ChannelRuntime {
    pub(super) kernel: Arc<Kernel>,
    pub(super) command_executor: Arc<CommandExecutor>,
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
    pub fn new(
        kernel: Arc<Kernel>,
        command_executor: Arc<CommandExecutor>,
        session: Arc<SessionController>,
        channel: Arc<ChannelController>,
        session_loop: Weak<SessionLoop>,
    ) -> Self {
        Self {
            kernel,
            command_executor,
            session,
            session_loop,
            channel,
            channel_loop: ChannelHandle::default(),
        }
    }

    /// The runtime of `actor` in the channel this runtime is for, for a test: one that takes part
    /// in nothing, acting in its own name without roles (issue 516). The channel makes its actors'
    /// runtimes of their participants (`participant_runtime`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn new_participant_runtime(&self, actor: ActorId) -> ParticipantRuntime {
        let (session_id, channel_id) = (self.session.id(), self.channel.id());
        self.runtime_acting(actor, Acting::Itself { actor, session_id, channel_id })
    }

    /// The runtime of the participant `resolved` in the channel this runtime is for: acting in
    /// the participant's context as the controllers hold it now (issue 516).
    pub(crate) fn participant_runtime(&self, resolved: &ResolvedParticipant) -> ParticipantRuntime {
        let participant = resolved.clone();
        let channel = Arc::clone(self.channel_controller());
        self.runtime_acting(resolved.actor_id(), Acting::Participant { participant, channel })
    }

    fn runtime_acting(&self, actor: ActorId, acting: Acting) -> ParticipantRuntime {
        ParticipantRuntime {
            runtime: self.clone(),
            actor,
            session_id: self.session.id(),
            channel_id: self.channel.id(),
            acting,
        }
    }

    /// Makes `channel` reachable from the runtimes of its actors. Called once
    /// by `ChannelLoop::new`, as soon as the channel exists.
    pub(crate) fn attach(&self, channel: &Arc<ChannelLoop>) {
        let _ = self.channel_loop.0.set(Arc::downgrade(channel));
    }

    /// The controller of the channel's session.
    pub(crate) fn session_controller(&self) -> &Arc<SessionController> {
        &self.session
    }

    /// The controller of the channel (issue 374).
    pub(crate) fn channel_controller(&self) -> &Arc<ChannelController> {
        &self.channel
    }

    /// The loop of the channel this runtime is for, once it exists and while
    /// it does: what its actors' inboxes read its history through (issue
    /// 223).
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.channel_loop.get()
    }

    /// The loop of the channel's session, for the channel (`ChannelLoop::session_loop`); `None`
    /// once the session is gone, or for a channel of none, in tests.
    pub(crate) fn session_loop(&self) -> Option<Arc<SessionLoop>> {
        self.session_loop.upgrade()
    }
}

/// What a channel is built with (`ChannelLoop::new`), each read once there into what the channel
/// holds or builds (issue 360): the runtime it is given offers the services by name rather than
/// handing out the `Kernel` behind it (issue 191), and `ChannelLoop::new` takes no service
/// beside it.
impl ChannelRuntime {
    pub(crate) fn policies(&self) -> &Policies {
        self.kernel.policies()
    }

    pub(crate) fn services(&self) -> &Services {
        self.kernel.services()
    }

    pub(crate) fn component_registry(&self) -> &Arc<ComponentRegistry> {
        self.kernel.component_registry()
    }

    pub(crate) fn agent_kinds(&self) -> &Arc<AgentKinds> {
        self.kernel.agent_kinds()
    }

    pub(crate) fn state_store(&self) -> &Arc<dyn StateStore> {
        self.kernel.state_store()
    }
    pub(crate) fn history_store(&self) -> &Arc<dyn HistoryStore> {
        self.kernel.history_store()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn transcript_store(&self) -> &Arc<dyn TranscriptStore> {
        self.kernel.transcript_store()
    }

    /// The texts given to models (issue 57), for the tests' views.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn prompts(&self) -> &Arc<PromptCatalogue> {
        self.kernel.prompts()
    }

    pub(crate) fn controller_manager(&self) -> &Arc<ControllerManager> {
        self.kernel.controller_manager()
    }

    pub(crate) fn participation(&self) -> &Arc<Participation> {
        self.kernel.participation()
    }

    pub(crate) fn attachment_manager(&self) -> &Arc<AttachmentManager> {
        self.kernel.attachment_manager()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ChannelRuntime {
    /// A runtime for `channel` of `session`, which is not running - in
    /// tests: it has no loop and no user commands (the model's
    /// `session::testing::bare_session` and `channel::testing::bare_channel`
    /// for ones that hold nothing but their ids).
    pub fn outside_a_session(kernel: Arc<Kernel>, session: Session, channel: Channel) -> Self {
        let command_executor = Arc::new(CommandExecutor::empty(Arc::clone(kernel.access_policy())));
        let session = kernel.controller_manager().new_session_controller(session);
        let channel = kernel.controller_manager().new_channel_controller(channel);
        Self::new(kernel, command_executor, session, channel, Weak::new())
    }

    /// The controller of the channel's session, as its actors read it (`session_controller`): what
    /// a test changes of the session where the channel runs without its session's loop, as
    /// `ChannelLoop::session_controller_of` hands it out.
    pub fn session_controller_of(&self) -> Arc<SessionController> {
        Arc::clone(&self.session)
    }

    /// The runtime of the same channel built again, as after a restart -
    /// in tests: the same services, commands and session, the channel as it
    /// is stored now in a controller of its own, and no loop attached yet.
    pub fn restarted(&self) -> Self {
        let channel = self
            .kernel
            .controller_manager()
            .new_channel_controller(Channel::clone(&self.channel.load()));
        Self::new(
            Arc::clone(&self.kernel),
            Arc::clone(&self.command_executor),
            Arc::clone(&self.session),
            channel,
            Weak::clone(&self.session_loop),
        )
    }
}
