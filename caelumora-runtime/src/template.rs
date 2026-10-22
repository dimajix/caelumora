//! Finding or making what a session template names (issue 88): the model
//! says what each participant is for a session (`ParticipantTemplate::
//! request`); this resolves the requests against the store, on behalf of
//! whoever creates the session. `SessionManager::create_from_template` is
//! its caller. What is made is made by its owner - an agent by the
//! `AgentManager`, a workspace by the `WorkspaceManager`; finding the one a
//! template names is the template's. Its agents are found or made once the
//! session is, and taken into it as anyone is taken into a running session.

use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, PersonalityId};
use caelumora_model::channel::Participant;
use caelumora_model::value::name::ScopedName;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, EntityStores, StoreError, StoreResultExt};
use caelumora_model::template::{ParticipantRequest, SessionTemplate, TemplateError, TemplateResult, WorkspaceTemplate};
use caelumora_model::user::UserId;
use caelumora_model::workspace::WorkspaceId;
use crate::agent::{AgentError, AgentManager};
use crate::session::{SessionError, SessionLoop};
use crate::workspace::{WorkspaceError, WorkspaceManager};
use anyhow::Result;

/// The participants of a session template as they are found for a session
/// (`resolve_participants`): its users, each with its name in the template,
/// and the agents it names, to be found or made once the session is.
pub(crate) type TemplateParticipants = (Vec<(String, Participant)>, Vec<(String, ScopedName, PersonalityId)>);

/// The users of `template` for the session `session_id` of
/// `owner`, each with its name in the template, the users found in
/// `stores` as `context`; and the agents it names, with their names in the
/// template, which are found or made once the session is (`take_in_participants`).
///
/// # Errors
/// A `TemplateError` if the template cannot be made into a session; a
/// `StoreError` if a user it names does not exist, or the store refuses or
/// fails.
pub(crate) async fn resolve_participants(stores: &EntityStores, context: &SecurityContext, template: &SessionTemplate, session_id: SessionId, owner: UserId) -> Result<TemplateParticipants> {
    let requests = template.participants.iter()
        .map(|p| p.request(session_id, owner).map(|request| (p.name().to_string(), request)))
        .collect::<TemplateResult<Vec<_>>>()?;
    let (mut participants, mut agents) = (Vec::with_capacity(requests.len()), Vec::new());
    for (name, request) in requests {
        match request {
            ParticipantRequest::User(id) => {
                // Only to find out that the user exists: the participant is
                // the reference (issue 120).
                stores.users.get(context, id).await?;
                participants.push((name, Participant::User(id)));
            }
            ParticipantRequest::Agent { name: agent_name, personality_id } => agents.push((name, agent_name, personality_id)),
        }
    }
    Ok((participants, agents))
}

/// Takes the participants `template` names - its `users`, in `session`
/// already, and its `agents`, as `resolve_participants` gives them - into
/// the channels of `session`, made from it, that name them, as their
/// founders (`ChannelLoop::found`, issue 420), on behalf of `context`, who
/// made it. An agent is found in `store` or made by `agent_manager`
/// (`agent`) once the session is, so that one of its scope is made where
/// `context` may put it, and taken into the session first.
///
/// # Errors
/// As `agent`, `SessionLoop::add_participant` and `ChannelLoop::found`;
/// who took part before stays.
pub(crate) async fn take_in_participants(store: &dyn EntityStore<Agent>, agent_manager: &AgentManager, context: &SecurityContext, template: &SessionTemplate, session: &SessionLoop, users: Vec<(String, Participant)>, agents: Vec<(String, ScopedName, PersonalityId)>) -> Result<()> {
    let channels = session.session().channels.clone();
    let mut founders: Vec<(String, ActorId)> = users.into_iter().map(|(name, user)| (name, user.actor_id())).collect();
    for (name, agent_name, personality_id) in agents {
        let participant = ActorId::Agent(agent(store, agent_manager, context, agent_name, personality_id).await?);
        session.add_participant(context, participant, None).await?;
        founders.push((name, participant));
    }
    for (name, participant) in founders {
        for channel in template.channels_of(&name, &channels) {
            let channel = session.channel(channel).ok_or(SessionError::ChannelNotFound { session: session.id(), channel })?;
            channel.found(context, participant).await?;
        }
    }
    Ok(())
}

