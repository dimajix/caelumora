//! `ResolvedParticipant`: a participant as a document names it, resolved to what it is in this
//! instance - what its actor is built from, and what its context and clearances follow from.

use crate::controller::{
    AgentController, ControllerManager, PersonalityController, UserController,
};
use anyhow::Result;
use caelumora_domain::actor::ActorId;
use caelumora_domain::approval::Approver;
use caelumora_domain::channel::{Channel, ChannelId, InternalActor, Participant, ThreadId};
use caelumora_domain::role::RoleId;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::StoreResultExt;
use caelumora_security::SecurityContext;
use std::iter;
use std::sync::Arc;

/// A participant as a document names it (`Participant`), resolved: with
/// what it is in this instance (`ResolvedKind`) - what its actor is built
/// from, and what its context and clearances follow from -, and the place
/// it takes part in: a channel of a session, or the session alone, as it is
/// taken in without a channel (issues 354, 356). Resolved by `resolve`, and asked
/// whether it may take part there by `Participation`.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedParticipant {
    kind: ResolvedKind,
    session_id: SessionId,
    /// `None` for one taken into the session alone.
    channel_id: Option<ChannelId>,
}

/// What a participant is resolved to: the instance's controller of its
/// user or agent (issue 121) - or, for an internal actor of the channel
/// (issue 420), the approver itself, or the thread a delegate delegates into (issue 557). An
/// agent comes with the controller of its personality (issue 406), whose kind says how its
/// actor is built (issue 179, `AgentKinds`). What it is cleared for follows
/// from it as well (`ResolvedParticipant::clearances`).
#[derive(Debug, Clone)]
pub(crate) enum ResolvedKind {
    User(Arc<UserController>),
    Agent(Arc<AgentController>, Arc<PersonalityController>),
    Approver(Approver),
    Delegate(ThreadId),
}

impl ResolvedParticipant {
    /// `participant` of channel `channel_id` of session `session_id` - of the session alone
    /// without one - resolved by `controllers`: with its controller, an agent also with its
    /// personality's, for its own context and for its actor to be built from; `None` if its user or
    /// agent no longer exists, an agent as `ControllerManager::agent_exists` says. Asks nothing:
    /// whether it may run is `Participation::require_runnable`'s, asked by whoever builds its actor
    /// or admits it.
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an agent's personality is gone.
    pub(crate) async fn resolve(
        controllers: &ControllerManager,
        participant: &Participant,
        session_id: SessionId,
        channel_id: Option<ChannelId>,
    ) -> Result<Option<Self>> {
        let at = |kind| Self { kind, session_id, channel_id };
        let agent = match participant {
            Participant::User(id) => {
                return Ok(controllers
                    .user_controller(*id)
                    .await
                    .optional()?
                    .map(|user| at(ResolvedKind::User(user))))
            }
            Participant::Agent(id) => {
                if !controllers.agent_exists(*id).await? {
                    return Ok(None);
                }
                match controllers.agent_controller(*id).await.optional()? {
                    Some(agent) => agent,
                    None => return Ok(None),
                }
            }
        };
        let personality = controllers.personality_controller(agent.load().personality_id).await?;
        Ok(Some(at(ResolvedKind::Agent(agent, personality))))
    }

    /// Those of `participants`, of `channel`, that still exist, resolved (`resolve`): whom
    /// `Participation::require_admitted` is asked of.
    ///
    /// # Errors
    /// As `resolve`.
    pub(crate) async fn resolve_existing(
        controllers: &ControllerManager,
        participants: &[Participant],
        channel: &Channel,
    ) -> Result<Vec<Self>> {
        let mut resolved = Vec::with_capacity(participants.len());
        for participant in participants {
            let found =
                Self::resolve(controllers, participant, channel.session_id, Some(channel.id));
            resolved.extend(found.await?);
        }
        Ok(resolved)
    }

