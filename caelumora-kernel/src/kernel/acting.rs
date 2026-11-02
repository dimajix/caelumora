//! In whose name something acts or is built (`Acting`, issues 516, 520): what the kernel builds the
//! context of each call from, never handed in by whoever acts.

use crate::channel::ResolvedParticipant;
use crate::controller::ChannelController;
use caelumora_model::actor::ActorId;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::channel::ChannelId;
use caelumora_model::security::SecurityContext;
#[cfg(any(test, feature = "test-support"))]
use caelumora_model::session::SessionId;
use std::sync::Arc;

/// In whose name something acts or is built: the context of each of its calls, built by the kernel
/// when the call is made - a participant's from what the controllers hold of it then, so that a
/// role taken away applies at the next call -, or one given once.
#[derive(Debug, Clone)]
pub(crate) enum Acting {
    /// A participant of a channel, as the channel resolved it: its context as the controllers hold
    /// it now - a user's own, an agent's with its owner's rights and roles, limited by its
    /// personality, the approver's answered for by the channel's owner -, in its channel as it
    /// stands.
    Participant { participant: ResolvedParticipant, channel: Arc<ChannelController> },
    /// A context given once, whatever the initiator: a channel's or a session's owner's as it was
    /// when their components were built (`ControllerManager::owner_context`), the system's; in a
    /// test, the one it gives (`ParticipantRuntime::acting_as`).
    Given(SecurityContext),
    /// In a test, an actor that takes part in nothing: in its own name, without roles.
    #[cfg(any(test, feature = "test-support"))]
    Itself { actor: ActorId, session_id: SessionId, channel_id: ChannelId },
}

impl Acting {
    /// The context it acts in now in its own name: of what it does of its own accord, and what it
    /// is built in.
    pub(crate) fn context(&self) -> SecurityContext {
        match self {
            Self::Participant { participant, .. } => self.context_for(participant.actor_id()),
            Self::Given(context) => context.clone(),
            #[cfg(any(test, feature = "test-support"))]
            Self::Itself { actor, .. } => self.context_for(*actor),
        }
    }

    /// The context it acts in now for `initiator` - who began the chain it answers, for the record,
    /// never for rights.
    pub(crate) fn context_for(&self, initiator: ActorId) -> SecurityContext {
        match self {
            Self::Participant { participant, channel } => {
                participant.context_in(&channel.load(), initiator)
            }
            Self::Given(context) => context.clone(),
            #[cfg(any(test, feature = "test-support"))]
            Self::Itself { actor, session_id, channel_id } => SecurityContext {
                initiator_id: initiator,
                ..SecurityContext::new(*actor, *session_id, *channel_id)
            },
        }
    }
}