/// The agent of `name` - in exactly its scope (issue 178) - if it is of
/// `personality_id`, or a new one of it there, made by `agents` and owned
/// by whoever `context` acts for. One of that name in another scope is not
/// it: from the session, the new one shadows it. If another makes it
/// meanwhile, the store refuses the second (issue 250) and the agent is
/// looked up once more.
///
/// # Errors
/// `TemplateError::AgentNameTaken` if the agent of `name` is of another
/// personality; an `AgentError` of making it - `PersonalityOutOfScope` if
/// the personality lies in a session's or a channel's scope the agent would
/// not, `StoreError::NameTaken` if the agent of `name` lies where `context`
/// may not read it; whatever else the store reports.
async fn agent(store: &dyn EntityStore<Agent>, agents: &AgentManager, context: &SecurityContext, name: ScopedName, personality_id: PersonalityId) -> Result<AgentId> {
    if let Some(found) = existing_agent(store, context, &name, personality_id).await? {
        return Ok(found);
    }
    match agents.create(context, name.name.as_str(), name.scope.clone(), personality_id).await {
        Ok(agent) => Ok(agent.id),
        Err(taken @ AgentError::Store(StoreError::NameTaken { .. })) => match existing_agent(store, context, &name, personality_id).await? {
            Some(found) => Ok(found),
            None => Err(taken.into()),
        },
        Err(e) => Err(e.into()),
    }
}

/// The agent of `name` if there is one the caller may read and it is of
/// `personality_id`; `None` if there is none.
///
/// # Errors
/// `TemplateError::AgentNameTaken` if it is of another personality;
/// whatever the store reports. Only "there is none" means "make one": a
/// lookup that failed or was refused is an error, not a reason for a second
/// agent of that name (issue 77).
async fn existing_agent(store: &dyn EntityStore<Agent>, context: &SecurityContext, name: &ScopedName, personality_id: PersonalityId) -> Result<Option<AgentId>> {
    match store.find_by_name(context, name).await.optional()? {
        Some(agent) if agent.personality_id == personality_id => Ok(Some(agent.id)),
        Some(_) => Err(TemplateError::AgentNameTaken(name.name.clone()).into()),
        None => Ok(None),
    }
}

/// The workspace `template` names for the session `session_id` of
/// `owner` (issue 251, decision 13): the one of its name in exactly its
/// scope, if it is in the storage the template names, or a new one there,
/// made as `context` - with whether it was made. As an agent is found or
/// made: if another makes it meanwhile, it is looked up once more.
///
/// # Errors
/// `TemplateError::WorkspaceNameTaken` if the one of its name is in
/// another storage, `UnsupportedScope` for a group scope; a
/// `WorkspaceError` of making it - `NameTaken` if the one of its name lies
/// where `context` may not read it; whatever else the store reports.
pub(crate) async fn resolve_workspace(workspaces: &WorkspaceManager, context: &SecurityContext, template: &WorkspaceTemplate, session_id: SessionId, owner: UserId) -> Result<(WorkspaceId, bool)> {
    let name = template.scoped_name(session_id, owner)?;
    if let Some(found) = existing_workspace(workspaces, context, &name, template).await? {
        return Ok((found, false));
    }
    let description = template.description.as_ref().map(|d| d.as_str());
    match workspaces.create(context, name.name.as_str(), name.scope.clone(), &template.storage, description, template.classification.clone()).await {
        Ok(made) => Ok((made.id, true)),
        Err(taken @ WorkspaceError::Store(StoreError::NameTaken { .. })) => match existing_workspace(workspaces, context, &name, template).await? {
            Some(found) => Ok((found, false)),
            None => Err(taken.into()),
        },
        Err(e) => Err(e.into()),
    }
}

