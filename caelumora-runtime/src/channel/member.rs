//! Who takes part in a channel: a participant with what its actor is built from, and whether it
//! may take part.

use crate::runtime::RuntimeManager;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, ChannelId, Participant};
use crate::controller::{AgentController, ControllerManager, UserController};
use caelumora_model::agent::{AgentId, Personality, PersonalityKind};
use caelumora_model::approval::Approver;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::role::RoleId;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use caelumora_model::scope::Scope;
use caelumora_model::session::SessionId;
use caelumora_model::store::{Entities, StoreError, StoreResult, StoreResultExt};
use std::sync::Arc;
use super::ChannelError;
use caelumora_model::removal::Stored;

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
    /// `None` if its user or agent no longer exists - an agent as
    /// `agent_exists` says. An agent runs only with
    /// a personality it may read, in the channel, as `security` decides
    /// (`GetPersonality`): one whose owner lost it - a share withdrawn - is
    /// not run (issue 191).
    ///
    /// # Errors
    /// Whatever else the store reports when loading it - also that an
    /// agent's personality is gone - and `StoreError::PermissionDenied` if
    /// it is not the agent's to read: the agent exists, but cannot be run.
    pub(super) async fn of(participant: &Participant, channel: &Channel, entities: &dyn Entities, controllers: &ControllerManager, security: &dyn SecurityManager) -> Result<Option<Self>> {
        let agent = match participant {
            Participant::User(id) => return Ok(controllers.user_controller(*id).await.optional()?.map(Member::User)),
            Participant::Approver(approver) => return Ok(Some(Member::Approver(approver.clone()))),
            Participant::Agent(id) => {
                if !agent_exists(entities, *id).await? {
                    return Ok(None);
                }
                match controllers.agent_controller(*id).await.optional()? {
                    Some(agent) => agent,
                    None => return Ok(None),
                }
            }
        };
        let personality = agent.personality().await?;
        let context = agent.security_context(channel.session_id, channel.id, personality.limiting_roles());
        security.require(&context, ActionType::get(Personality::KIND, &personality.security_attributes), || StoreError::PermissionDenied).await?;
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

/// Whether agent `id` exists - neither deleted nor purged -, asked of
/// `entities` rather than of the controllers: the instance keeps the
/// controller of an agent while an actor of it runs, also once it was
/// deleted (issue 314).
///
/// # Errors
/// Whatever the store reports but that there is none.
pub(super) async fn agent_exists(entities: &dyn Entities, id: AgentId) -> StoreResult<bool> {
    Ok(entities.get_agent(&SecurityContext::system(), id).await.optional()?.is_some())
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
        if let Some(member) = Member::of(participant, channel, runtime.entity_store().as_ref(), runtime.controller_manager(), runtime.security_manager().as_ref()).await? {
            member.require_in_scope(channel.session_id, channel.id)?;
            member.require_cleared(runtime.confidentiality().as_ref(), context, &channel.classification, channel.session_id, channel.id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use caelumora_model::value::name::Name;

    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::Agent;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::scope::Scope;
    use caelumora_model::session::{ChannelId, SessionId};
    use std::sync::Arc;
    use caelumora_model::value::name::ScopedName;

    
    use crate::session::{ChannelSpec, SessionManager};
    use crate::session::testing::*;

    #[tokio::test]
    async fn an_agent_is_cleared_only_as_far_as_its_owner_and_its_personality_are() {
        use crate::channel::ChannelError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::confidentiality::{Clearance, Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::role::{RoleId, Roles};
        use caelumora_model::value::name::ScopedName;
        use caelumora_model::scope::Scope;
        use caelumora_model::store::Entities;
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let roles = Roles::new([
            (RoleId::new(), "staff".to_string(), vec![Clearance { level: Confidentiality::Internal, compartment: None }]),
            (RoleId::new(), "hr".to_string(), vec![Clearance { level: Confidentiality::Confidential, compartment: Some(Compartment::new("hr")) }]),
        ]).unwrap();
        let ids = roles.clone();
        let (store, manager) = owned_runtime(ConfidentialityRules::new(compartments).with_roles(roles));
        let runtime = Arc::clone(runtime_of(&manager));
        let system = SecurityContext::system();
        let user = |name: &str, roles: &[&str]| User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: ids.ids_of(roles).unwrap(), deleted: None };
        let (alice, bob) = (user("alice", &["staff", "hr"]), user("bob", &["staff"]));
        let (alice_id, bob_id) = (ActorId::User(alice.id), ActorId::User(bob.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        // An agent of `owner` whose personality is of `kind` and names `roles`.
        let agent = |name: &'static str, owner: ActorId, kind: PersonalityKind, roles: Option<&'static [&'static str]>| {
            let store = Arc::clone(&store);
            let system = system.clone();
            let ids = ids.clone();
            async move {
                let personality = Personality {
                    id: PersonalityId::new(), name: ScopedName::new(name, Default::default()).unwrap(), kind,
                    roles: roles.map(|roles| ids.ids_of(roles).unwrap()),
                    description: None, introduction: None, components: Default::default(),
                    security_attributes: Default::default(), reasoning_visibility: Default::default(),
                    deleted: None,
                };
                store.create_personality(&system, &personality).await.unwrap();
                let agent = Agent::new(ScopedName::new(name, Scope::GlobalScope).unwrap(), &personality, owner);
                store.create_agent(&system, &agent).await.unwrap();
                ActorId::Agent(agent.id)
            }
        };
        // Stored first: their roles are read from the store.
        let users = record_users(&manager, &[&alice, &bob]).await;
        let hers = agent("hers", alice_id, PersonalityKind::Internal, None).await;
        let limited = agent("limited", alice_id, PersonalityKind::Internal, Some(&["staff"])).await;
        let bobs = agent("bobs", bob_id, PersonalityKind::Internal, None).await;
        let remote = agent("remote", alice_id, PersonalityKind::External, None).await;
        let cleared_remote = agent("cleared", alice_id, PersonalityKind::External, Some(&["staff"])).await;

        let hr = Classification::new(Confidentiality::Confidential, [Compartment::new("hr")]).unwrap();
        let internal = Classification::general(Confidentiality::Internal);
        let cleared = |participant: ActorId, classification: &Classification| {
            let runtime = Arc::clone(&runtime);
            let (alice_ctx, classification) = (alice_ctx.clone(), classification.clone());
            async move {
                // A channel of only `participant` - and its approver, who is
                // cleared for anything.
                let ActorId::Agent(id) = participant else { unreachable!("agents only") };
                let channel = caelumora_model::channel::Channel::build(SessionId::new(), alice_id, caelumora_model::value::name::Name::new("probe").unwrap(),
                    Default::default(), classification, None, vec![caelumora_model::channel::Participant::Agent(id)]);
                crate::channel::require_admitted(&runtime, &alice_ctx, &channel).await.is_ok()
            }
        };
        assert!(cleared(hers, &hr).await, "an agent that names no roles is cleared as its owner");
        assert!(!cleared(limited, &hr).await, "its personality limits it below its owner");
        assert!(cleared(limited, &internal).await, "within its limit");
        assert!(!cleared(bobs, &hr).await, "its owner is not cleared");
        assert!(!cleared(remote, &internal).await, "an external agent that names no roles sees only what is public");
        assert!(cleared(remote, &Classification::PUBLIC).await);
        assert!(cleared(cleared_remote, &internal).await, "one that names its roles, within them");
        assert!(!cleared(cleared_remote, &hr).await);

        // The same question when a channel is created and when one is joined.
        let mut session = make_session("hers", alice_id);
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(bob_id), caelumora_model::security::Grant::Read);
        session.participants = users;
        session.participants.extend([hers, limited].map(|a| match a { ActorId::Agent(id) => caelumora_model::channel::Participant::Agent(id), _ => unreachable!() }));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let with = |name: &str, participants| ChannelSpec { classification: Some(hr.clone()), ..spec(name, participants) };
        let refused = session.create_channel(&alice_ctx, with("hr", vec![limited])).await.expect_err("limited below HR");
        assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { participant, .. }) if *participant == limited), "{refused}");
        let channel = session.create_channel(&alice_ctx, with("hr", vec![hers])).await.expect("cleared as alice");
        let joining = channel.add_participant(&alice_ctx, limited, caelumora_model::channel::HistoryAccess::All).await.expect_err("nor may it join");
        assert!(matches!(joining.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })), "{joining}");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_agent_takes_part_only_within_its_sessions_or_channels_scope() {
        use crate::channel::ChannelError;
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::{HistoryAccess, Participant};

        let runtime = test_runtime();
        let store = Arc::clone(runtime.entity_store());
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Scope::GlobalScope).unwrap(), kind: Default::default(),
            roles: None, description: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let mut session = make_session("scoped", ActorId::None);
        let id = session.id;
        let elsewhere = [
            Agent::new(ScopedName::new("aside", Scope::ChannelScope { session_id: id, channel_id: ChannelId::new() }).unwrap(), &personality, ActorId::None),
            Agent::new(ScopedName::new("visitor", Scope::SessionScope { session_id: SessionId::new() }).unwrap(), &personality, ActorId::None),
        ];
        for agent in &elsewhere {
            store.create_agent(&system, agent).await.unwrap();
            session.participants.push(Participant::Agent(agent.id));
        }
        // In the session, not in its channel: adding them there is refused below.
        let main = caelumora_model::channel::Channel { participants: Vec::new(), ..main_channel(&session) };
        let running = manager.create_session(&ctx, session, vec![main]).await.unwrap();
        let main = running.channel_by_name("main").unwrap();

        for agent in &elsewhere {
            let actor = ActorId::Agent(agent.id);
            let refused = running.create_channel(&ctx, spec("with-it", vec![actor])).await.expect_err("not in a new channel");
            assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::OutOfScope { participant, .. }) if *participant == actor), "{refused}");
            let refused = main.add_participant(&ctx, actor, HistoryAccess::All).await.expect_err("nor in one there is");
            assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::OutOfScope { participant, .. }) if *participant == actor), "{refused}");
        }
        assert!(running.channel_by_name("with-it").is_none(), "nothing was created");
        assert!(main.channel().participants.is_empty(), "nor added");
        manager.stop_session(&system, id).await.unwrap();
    }
}
