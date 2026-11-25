//! `Participation`: the decisions on participants - whether one may run, take part in a channel,
//! and act for whom, by its own act or as its owner.

use super::{ChannelError, ResolvedKind, ResolvedParticipant};
use anyhow::Result;
use caelumora_domain::agent::Personality;
use caelumora_domain::channel::Channel;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::scope::Scope;
use caelumora_domain::security::Refused;
use caelumora_domain::store::{StoreError, StoreResult, Stored};
use caelumora_security::confidentiality::ConfidentialityQuestions;
use caelumora_security::{ActionType, Enforcement, Policies, SecurityContext};

/// The decisions on participants (issue 356): who may run, take part in a channel, and act for
/// whom - asked by whoever enforces them: a loop as it acts, a procedure as it acts or ahead, each
/// refusal turned into the asker's error. One for the instance, held by the `Kernel`, deciding by
/// its `AccessPolicy` and `ConfidentialityPolicy` alone; who a participant is, it is given
/// (`ResolvedParticipant::resolve`).
#[derive(Debug)]
pub(crate) struct Participation {
    policies: Policies,
}

impl Participation {
    /// The decisions on participants, by `policies`.
    pub(crate) fn new(policies: &Policies) -> Self {
        Self { policies: policies.clone() }
    }

    /// Nothing if `resolved` can be run: an agent only with a personality it may read, in its place
    /// (`GetPersonality`): one whose owner lost it - a share withdrawn - is not run (issue 191). A
    /// user and an approver have none to read. Asked of every participant before its actor is
    /// built, and before it is admitted to a channel.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if the personality is not the agent's to read.
    pub(crate) async fn require_runnable(&self, resolved: &ResolvedParticipant) -> StoreResult<()> {
        match (resolved.kind(), resolved.context()) {
            (ResolvedKind::Agent(_, personality), Some(context)) => {
                self.policies
                    .access_policy()
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

    /// Nothing if `context` may do `action` for `resolved` by its own act - join a session or
    /// channel (`JOIN`, issue 221), a thread (`GetChannel`, issue 602): the participant is whom
    /// `context` acts for, or owned by them - themselves, or their own agent (issue 341) - and
    /// `action` is permitted in the participant's own context, an agent's by its owner.
    ///
    /// # Errors
    /// `refusal` of what was refused - `action`, also for a participant that is not the caller's
    /// own.
    pub(crate) async fn require_own_act<E: Send>(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        action: ActionType<'_>,
        refusal: impl FnOnce(Refused) -> E + Send,
    ) -> Result<(), E> {
        self.require_in(own_context(resolved, context), action, refusal).await
    }

    /// Nothing if `context` may have `resolved` leave by its own act - a channel (`LeaveChannel`,
    /// issue 562) or a session (`LeaveSession`, issue 590) -, asked as `require_own_act` asks it;
    /// or if `context` is the kernel's own, which may have anyone leave so (issue 598: a thread
    /// closed with the agents it owns, once its delegation was let go), `action` asked in the
    /// participant's own context all the same.
    ///
    /// # Errors
    /// As `require_own_act`.
    pub(crate) async fn require_own_leave<E: Send>(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        action: ActionType<'_>,
        refusal: impl FnOnce(Refused) -> E + Send,
    ) -> Result<(), E> {
        let own =
            if context.is_system() { resolved.context() } else { own_context(resolved, context) };
        self.require_in(own, action, refusal).await
    }

    /// Nothing if `action` is permitted in `own`, the participant's own context; `refusal` of it
    /// if it is not, or if there is none.
    async fn require_in<E: Send>(
        &self,
        own: Option<SecurityContext>,
        action: ActionType<'_>,
        refusal: impl FnOnce(Refused) -> E + Send,
    ) -> Result<(), E> {
        match own {
            Some(own) => self.policies.access_policy().require(&own, action, refusal).await,
            None => Err(refusal(action.refused())),
        }
    }

    /// Nothing if `resolved` is whom `context` acts for, or owned by them - themselves, or their
    /// own agent (issue 341) -, for `action`, which is asked of `context` itself, not in the
    /// participant's context: whom an opener founds a thread with (`OpenThread`, issue 557), as
    /// `require_own_act` asks it of whoever takes part by their own act.
    ///
    /// # Errors
    /// `refusal` of `action` if it is not.
    pub(crate) fn require_own<E>(
        &self,
        resolved: &ResolvedParticipant,
        context: &SecurityContext,
        action: ActionType<'_>,
        refusal: impl FnOnce(Refused) -> E,
    ) -> Result<(), E> {
        own_context(resolved, context).map(|_| ()).ok_or_else(|| refusal(action.refused()))
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
        let Some(channel) = resolved.channel_id() else { return Ok(()) };
        let participant = resolved.actor_id();
        for roles in resolved.clearances() {
            self.policies.confidentiality().require_cleared(
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
    /// (`require_in_scope`) and is cleared for it
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
        require_in_scope(resolved)?;
        self.require_cleared(resolved, context, classification)
    }

    /// Nothing if every one of `participants` can run (`require_runnable`) and, with `channel`,
    /// may take part in it (`require_admissible`): what is asked before anyone is taken in - into
    /// a session, and into a channel of it where one is named (`procedure::participant`) -, before
    /// anything of a new or restored channel is stored, of the founders of a new one (issue 420)
    /// and the participants of a restored one, those that still exist
    /// (`ResolvedParticipant::resolve_existing`), and what says whom one cannot take in
    /// (`SessionView::candidates`). Each is asked in turn, once all are resolved: whatever
    /// resolving them reported comes before a refusal.
    ///
    /// # Errors
    /// `ChannelError::OutOfScope` or `NotCleared` for the first who may not;
    /// `StoreError::PermissionDenied` for an agent that cannot run.
    pub(crate) async fn require_admitted(
        &self,
        context: &SecurityContext,
        channel: Option<&Channel>,
        participants: &[ResolvedParticipant],
    ) -> Result<()> {
        for participant in participants {
            self.require_runnable(participant).await?;
            if let Some(channel) = channel {
                self.require_admissible(participant, context, &channel.classification)?;
            }
        }
        Ok(())
    }
}

/// Nothing if `resolved` may take part in its channel by where it lies: an agent of a session's or
/// a channel's scope only within it (`Scope::admits`, issue 178). A question of a channel: a
/// participant of the session alone is not asked it, and answers nothing.
///
/// # Errors
/// `ChannelError::OutOfScope` if it may not.
fn require_in_scope(resolved: &ResolvedParticipant) -> Result<(), ChannelError> {
    let Some(channel_id) = resolved.channel_id() else { return Ok(()) };
    let ResolvedKind::Agent(agent, _) = resolved.kind() else { return Ok(()) };
    let channel = Scope::ChannelScope { session_id: resolved.session_id(), channel_id };
    match agent.load().name.scope.admits(&channel) {
        true => Ok(()),
        false => {
            Err(ChannelError::OutOfScope { participant: resolved.actor_id(), channel: channel_id })
        }
    }
}

/// The own context of `resolved` if it is whom `context` acts for, or owned by them - themselves,
/// or their own agent (issue 341); `None` otherwise.
fn own_context(
    resolved: &ResolvedParticipant,
    context: &SecurityContext,
) -> Option<SecurityContext> {
    resolved.context().filter(|own| own.owner_id == context.principal_id)
}

#[cfg(test)]
mod tests {
    use crate::channel::ResolvedParticipant;
    use crate::session::testing::*;
    use crate::session::ChannelSpec;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::Agent;
    use caelumora_domain::channel::testing::main_channel;
    use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, Participant};
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::{Grant, Subject};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::name::Name;
    use caelumora_domain::value::name::ScopedName;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;
    use std::sync::Arc;

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn an_agent_is_cleared_only_as_far_as_its_owner_and_its_personality_are() {
        use crate::channel::ChannelError;
        use caelumora_domain::agent::{Personality, PersonalityKind};
        use caelumora_domain::confidentiality::{
            Clearance, Compartment, Compartments, Confidentiality,
        };
        use caelumora_domain::role::{RoleId, Roles};
        use caelumora_domain::user::User;
        use caelumora_security::confidentiality::ConfidentialityRules;

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
        let kernel = owned_kernel(ConfidentialityRules::new(compartments).with_roles(roles));
        let manager = kernel.session_manager();
        let user = |name: &str, roles: &[&str]| User {
            roles: ids.ids_of(roles).unwrap(),
            ..make_user(name)
        };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        // With her roles, as her login gives them: she reads the channel she adds to.
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default())
            .with_roles(alice.roles.clone(), None);
        // An agent of `owner` whose personality is of `kind` and names `roles`.
        let agent = |name: &'static str,
                     owner: ActorId,
                     kind: PersonalityKind,
                     roles: Option<&'static [&'static str]>| {
            let kernel = Arc::clone(&kernel);
            let ids = ids.clone();
            async move {
                let personality = Personality {
                    kind,
                    roles: roles.map(|roles| ids.ids_of(roles).unwrap()),
                    ..make_personality(name)
                };
                let agent = stored_agent(kernel.entity_stores(), name, personality, owner).await;
                ActorId::Agent(agent.id)
            }
        };
        // Stored first: their roles are read from the store.
        let users = record_users(kernel.entity_stores(), &[&alice, &bob]).await;
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
            let kernel = Arc::clone(&kernel);
            let (alice_ctx, classification) = (alice_ctx.clone(), classification.clone());
            async move {
                // A channel of only `participant` - and its approver, who is
                // cleared for anything.
                let ActorId::Agent(id) = participant else { unreachable!("agents only") };
                let channel = Channel::build(
                    ChannelId::new(),
                    SessionId::new(),
                    alice_id,
                    Name::new("probe").unwrap(),
                    Default::default(),
                    classification,
                    None,
                );
                let named = [Participant::Agent(id)];
                let controllers = kernel.controller_manager();
                let founders = ResolvedParticipant::resolve_existing(controllers, &named, &channel);
                let founders = founders.await.unwrap();
                kernel
                    .participation()
                    .require_admitted(&alice_ctx, Some(&channel), &founders)
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
        let session = create(manager, &alice_ctx, session).await.unwrap();
        let with = |name: &str, participants| ChannelSpec {
            classification: hr.clone(),
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

        let kernel = test_kernel();
        let personalities = Arc::clone(&kernel.entity_stores().personalities);
        let agents = Arc::clone(&kernel.entity_stores().agents);
        let manager = kernel.session_manager();
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