/// The workspace of `name` if there is one the caller may read and it is
/// in the storage `template` names; `None` if there is none.
///
/// # Errors
/// `TemplateError::WorkspaceNameTaken` if it is in another storage;
/// whatever the store reports - never "none" for a lookup that failed.
async fn existing_workspace(workspaces: &WorkspaceManager, context: &SecurityContext, name: &ScopedName, template: &WorkspaceTemplate) -> Result<Option<WorkspaceId>> {
    match workspaces.find_by_name(context, name).await.optional()? {
        Some(workspace) if workspace.storage == template.storage => Ok(Some(workspace.id)),
        Some(_) => Err(TemplateError::WorkspaceNameTaken(name.name.clone()).into()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::store::query::Query;
    use caelumora_model::value::name::{Name, ScopedName};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::agent::Personality;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{ActionType, AllowAllPolicy, SecurityAttributes, AccessPolicy, Verb};
    use caelumora_model::session::Session;
    use caelumora_model::store::Stored;
    use crate::event::EventBus;
    use crate::session::testing::bare_session;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::StoreBackend;
    use caelumora_model::template::{ParticipantTemplate, ScopeTemplate, SessionTemplateId, TemplateError};
    use caelumora_model::user::User;
    use std::sync::{Arc, OnceLock};
    use caelumora_model::confidentiality::ConfidentialityRules;
    use std::sync::atomic::AtomicBool;

    /// Allows everything but listing agents.
    #[derive(Debug)]
    struct NoAgentLookup;

    #[async_trait::async_trait]
    impl AccessPolicy for NoAgentLookup {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            !matches!(action, ActionType::List(Agent::KIND))
        }
    }

    fn template(participants: Vec<ParticipantTemplate>) -> SessionTemplate {
        SessionTemplate {
            access: Vec::new(),
            summary: None, briefing: None, language: None, time_zone: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionTemplateId::new(),
            name: ScopedName::new("pairing", Default::default()).unwrap(),
            description: None,
            participants,
            channels: Vec::new(),
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
            deleted: None,
        }
    }

    fn helper(personality_id: PersonalityId) -> ParticipantTemplate {
        ParticipantTemplate::Agent { name: Name::new("helper").unwrap(), scope: ScopeTemplate::GlobalScope, personality_id }
    }

    async fn store_with_personality(store: StoreBackend) -> (StoreBackend, PersonalityId) {
        let personality = Personality {
            description: None,
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Visibility::Channel,
            hears_during_turn: Default::default(),
            deleted: None,
        };
        store.entity_stores().unwrap().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
        (store, personality.id)
    }

    /// The participants of `template` as `create_from_template` finds them
    /// in `store` or makes them there: its users and approvers, then - once
    /// the session is stored, owned by `owner` - its agents (`agent`).
    async fn resolve(store: &StoreBackend, context: &SecurityContext, template: &SessionTemplate, session_id: SessionId, owner: UserId) -> Result<Vec<(String, Participant)>> {
        let access_policy: Arc<dyn AccessPolicy> = Arc::new(AllowAllPolicy::new());
        let events = Arc::new(EventBus::new(Arc::clone(&access_policy), Arc::new(ConfidentialityRules::default())));
        let agents = AgentManager::new(&store.entity_stores().unwrap(), store.state_store(), events, access_policy);
        let (mut participants, named) = resolve_participants(&store.entity_stores().unwrap(), context, template, session_id, owner).await?;
        let (system, sessions) = (SecurityContext::system(), store.entity_stores().unwrap().sessions);
        if sessions.get(&system, session_id).await.is_err() {
            let session = Session { security_attributes: SecurityAttributes::owned_by(ActorId::User(owner)), ..bare_session(session_id) };
            sessions.create(&system, &session).await.unwrap();
        }
        for (name, agent_name, personality_id) in named {
            let made = agent(store.entity_stores().unwrap().agents.as_ref(), &agents, context, agent_name, personality_id).await?;
            participants.push((name, Participant::Agent(made)));
        }
        Ok(participants)
    }

    fn as_user(owner: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(owner), Default::default(), Default::default())
    }

    #[tokio::test]
    async fn an_agent_is_created_only_when_the_lookup_finds_none() {
        let owner = UserId::new();
        let (store, personality) = store_with_personality(StoreBackend::in_memory()).await;
        let template = template(vec![helper(personality)]);

        // None there yet: one is made, owned by the creator, and found the next time.
        let first = resolve(&store, &as_user(owner), &template, SessionId::new(), owner).await.expect("created");
        let again = resolve(&store, &as_user(owner), &template, SessionId::new(), owner).await.expect("found");
        let (Participant::Agent(first), Participant::Agent(again)) = (&first[0].1, &again[0].1) else { panic!("agents") };
        assert_eq!(first, again, "the same agent, not a second one");
        let agent = store.entity_stores().unwrap().agents.get(&SecurityContext::system(), *first).await.unwrap();
        assert_eq!(agent.personality_id, personality);
        assert_eq!(agent.security_attributes.owner_id, ActorId::User(owner));

        // A lookup that fails is an error, not a reason to make another.
        let (store, personality) = store_with_personality(StoreBackend::new(Arc::new(MemoryStore::new()), Arc::new(NoAgentLookup), Arc::new(ConfidentialityRules::default()))).await;
        let refused = resolve(&store, &as_user(owner), &self::template(vec![helper(personality)]), SessionId::new(), owner).await
            .expect_err("the lookup is refused");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_))), "{refused}");
    }

    #[tokio::test]
    async fn an_agent_is_reused_only_in_the_scope_asked_for_and_of_its_personality() {
        let owner = UserId::new();
        let (store, personality) = store_with_personality(StoreBackend::in_memory()).await;
        let system = SecurityContext::system();
        let plain = store.entity_stores().unwrap().personalities.get(&system, personality).await.unwrap();
        let other = Personality { id: PersonalityId::new(), name: ScopedName::new("other", Default::default()).unwrap(), ..plain.clone() };
        store.entity_stores().unwrap().personalities.create(&system, &other).await.unwrap();
        let own = Agent::new(ScopedName::new("helper", Scope::UserScope { user_id: owner }).unwrap(), plain.id, ActorId::User(owner));
        store.entity_stores().unwrap().agents.create(&system, &own).await.unwrap();
        let in_scope = |scope| template(vec![ParticipantTemplate::Agent { name: Name::new("Helper").unwrap(), scope, personality_id: personality }]);
        let made = |participants: Vec<(String, Participant)>| match participants[0].1 { Participant::Agent(id) => id, _ => panic!("an agent") };

        let reused = resolve(&store, &as_user(owner), &in_scope(ScopeTemplate::UserScope), SessionId::new(), owner).await.unwrap();
        assert_eq!(made(reused), own.id, "the user's own, with its memory");

        let session_id = SessionId::new();
        let shadowing = made(resolve(&store, &as_user(owner), &in_scope(ScopeTemplate::SessionScope), session_id, owner).await.unwrap());
        assert_ne!(shadowing, own.id, "the user's lies in another scope: a new one shadows it");
        assert_eq!(store.entity_stores().unwrap().agents.get(&system, shadowing).await.unwrap().name.scope, Scope::SessionScope { session_id });

        let global = made(resolve(&store, &as_user(owner), &in_scope(ScopeTemplate::GlobalScope), SessionId::new(), owner).await.unwrap());
        let again = made(resolve(&store, &as_user(owner), &in_scope(ScopeTemplate::GlobalScope), SessionId::new(), owner).await.unwrap());
        assert_eq!(global, again, "one global agent, although the user's own lies further in");
        assert_ne!(global, own.id);

        let taken = ParticipantTemplate::Agent { name: Name::new("helper").unwrap(), scope: ScopeTemplate::GlobalScope, personality_id: other.id };
        let refused = resolve(&store, &as_user(owner), &template(vec![taken]), SessionId::new(), owner).await
            .expect_err("the global one is of another personality");
        assert!(matches!(refused.downcast_ref::<TemplateError>(), Some(TemplateError::AgentNameTaken(name)) if *name == "helper"), "{refused}");
        assert_eq!(store.entity_stores().unwrap().agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), 3, "and none is made: the user's, the session's, the global one");
    }

    /// Allows everything, and has another make the agent `helper` between a
    /// template's lookup and its making it, as the first to ask to make one
    /// (issue 250).
    #[derive(Debug, Default)]
    struct MadeMeanwhile {
        /// What the stores keep, written to past the policy, which is this.
        store: OnceLock<Arc<MemoryStore>>,
        personality: OnceLock<Personality>,
        made: AtomicBool,
    }

    #[async_trait::async_trait]
    impl AccessPolicy for MadeMeanwhile {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            use std::sync::atomic::Ordering;
            if matches!(action, ActionType::Entity { kind: Agent::KIND, verb: Verb::Create, .. }) && !self.made.swap(true, Ordering::SeqCst) {
                let (Some(store), Some(personality)) = (self.store.get(), self.personality.get()) else { return true };
                let other = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
                let store = StoreBackend::new(Arc::<MemoryStore>::clone(store), Arc::new(AllowAllPolicy::new()), Arc::new(ConfidentialityRules::default()));
                store.entity_stores().unwrap().agents.create(&SecurityContext::system(), &other).await.unwrap();
            }
            true
        }
    }

    #[tokio::test]
    async fn an_agent_made_meanwhile_is_looked_up_again() {
        let owner = UserId::new();
        let manager = Arc::new(MadeMeanwhile::default());
        let memory = Arc::new(MemoryStore::new());
        manager.store.set(Arc::clone(&memory)).unwrap();
        let (store, personality) = store_with_personality(StoreBackend::new(memory, Arc::<MadeMeanwhile>::clone(&manager), Arc::new(ConfidentialityRules::default()))).await;
        let system = SecurityContext::system();
        manager.personality.set(store.entity_stores().unwrap().personalities.get(&system, personality).await.unwrap()).unwrap();

        let participants = resolve(&store, &as_user(owner), &template(vec![helper(personality)]), SessionId::new(), owner).await
            .expect("the one made meanwhile");
        let agents = store.entity_stores().unwrap().agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap();
        assert_eq!(agents.len(), 1, "no second one");
        assert!(matches!(participants[0].1, Participant::Agent(id) if id == agents[0].id && agents[0].security_attributes.owner_id == ActorId::None), "the other's");
    }

    #[tokio::test]
    async fn an_agent_the_caller_may_not_read_keeps_its_name() {
        use caelumora_model::security::OwnershipPolicy;
        let (owner, other) = (UserId::new(), UserId::new());
        let (store, personality) = store_with_personality(StoreBackend::new(Arc::new(MemoryStore::new()), Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()))).await;
        let system = SecurityContext::system();
        let plain = store.entity_stores().unwrap().personalities.get(&system, personality).await.unwrap();
        store.entity_stores().unwrap().agents.create(&system, &Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), plain.id, ActorId::User(other))).await.unwrap();

        let refused = resolve(&store, &as_user(owner), &template(vec![helper(personality)]), SessionId::new(), owner).await
            .expect_err("the name is another's");
        assert!(matches!(refused.downcast_ref::<AgentError>(), Some(AgentError::Store(StoreError::NameTaken { entity: "agent" }))), "{refused}");
        assert_eq!(store.entity_stores().unwrap().agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), 1, "and no second one is made");
    }

    #[tokio::test]
    async fn a_personality_of_a_session_makes_agents_only_within_it() {
        let owner = UserId::new();
        let (store, personality) = store_with_personality(StoreBackend::in_memory()).await;
        let system = SecurityContext::system();
        let plain = store.entity_stores().unwrap().personalities.get(&system, personality).await.unwrap();
        let session_id = SessionId::new();
        let sessions = Personality { id: PersonalityId::new(), name: ScopedName::new("planner", Scope::SessionScope { session_id }).unwrap(), ..plain };
        store.entity_stores().unwrap().personalities.create(&system, &sessions).await.unwrap();
        let in_scope = |scope| template(vec![ParticipantTemplate::Agent { name: Name::new("planner").unwrap(), scope, personality_id: sessions.id }]);

        resolve(&store, &as_user(owner), &in_scope(ScopeTemplate::SessionScope), session_id, owner).await
            .expect("an agent of its session");
        for (scope, session) in [(ScopeTemplate::SessionScope, SessionId::new()), (ScopeTemplate::UserScope, session_id), (ScopeTemplate::GlobalScope, session_id)] {
            let refused = resolve(&store, &as_user(owner), &in_scope(scope.clone()), session, owner).await
                .expect_err("not one outside it");
            assert!(matches!(refused.downcast_ref::<AgentError>(), Some(AgentError::PersonalityOutOfScope(name)) if *name == "planner"), "{scope:?}: {refused}");
        }
        assert_eq!(store.entity_stores().unwrap().agents.list(&system, &Query::all()).await.map(|page| page.items).unwrap().len(), 1, "none is made outside it");
    }

    #[tokio::test]
    async fn the_owner_must_exist() {
        let owner = UserId::new();
        let store = StoreBackend::in_memory();
        let template = template(vec![ParticipantTemplate::User { name: "user".to_string() }]);

        let missing = resolve(&store, &as_user(owner), &template, SessionId::new(), owner).await.expect_err("no such user");
        assert!(matches!(missing.downcast_ref::<StoreError>(), Some(StoreError::NotFound { entity: "user", .. })), "{missing}");

        store.entity_stores().unwrap().users.create(&SecurityContext::system(), &User { id: owner, username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None }).await.unwrap();
        let participants = resolve(&store, &as_user(owner), &template, SessionId::new(), owner).await.unwrap();
        assert!(matches!(participants[0], (ref name, Participant::User(id)) if name == "user" && id == owner));

        let unbound = self::template(vec![ParticipantTemplate::User { name: "guest".to_string() }]);
        let error = resolve(&store, &as_user(owner), &unbound, SessionId::new(), owner).await.expect_err("nobody fills it");
        assert!(matches!(error.downcast_ref::<TemplateError>(), Some(TemplateError::UnboundUser(_))), "{error}");
    }
}