    /// `internal`, an internal actor of channel `channel_id` of session
    /// `session_id` (issue 420): what its actor is built from, nothing to
    /// resolve.
    pub(crate) fn internal(
        internal: &InternalActor,
        session_id: SessionId,
        channel_id: ChannelId,
    ) -> Self {
        let kind = match internal {
            InternalActor::Approver(approver) => ResolvedKind::Approver(approver.clone()),
        };
        Self { kind, session_id, channel_id: Some(channel_id) }
    }

    /// The delegate of `thread`, of channel `channel_id` of session `session_id` (issue 557): an
    /// internal actor of the channel, which nothing lists - the thread it delegates into is what
    /// its actor is built from.
    pub(crate) fn delegate(thread: ThreadId, session_id: SessionId, channel_id: ChannelId) -> Self {
        Self { kind: ResolvedKind::Delegate(thread), session_id, channel_id: Some(channel_id) }
    }

    /// What its actor is built from.
    pub(crate) fn kind(&self) -> &ResolvedKind {
        &self.kind
    }

    /// The session it takes part in.
    pub(super) fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The channel it takes part in; `None` for one taken into the session alone.
    pub(super) fn channel_id(&self) -> Option<ChannelId> {
        self.channel_id
    }

    pub(crate) fn actor_id(&self) -> ActorId {
        match &self.kind {
            ResolvedKind::User(user) => ActorId::User(user.id()),
            ResolvedKind::Agent(agent, _) => ActorId::Agent(agent.id()),
            ResolvedKind::Approver(approver) => ActorId::Approver(approver.id),
            ResolvedKind::Delegate(thread) => ActorId::Delegate(*thread),
        }
    }

    /// The context the participant acts in, in its place: a user's own, an
    /// agent's with its owner's rights and roles, limited by its
    /// personality's (issue 179) - as the controllers hold them now (issues
    /// 304, 306). None for an internal actor of the channel. A participant of the session alone
    /// acts in no channel, which a `SecurityContext` names as the default id.
    pub(super) fn context(&self) -> Option<SecurityContext> {
        let (session, channel) = (self.session_id, self.channel_id.unwrap_or_default());
        match &self.kind {
            ResolvedKind::User(user) => Some(user.security_context(session, channel)),
            ResolvedKind::Agent(agent, personality) => {
                Some(agent.security_context(session, channel, personality.load().limiting_roles()))
            }
            ResolvedKind::Approver(_) | ResolvedKind::Delegate(_) => None,
        }
    }

    /// The context the participant acts in, in `channel`, for `initiator` (issue 516): a user's
    /// own, an agent's with its owner's rights and roles, limited by its personality, as the
    /// controllers hold them now, the approver's and a delegate's - internal actors of the
    /// channel - answered for by the channel's owner (`SecurityContext::of_approver`,
    /// `of_delegate`).
    pub(crate) fn context_in(&self, channel: &Channel, initiator: ActorId) -> SecurityContext {
        let own = match &self.kind {
            ResolvedKind::User(user) => user.security_context(channel.session_id, channel.id),
            ResolvedKind::Agent(agent, personality) => agent.security_context(
                channel.session_id,
                channel.id,
                personality.load().limiting_roles(),
            ),
            ResolvedKind::Approver(approver) => SecurityContext::of_approver(approver, channel),
            ResolvedKind::Delegate(thread) => SecurityContext::of_delegate(*thread, channel),
        };
        SecurityContext { initiator_id: initiator, ..own }
    }

    /// The sets of roles the participant has to be cleared by, each of them
    /// (issues 133, 179): a user's own; an agent's owner's - it must not
    /// see more than the person responsible for it - and, if its
    /// personality limits it, those roles too, which narrow the owner's and
    /// never widen them. None for an internal actor of the channel.
    pub(super) fn clearances(&self) -> Vec<Vec<RoleId>> {
        let Some(context) = self.context() else { return Vec::new() };
        iter::once(context.roles).chain(context.limiting_roles).collect()
    }
}
