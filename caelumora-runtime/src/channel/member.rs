//! Who takes part in a channel: a participant with what its actor is built from, and whether it
//! may take part.

use crate::runtime::RuntimeManager;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, ChannelId, Participant};
use crate::controller::{AgentController, ControllerManager, UserController};
use caelumora_model::agent::{Personality, PersonalityKind};
use caelumora_model::approval::Approver;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::role::RoleId;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::scope::Scope;
use caelumora_model::session::SessionId;
use caelumora_model::store::{StoreError, StoreResultExt};
use std::sync::Arc;
use super::ChannelError;

/// A participant with what its actor is built from: the instance's
/// controller of its user or agent (issue 121), or the approver itself. An
/// agent is run here or elsewhere, as its personality's kind says (issue
/// 179). What it is cleared for follows from it as well (`clearances`).
#[derive(Debug, Clone)]
pub(super) enum Member {
    User(Arc<UserController>),
    Agent(Arc<AgentController>, Personality),
    ExternalAgent(Arc<AgentController>, Personality),
    Approver(Approver),
}

impl Member {
    /// `participant` of `channel` with its controller - an agent also with
    /// its personality, read once, for its actor to be built from - or
    /// `None` if its user or agent no longer exists. An agent runs only with
    /// a personality it may read, in the channel, as `security` decides
    /// (`GetPersonality`): one whose owner lost it - a share withdrawn - is
    /// not run (issue 191).
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an
    /// agent's personality is gone - and `StoreError::PermissionDenied` if
    /// it is not the agent's to read: the agent exists, but cannot be run.
    pub(super) async fn of(participant: &Participant, channel: &Channel, controllers: &ControllerManager, security: &dyn SecurityManager) -> Result<Option<Self>> {
        let agent = match participant {
            Participant::User(id) => return Ok(controllers.user_controller(*id).await.optional()?.map(Member::User)),
            Participant::Approver(approver) => return Ok(Some(Member::Approver(approver.clone()))),
            Participant::Agent(id) => match controllers.agent_controller(*id).await.optional()? {
                Some(agent) => agent,
                None => return Ok(None),
            },
        };
        let personality = agent.personality().await?;
        let context = agent.security_context(channel.session_id, channel.id, personality.limiting_roles());
        security.require(&context, ActionType::GetPersonality(&personality.security_attributes), || StoreError::PermissionDenied).await?;
        Ok(Some(match personality.kind {
            PersonalityKind::Internal => Member::Agent(agent, personality),
            PersonalityKind::External => Member::ExternalAgent(agent, personality),
        }))
    }

    pub(super) fn actor_id(&self) -> ActorId {
        match self {
            Member::User(user) => ActorId::User(user.id()),
            Member::Agent(agent, _) | Member::ExternalAgent(agent, _) => ActorId::Agent(agent.id()),
            Member::Approver(approver) => ActorId::Approver(approver.id),
        }
    }

    /// The sets of roles the member has to be cleared by, each of them
    /// (issues 133, 179): a user's own; an agent's owner's - it must not
    /// see more than the person responsible for it - and, if its
    /// personality limits it, those roles too, which narrow the owner's and
    /// never widen them. None for an approver. As the controllers hold them
    /// now (issues 304, 306), in `session` and `channel`.
    fn clearances(&self, session: SessionId, channel: ChannelId) -> Vec<Vec<RoleId>> {
        let context = match self {
            Member::User(user) => user.security_context(session, channel),
            Member::Agent(agent, personality) | Member::ExternalAgent(agent, personality) => agent.security_context(session, channel, personality.limiting_roles()),
            Member::Approver(_) => return Vec::new(),
        };
        std::iter::once(context.roles).chain(context.limiting_roles).collect()
    }

    /// Nothing if the member may take part in channel `channel_id` of
    /// session `session_id` by where it lies: an agent of a session's or a
    /// channel's scope only within it (`Scope::admits`, issue 178).
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` if it may not.
    pub(super) fn require_in_scope(&self, session_id: SessionId, channel_id: ChannelId) -> Result<(), ChannelError> {
        let (Member::Agent(agent, _) | Member::ExternalAgent(agent, _)) = self else { return Ok(()) };
        if agent.load().name.scope.admits(&Scope::ChannelScope { session_id, channel_id }) {
            Ok(())
        } else {
            Err(ChannelError::OutOfScope { participant: self.actor_id(), channel: channel_id })
        }
    }

    /// Nothing if the member is cleared for channel `channel`, classified
    /// `classification`, by every set of roles that binds it - to join it,
    /// to take part in it when it is created, or for what it would be
    /// reclassified to - as `confidentiality` decides, asked in `context`,
    /// the context of whoever admits it (issue 304).
    ///
    /// # Errors
    /// `ChannelError::NotCleared` if it is not.
    pub(super) fn require_cleared(&self, confidentiality: &dyn ConfidentialityPolicy, context: &SecurityContext, classification: &Classification, session: SessionId, channel: ChannelId) -> Result<()> {
        let participant = self.actor_id();
        for roles in self.clearances(session, channel) {
            confidentiality.require_cleared(context, participant, &roles, classification, || ChannelError::NotCleared { participant, channel })?;
        }
        Ok(())
    }
}

/// Nothing if every participant of `channel` that still exists may take
/// part in it - lies where it may (`Member::require_in_scope`) and is
/// cleared for its classification (`Member::require_cleared`) - asked
/// before anything of a new or restored channel or session is stored.
///
/// # Errors
/// `ChannelError::OutOfScope` or `NotCleared` for the first who may not;
/// whatever reading a participant reports.
pub(crate) async fn require_admitted(runtime: &RuntimeManager, context: &SecurityContext, channel: &Channel) -> Result<()> {
    for participant in &channel.participants {
        if let Some(member) = Member::of(participant, channel, runtime.controller_manager(), runtime.security_manager().as_ref()).await? {
            member.require_in_scope(channel.session_id, channel.id)?;
            member.require_cleared(runtime.confidentiality().as_ref(), context, &channel.classification, channel.session_id, channel.id)?;
        }
    }
    Ok(())
}
