//! Who takes part in a channel: a participant with what its actor is built from, and whether it
//! may take part.

use super::ChannelError;
use crate::controller::{
    AgentController, ControllerManager, PersonalityController, UserController,
};
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality};
use caelumora_model::approval::Approver;
use caelumora_model::channel::{Channel, ChannelId, InternalActor, Participant};
use caelumora_model::confidentiality::{
    Classification, ConfidentialityPolicy, ConfidentialityQuestions,
};
use caelumora_model::role::RoleId;
use caelumora_model::scope::Scope;
use caelumora_model::security::{
    AccessPolicy, ActionType, Enforcement, Refused, SecurityAttributes, SecurityContext, JOIN,
};
use caelumora_model::session::SessionId;
use caelumora_model::store::{
    EntityKind, EntityStore, StoreError, StoreResult, StoreResultExt, Stored,
};
use std::iter;
use std::sync::Arc;

/// A participant as a document names it (`Participant`), resolved: with
/// what it is in this instance (`ResolvedKind`) - what its actor is built
/// from, and what its context and clearances follow from -, and the place
/// it takes part in: a channel of a session, or the session alone, as it is
/// taken in without a channel (issues 354, 356). Resolved, and asked
/// whether it may take part there, by `Participation`.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedParticipant {
    kind: ResolvedKind,
    session_id: SessionId,
    /// `None` for one taken into the session alone.
    channel_id: Option<ChannelId>,
}

/// What a participant is resolved to: the instance's controller of its
/// user or agent (issue 121) - or, for an internal actor of the channel
/// (issue 420), the approver itself. An agent comes with
/// the controller of its personality (issue 406), whose kind says how its
/// actor is built (issue 179, `AgentKinds`). What it is cleared for follows
/// from it as well (`ResolvedParticipant::clearances`).
#[derive(Debug, Clone)]
pub(crate) enum ResolvedKind {
    User(Arc<UserController>),
    Agent(Arc<AgentController>, Arc<PersonalityController>),
    Approver(Approver),
}

