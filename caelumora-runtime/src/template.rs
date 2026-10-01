//! Finding or making what a session template names (issue 88): the model
//! says what each participant is for a session (`ParticipantTemplate::
//! request`); this resolves the requests against the store, on behalf of
//! whoever creates the session. `SessionManager::create_from_template` is
//! its caller.

use caelumora_model::agent::{Agent, AgentId, PersonalityId};
use caelumora_model::channel::Participant;
use caelumora_model::name::ScopedName;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::store::{EntityStore, StoreResult};
use caelumora_model::template::{ParticipantRequest, SessionTemplate, TemplateResult};
use caelumora_model::user::UserId;

/// The participants of `template` for the session `session_id` of `owner`,
/// each with its name in the template, found or made as `context`.
///
/// # Errors
/// A `TemplateError` if the template cannot be made into a session; a
/// `StoreError` if a user or personality it names does not exist, or the
/// store refuses or fails - never a second agent because a lookup failed.
pub(crate) async fn resolve_participants(store: &dyn EntityStore, context: &SecurityContext, template: &SessionTemplate, session_id: SessionId, owner: UserId) -> anyhow::Result<Vec<(String, Participant)>> {
    let requests = template.participants.iter()
        .map(|p| p.request(session_id, owner).map(|request| (p.name().to_string(), request)))
        .collect::<TemplateResult<Vec<_>>>()?;
    let mut participants = Vec::with_capacity(requests.len());
    for (name, request) in requests {
        let participant = match request {
            ParticipantRequest::User(id) => {
                // Only to find out that the user exists: the participant is
                // the reference (issue 120).
                store.get_user(context, id).await?;
                Participant::User(id)
            }
            ParticipantRequest::Agent { name: agent_name, personality_id } => {
                Participant::Agent(agent(store, context, agent_name, personality_id).await?)
            }
            ParticipantRequest::Approver(approver) => Participant::Approver(approver),
        };
        participants.push((name, participant));
    }
    Ok(participants)
}

/// The agent of `name`, or a new one of `personality_id`, owned by whoever
/// `context` acts for.
async fn agent(store: &dyn EntityStore, context: &SecurityContext, name: ScopedName, personality_id: PersonalityId) -> StoreResult<AgentId> {
    // Only "there is none" means "make one": a lookup that failed or was
    // refused is an error, not a reason for a second agent of that name
    // (issue 77).
    let known = store.list_agents(context).await?.into_iter()
        .find(|a| a.name.name == name.name && name.scope.contains(&a.name.scope));
    if let Some(agent) = known {
        return Ok(agent.id);
    }
    let personality = store.get_personality(context, personality_id).await?;
    let agent = Agent::new(name, &personality, context.initiator_id);
    store.create_agent(context, &agent).await?;
    Ok(agent.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::{Name, ScopedName};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::Personality;
    use caelumora_model::security::{ActionType, SecurityManager};
    use caelumora_model::store::StoreError;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::template::{ParticipantTemplate, ScopeTemplate, SessionTemplateId, TemplateError};
    use caelumora_model::user::User;
    use std::sync::Arc;

    /// Allows everything but listing agents.
    #[derive(Debug)]
    struct NoAgentLookup;

    #[async_trait::async_trait]
    impl SecurityManager for NoAgentLookup {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            !matches!(action, ActionType::ListAgents)
        }
    }

    fn template(participants: Vec<ParticipantTemplate>) -> SessionTemplate {
        SessionTemplate {
            id: SessionTemplateId::new(),
            name: ScopedName::new("pairing", Default::default()).unwrap(),
            description: String::new(),
            participants,
            channels: Vec::new(),
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
        }
    }

    fn helper(personality_id: PersonalityId) -> ParticipantTemplate {
        ParticipantTemplate::Agent { name: Name::new("helper").unwrap(), scope: ScopeTemplate::GlobalScope, personality_id }
    }

    async fn store_with_personality(store: MemoryStore) -> (Arc<MemoryStore>, PersonalityId) {
        let personality = Personality {
            description: String::new(),
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Visibility::Channel,
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        (Arc::new(store), personality.id)
    }

    fn as_user(owner: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(owner), Default::default(), Default::default())
    }

    #[tokio::test]
    async fn an_agent_is_created_only_when_the_lookup_finds_none() {
        let owner = UserId::new();
        let (store, personality) = store_with_personality(MemoryStore::new()).await;
        let template = template(vec![helper(personality)]);

        // None there yet: one is made, owned by the creator, and found the next time.
        let first = resolve_participants(store.as_ref(), &as_user(owner), &template, SessionId::new(), owner).await.expect("created");
        let again = resolve_participants(store.as_ref(), &as_user(owner), &template, SessionId::new(), owner).await.expect("found");
        let (Participant::Agent(first), Participant::Agent(again)) = (&first[0].1, &again[0].1) else { panic!("agents") };
        assert_eq!(first, again, "the same agent, not a second one");
        let agent = store.get_agent(&SecurityContext::system(), *first).await.unwrap();
        assert_eq!(agent.personality_id, personality);
        assert_eq!(agent.security_attributes.owner_id, ActorId::User(owner));

        // A lookup that fails is an error, not a reason to make another.
        let (store, personality) = store_with_personality(MemoryStore::with_security_manager(Arc::new(NoAgentLookup))).await;
        let refused = resolve_participants(store.as_ref(), &as_user(owner), &self::template(vec![helper(personality)]), SessionId::new(), owner).await
            .expect_err("the lookup is refused");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied)), "{refused}");
    }

    #[tokio::test]
    async fn the_owner_must_exist_and_the_approver_needs_no_store() {
        let owner = UserId::new();
        let store = MemoryStore::new();
        let template = template(vec![
            ParticipantTemplate::User { name: "user".to_string() },
            ParticipantTemplate::Approver { name: "approval".to_string(), strategy: caelumora_model::approval::ApprovalStrategy::Any },
        ]);

        let missing = resolve_participants(&store, &as_user(owner), &template, SessionId::new(), owner).await.expect_err("no such user");
        assert!(matches!(missing.downcast_ref::<StoreError>(), Some(StoreError::NotFound { entity: "user", .. })), "{missing}");

        store.create_user(&SecurityContext::system(), &User { id: owner, username: "alice".to_string(), email: String::new(), roles: Vec::new() }).await.unwrap();
        let participants = resolve_participants(&store, &as_user(owner), &template, SessionId::new(), owner).await.unwrap();
        assert!(matches!(participants[0], (ref name, Participant::User(id)) if name == "user" && id == owner));
        assert!(matches!(participants[1], (ref name, Participant::Approver(_)) if name == "approval"));

        let unbound = self::template(vec![ParticipantTemplate::User { name: "guest".to_string() }]);
        let error = resolve_participants(&store, &as_user(owner), &unbound, SessionId::new(), owner).await.expect_err("nobody fills it");
        assert!(matches!(error.downcast_ref::<TemplateError>(), Some(TemplateError::UnboundUser(_))), "{error}");
    }
}