/// The question who takes part in a channel and whether they may, with the
/// services it is asked with (issue 356): the store of the agents - whether
/// one exists -, the controllers participants are built from, the
/// `AccessPolicy` and the `ConfidentialityPolicy`. One for the instance,
/// held by the `RuntimeManager`, asked by the session and channel loops and
/// the `SessionManager` alike - when a channel is created, restored or
/// built, when someone joins or is added, when an agent comes back and when
/// a channel is reclassified.
#[derive(Debug)]
pub(crate) struct Participation {
    agents: Arc<dyn EntityStore<Agent>>,
    controllers: Arc<ControllerManager>,
    access_policy: Arc<dyn AccessPolicy>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl Participation {
    pub(crate) fn new(
        agents: Arc<dyn EntityStore<Agent>>,
        controllers: Arc<ControllerManager>,
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self { agents, controllers, access_policy, confidentiality }
    }

    /// `participant` of channel `channel_id` of session `session_id` - of
    /// the session alone without one - resolved: with its controller, an
    /// agent also with its personality's, for its actor to be built from; `None` if its user or
    /// agent no longer exists - an agent as `agent_exists` says. An agent runs only with a
    /// personality it may read there (`require_runnable`).
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an
    /// agent's personality is gone - and `StoreError::PermissionDenied` if
    /// it is not the agent's to read: the agent exists, but cannot be run.
    pub(crate) async fn resolve(
        &self,
        participant: &Participant,
        session_id: SessionId,
        channel_id: Option<ChannelId>,
    ) -> Result<Option<ResolvedParticipant>> {
        let at = |kind| ResolvedParticipant { kind, session_id, channel_id };
        let agent = match participant {
            Participant::User(id) => {
                return Ok(self
                    .controllers
                    .user_controller(*id)
                    .await
                    .optional()?
                    .map(|user| at(ResolvedKind::User(user))))
            }
            Participant::Agent(id) => {
                if !self.agent_exists(*id).await? {
                    return Ok(None);
                }
                match self.controllers.agent_controller(*id).await.optional()? {
                    Some(agent) => agent,
                    None => return Ok(None),
                }
            }
        };
        let personality =
            self.controllers.personality_controller(agent.load().personality_id).await?;
        let resolved = at(ResolvedKind::Agent(agent, personality));
        self.require_runnable(&resolved).await?;
        Ok(Some(resolved))
    }

    /// Whether agent `id` exists - neither deleted nor purged -, asked of
    /// the agents' store rather than of the controllers: the instance keeps
    /// the controller of an agent while an actor of it runs, also once it
    /// was deleted (issue 314).
    ///
    /// # Errors
    /// Whatever the store reports but that there is none.
    pub(crate) async fn agent_exists(&self, id: AgentId) -> StoreResult<bool> {
        Ok(self.agents.get(&SecurityContext::system(), id).await.optional()?.is_some())
    }

    /// Nothing if `resolved` can be run: an agent only with a personality it
    /// may read, in its place (`GetPersonality`): one whose owner lost it - a
    /// share withdrawn - is not run (issue 191). A user and an approver have
    /// none to read.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if the personality is not the agent's
    /// to read.
    async fn require_runnable(&self, resolved: &ResolvedParticipant) -> StoreResult<()> {
        match (&resolved.kind, resolved.context()) {
            (ResolvedKind::Agent(_, personality), Some(context)) => {
                self.access_policy
                    .require(
                        &context,
                        ActionType::get(Personality::KIND, &personality.load().security_attributes),
                        StoreError::PermissionDenied,
                    )
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Nothing if `context` may take `resolved` into a session or channel of
    /// `kind` with `attributes` by joining (issue 221): the participant is whom
    /// `context` acts for, or owned by them - themselves, or their own agent
    /// (issue 341) - and an entry lets the participant join (`JOIN`), asked in the
    /// participant's own context, an agent by its owner.
    ///
    /// # Errors
    /// `refusal` of what was refused - joining, also for a participant that is
    /// not the caller's to take in.
    pub(crate) async fn require_joinable<E: Send>(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        kind: EntityKind,
        attributes: &SecurityAttributes,
        refusal: impl FnOnce(Refused) -> E + Send,
    ) -> Result<(), E> {
        let join = ActionType::custom(kind, &JOIN, attributes);
        match resolved.context() {
            Some(own) if own.owner_id == context.principal_id => {
                self.access_policy.require(&own, join, refusal).await
            }
            _ => Err(refusal(join.refused())),
        }
    }

    /// Nothing if `resolved` is cleared for its channel, classified
    /// `classification`, by every set of roles that binds it - to join it,
    /// to take part in it when it is created, or for what it would be
    /// reclassified to - asked in `context`, the context of whoever admits
    /// it (issue 304). A question of a channel: a participant of the session
    /// alone is not asked it, and answers nothing.
    ///
    /// # Errors
    /// `ChannelError::NotCleared` if it is not.
    pub(crate) fn require_cleared(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        classification: &Classification,
    ) -> Result<()> {
        let Some(channel) = resolved.channel_id else { return Ok(()) };
        let participant = resolved.actor_id();
        for roles in resolved.clearances() {
            self.confidentiality.require_cleared(
                context,
                participant,
                &roles,
                classification,
                || ChannelError::NotCleared { participant, channel },
            )?;
        }
        Ok(())
    }

    /// Nothing if `resolved` may take part in its channel, classified
    /// `classification`, asked in `context`: it lies where it may
    /// (`ResolvedParticipant::require_in_scope`) and is cleared for it
    /// (`require_cleared`).
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` or `NotCleared`.
    pub(crate) fn require_admissible(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        classification: &Classification,
    ) -> Result<()> {
        resolved.require_in_scope()?;
        self.require_cleared(resolved, context, classification)
    }

    /// Nothing if every one of `participants` that still exists may take
    /// part in `channel` (`require_admissible`) - asked before anything of a
    /// new or restored channel is stored: of the founders of a new one
    /// (issue 420), of the participants of a restored one.
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` or `NotCleared` for the first who may not;
    /// whatever reading a participant reports.
    pub(crate) async fn require_admitted(
        &self,
        context: &SecurityContext,
        channel: &Channel,
        participants: &[Participant],
    ) -> Result<()> {
        for participant in participants {
            if let Some(resolved) =
                self.resolve(participant, channel.session_id, Some(channel.id)).await?
            {
                self.require_admissible(&resolved, context, &channel.classification)?;
            }
        }
        Ok(())
    }
}

impl ResolvedParticipant {
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

    /// An approver of channel `channel_id` of session `session_id` - in
    /// tests, as the participant of an actor of whatever kind the channel
    /// only looks up.
    #[cfg(test)]
    pub(crate) fn approver(session_id: SessionId, channel_id: ChannelId) -> Self {
        use caelumora_model::approval::ApprovalStrategy;
        Self::internal(
            &InternalActor::Approver(Approver::new(ApprovalStrategy::default())),
            session_id,
            channel_id,
        )
    }

    /// What its actor is built from.
    pub(crate) fn kind(&self) -> &ResolvedKind {
        &self.kind
    }

    pub(crate) fn actor_id(&self) -> ActorId {
        match &self.kind {
            ResolvedKind::User(user) => ActorId::User(user.id()),
            ResolvedKind::Agent(agent, _) => ActorId::Agent(agent.id()),
            ResolvedKind::Approver(approver) => ActorId::Approver(approver.id),
        }
    }

    /// The context the participant acts in, in its place: a user's own, an
    /// agent's with its owner's rights and roles, limited by its
    /// personality's (issue 179) - as the controllers hold them now (issues
    /// 304, 306). None for an approver. A participant of the session alone acts
    /// in no channel, which a `SecurityContext` names as the default id.
    fn context(&self) -> Option<SecurityContext> {
        let (session, channel) = (self.session_id, self.channel_id.unwrap_or_default());
        match &self.kind {
            ResolvedKind::User(user) => Some(user.security_context(session, channel)),
            ResolvedKind::Agent(agent, personality) => {
                Some(agent.security_context(session, channel, personality.load().limiting_roles()))
            }
            ResolvedKind::Approver(_) => None,
        }
    }

    /// The sets of roles the participant has to be cleared by, each of them
    /// (issues 133, 179): a user's own; an agent's owner's - it must not
    /// see more than the person responsible for it - and, if its
    /// personality limits it, those roles too, which narrow the owner's and
    /// never widen them. None for an approver.
    fn clearances(&self) -> Vec<Vec<RoleId>> {
        let Some(context) = self.context() else { return Vec::new() };
        iter::once(context.roles).chain(context.limiting_roles).collect()
    }

    /// Nothing if the participant may take part in its channel by where it lies:
    /// an agent of a session's or a channel's scope only within it
    /// (`Scope::admits`, issue 178). A question of a channel: a participant of
    /// the session alone is not asked it, and answers nothing.
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` if it may not.
    fn require_in_scope(&self) -> Result<(), ChannelError> {
        let Some(channel_id) = self.channel_id else { return Ok(()) };
        let ResolvedKind::Agent(agent, _) = &self.kind else { return Ok(()) };
        if agent
            .load()
            .name
            .scope
            .admits(&Scope::ChannelScope { session_id: self.session_id, channel_id })
        {
            Ok(())
        } else {
            Err(ChannelError::OutOfScope { participant: self.actor_id(), channel: channel_id })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionManager};
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::testing::{make_personality, stored_agent};
    use caelumora_model::agent::Agent;
    use caelumora_model::channel::testing::main_channel;
    use caelumora_model::channel::{Channel, HistoryAccess, Participant};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{Grant, SecurityContext, Subject};
    use caelumora_model::session::testing::make_session;
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::user::testing::{make_user, record_users};
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::Name;
    use caelumora_model::value::name::ScopedName;
    use std::sync::Arc;

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn an_agent_is_cleared_only_as_far_as_its_owner_and_its_personality_are() {
        use crate::channel::ChannelError;
        use caelumora_model::agent::{Personality, PersonalityKind};
        use caelumora_model::confidentiality::{
            Clearance, Compartment, Compartments, Confidentiality, ConfidentialityRules,
        };
        use caelumora_model::role::{RoleId, Roles};
        use caelumora_model::user::User;

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            (
                RoleId::new(),
                "staff".to_string(),
                vec![Clearance { level: Confidentiality::Internal, compartment: None }],
            ),
            (
                RoleId::new(),
                "hr".to_string(),
                vec![Clearance {
                    level: Confidentiality::Confidential,
                    compartment: Some(Compartment::new("hr")),
                }],
            ),
        ])
        .unwrap();
        let ids = roles.clone();
        let (_, manager) = owned_runtime(ConfidentialityRules::new(compartments).with_roles(roles));
        let runtime = Arc::clone(manager.runtime());
        let user = |name: &str, roles: &[&str]| User {
            roles: ids.ids_of(roles).unwrap(),
            ..make_user(name)
        };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        // An agent of `owner` whose personality is of `kind` and names `roles`.
        let agent = |name: &'static str,
                     owner: ActorId,
                     kind: PersonalityKind,
                     roles: Option<&'static [&'static str]>| {
            let runtime = Arc::clone(&runtime);
            let ids = ids.clone();
            async move {
                let personality = Personality {
                    kind,
                    roles: roles.map(|roles| ids.ids_of(roles).unwrap()),
                    ..make_personality(name)
                };
                let agent = stored_agent(runtime.entity_stores(), name, personality, owner).await;
                ActorId::Agent(agent.id)
            }
        };
        // Stored first: their roles are read from the store.
        let users = record_users(runtime.entity_stores(), &[&alice, &bob]).await;
        let hers = agent("hers", alice_id, PersonalityKind::Internal, None).await;
        let limited = agent("limited", alice_id, PersonalityKind::Internal, Some(&["staff"])).await;
        let bobs = agent("bobs", bob_id, PersonalityKind::Internal, None).await;
        let remote = agent("remote", alice_id, PersonalityKind::External, None).await;
        let cleared_remote =
            agent("cleared", alice_id, PersonalityKind::External, Some(&["staff"])).await;

        let hr =
            Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let internal = Classification::general(Confidentiality::Internal);
        let cleared = |participant: ActorId, classification: &Classification| {
            let runtime = Arc::clone(&runtime);
            let (alice_ctx, classification) = (alice_ctx.clone(), classification.clone());
            async move {
                // A channel of only `participant` - and its approver, who is
                // cleared for anything.
                let ActorId::Agent(id) = participant else { unreachable!("agents only") };
                let channel = Channel::build(
                    SessionId::new(),
                    alice_id,
                    Name::new("probe").unwrap(),
                    Default::default(),
                    classification,
                    None,
                );
                runtime
                    .participation()
                    .require_admitted(&alice_ctx, &channel, &[Participant::Agent(id)])
                    .await
                    .is_ok()
            }
        };
        assert!(cleared(hers, &hr).await, "an agent that names no roles is cleared as its owner");
        assert!(!cleared(limited, &hr).await, "its personality limits it below its owner");
        assert!(cleared(limited, &internal).await, "within its limit");
        assert!(!cleared(bobs, &hr).await, "its owner is not cleared");
        assert!(
            !cleared(remote, &internal).await,
            "an external agent that names no roles sees only what is public"
        );
        assert!(cleared(remote, &Classification::PUBLIC).await);
        assert!(cleared(cleared_remote, &internal).await, "one that names its roles, within them");
        assert!(!cleared(cleared_remote, &hr).await);

        // The same question when a channel is created and when one is joined.
        let mut session = make_session("hers", alice.id);
        session.security_attributes =
            session.security_attributes.clone().granting(Subject::Actor(bob_id), Grant::Read);
        session.participants = users;
        session.participants.extend([hers, limited].map(|a| match a {
            ActorId::Agent(id) => Participant::Agent(id),
            _ => unreachable!(),
        }));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let with = |name: &str, participants| ChannelSpec {
            classification: Some(hr.clone()),
            ..spec(name, participants)
        };
        let refused = session
            .create_channel(&alice_ctx, with("hr", vec![limited]))
            .await
            .expect_err("limited below HR");
        assert!(
            matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { participant, .. }) if *participant == limited),
            "{refused}"
        );
        let channel = session
            .create_channel(&alice_ctx, with("hr", vec![hers]))
            .await
            .expect("cleared as alice");
        let joining = channel
            .add_participant(&alice_ctx, limited, HistoryAccess::All)
            .await
            .expect_err("nor may it join");
        assert!(
            matches!(joining.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })),
            "{joining}"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_agent_takes_part_only_within_its_sessions_or_channels_scope() {
        use crate::channel::ChannelError;

        let runtime = test_runtime();
        let personalities = Arc::clone(&runtime.entity_stores().personalities);
        let agents = Arc::clone(&runtime.entity_stores().agents);
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let mut personality = make_personality("plain");
        personalities.create(&system, &mut personality).await.unwrap();
        let mut session = make_session("scoped", UserId::new());
        let id = session.id;
        let mut elsewhere = [
            Agent::new(
                ScopedName::new(
                    "aside",
                    Scope::ChannelScope { session_id: id, channel_id: ChannelId::new() },
                )
                .unwrap(),
                personality.id,
                ActorId::User(UserId::new()),
            ),
            Agent::new(
                ScopedName::new("visitor", Scope::SessionScope { session_id: SessionId::new() })
                    .unwrap(),
                personality.id,
                ActorId::User(UserId::new()),
            ),
        ];
        for agent in &mut elsewhere {
            agents.create(&system, agent).await.unwrap();
            session.participants.push(Participant::Agent(agent.id));
        }
        // In the session, not in its channel: adding them there is refused below.
        let main = Channel { participants: Vec::new(), ..main_channel(&session) };
        let running = manager.create(&ctx, session, vec![main]).await.unwrap();
        let main = running.channel_named("main").unwrap();

        for agent in &elsewhere {
            let actor = ActorId::Agent(agent.id);
            let refused = running
                .create_channel(&ctx, spec("with-it", vec![actor]))
                .await
                .expect_err("not in a new channel");
            assert!(
                matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::OutOfScope { participant, .. }) if *participant == actor),
                "{refused}"
            );
            let refused = main
                .add_participant(&ctx, actor, HistoryAccess::All)
                .await
                .expect_err("nor in one there is");
            assert!(
                matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::OutOfScope { participant, .. }) if *participant == actor),
                "{refused}"
            );
        }
        assert!(running.channel_named("with-it").is_none(), "nothing was created");
        assert!(main.channel().participants.is_empty(), "nor added");
        manager.stop(&system, id).await.unwrap();
    }
}
