use async_trait::async_trait;
use rand::seq::IndexedRandom;
use std::any::Any;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;

use crate::actor::Actor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::user::UserActor;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::message::{Message, MessageRole};
use dashmap::DashMap;
use rand::rngs::StdRng;

#[async_trait]
pub trait Moderator : Send + Sync + Debug {
    async fn route(&mut self, event: &Envelope) -> RouteDecision;
    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> ();
    fn remove_actor(&mut self, actor: Arc<dyn Actor>) -> ();
}

pub struct RouteDecision {
    pub forwards: Vec<(ActorId, Envelope)>,
    /// Whether this event should also be broadcast to external subscribers (e.g. the UI).
    /// Only approval events set this explicitly, since they're the only ones relayed through
    /// multiple internal hops (Agent -> Approver -> User) where the intermediate hop must stay
    /// internal; everything else defaults to `true`.
    pub broadcast: bool,
}

impl From<Vec<(ActorId, Envelope)>> for RouteDecision {
    fn from(forwards: Vec<(ActorId, Envelope)>) -> Self {
        Self { forwards, broadcast: true }
    }
}


#[derive(Debug)]
struct ModeratorActors {
    agent_actors: DashMap<ActorId, Arc<dyn Actor>>,
    user_actors: DashMap<ActorId, Arc<dyn Actor>>,
    approval_actors: DashMap<ActorId, Arc<dyn Actor>>,
}

impl ModeratorActors {
    fn new() -> Self {
        Self {
            agent_actors: DashMap::new(),
            user_actors: DashMap::new(),
            approval_actors: DashMap::new(),
        }
    }

    fn add_actor(&mut self, actor: Arc<dyn Actor>) {
        let any_actor:&dyn Any = actor.as_ref();
        if any_actor.downcast_ref::<UserActor>().is_some() {
            self.user_actors.insert(actor.id(), actor);
        }
        else if any_actor.downcast_ref::<ApproverActor>().is_some() {
            self.approval_actors.insert(actor.id(), actor);
        }
        else {
            self.agent_actors.insert(actor.id(), actor);
        }
    }

    fn remove_actor(&mut self, actor: Arc<dyn Actor>) {
        self.agent_actors.remove(&actor.id());
        self.user_actors.remove(&actor.id());
        self.approval_actors.remove(&actor.id());
    }

    fn forward_routes(&self, event: &Envelope, messages: &Vec<Message>) -> Vec<(ActorId, Envelope)> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let user_messages = replace_role(messages, MessageRole::User);
        let user_messages = remove_tools(&user_messages);

        if user_messages.is_empty() {
            return Vec::new();
        }

        let forward_event = event.forward_with_response(user_messages);
        self.agent_actors.iter()
            .chain(self.user_actors.iter())
            .filter(|r| !sender_ids.contains(r.key()))
            .map(|r| (*r.key(), forward_event.clone()))
            .collect()
    }

    fn actor_candidates(&self, messages: &[Message]) -> Vec<ActorId> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        self.agent_actors.iter()
            .filter(|r| r.value().state() != ActorState::Sleeping)
            .map(|r| *r.key())
            .filter(|id| !sender_ids.contains(id))
            .collect()
    }

    fn route_approval_events(&self, event: &Envelope) -> RouteDecision {
        match &event.event {
            ActorEvent::ApprovalRequested(_) => {
                if matches!(event.context.sender_id, ActorId::Agent(_)) {
                    // Agent -> ApproverActor: internal plumbing, not shown to users yet.
                    let forwards = self.approval_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect();
                    RouteDecision { forwards, broadcast: false }
                } else if matches!(event.context.sender_id, ActorId::Approver(_)) {
                    // ApproverActor -> Users: the request users actually act on.
                    let forwards = self.user_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect();
                    RouteDecision { forwards, broadcast: true }
                } else {
                    RouteDecision { forwards: Vec::new(), broadcast: false }
                }
            }
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) => {
                if matches!(event.context.sender_id, ActorId::User(_)) {
                    // User -> ApproverActor: lets other observers see the decision too.
                    let forwards = self.approval_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect();
                    RouteDecision { forwards, broadcast: true }
                } else if matches!(event.context.sender_id, ActorId::Approver(_)) {
                    // ApproverActor -> Agent: internal plumbing informing the agent.
                    let agent_id = ActorId::Agent(response.agent_id);
                    let forwards = if self.agent_actors.contains_key(&agent_id) {
                        vec![(agent_id, event.clone())]
                    } else {
                        Vec::new()
                    };
                    RouteDecision { forwards, broadcast: false }
                } else {
                    RouteDecision { forwards: Vec::new(), broadcast: false }
                }
            }
            ActorEvent::ApprovalTimedOut(response) => {
                if matches!(event.context.sender_id, ActorId::Approver(_)) {
                    // ApproverActor -> Agent: internal plumbing informing the agent.
                    let agent_id = ActorId::Agent(response.agent_id);
                    let forwards = if self.agent_actors.contains_key(&agent_id) {
                        vec![(agent_id, event.clone())]
                    } else {
                        Vec::new()
                    };
                    RouteDecision { forwards, broadcast: false }
                } else {
                    RouteDecision { forwards: Vec::new(), broadcast: false }
                }
            }
            _ => RouteDecision { forwards: Vec::new(), broadcast: false }
        }
    }
}


#[derive(Debug)]
pub struct BroadcastModerator {
    actors: ModeratorActors,
}

impl BroadcastModerator {
    pub fn new() -> Self {
        Self {
            actors: ModeratorActors::new(),
        }
    }

    async fn handle_full_response(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            let mut routes = self.actors.forward_routes(event, messages);

            if !routes.is_empty() {
                // Pick random actor for response
                let possible_actors = self.actors.actor_candidates(messages);
                for id in possible_actors {
                    let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                    routes.push((id, request_response_event));
                }
            }

            routes
        }
        else {
            Vec::new()
        }
    }
}
#[async_trait]
impl Moderator for BroadcastModerator {
    async fn route(&mut self, event: &Envelope) -> RouteDecision {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await.into()
            }
            ActorEvent::ApprovalRequested(_) |
            ActorEvent::ApprovalGranted(_) |
            ActorEvent::ApprovalDenied(_) |
            ActorEvent::ApprovalTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new().into()
            }
        }
    }

    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: Arc<dyn Actor>) -> () {
        self.actors.remove_actor(agent);
    }
}



#[derive(Debug)]
pub struct RandomModerator {
    actors: ModeratorActors,
    rng: StdRng
}

impl RandomModerator {
    pub fn new() -> Self {
        Self {
            actors: ModeratorActors::new(),
            rng: rand::make_rng()
        }
    }

    async fn handle_full_response(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            let mut routes = self.actors.forward_routes(event, messages);

            if !routes.is_empty() {
                // Pick random actor for response
                let possible_actors = self.actors.actor_candidates(messages);
                if let Some(id) = possible_actors.choose(&mut self.rng) {
                    let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                    routes.push((*id, request_response_event));
                }
            }

            routes
        }
        else {
            Vec::new()
        }
    }
}
#[async_trait]
impl Moderator for RandomModerator {
    async fn route(&mut self, event: &Envelope) -> RouteDecision {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await.into()
            }
            ActorEvent::ApprovalRequested(_) |
            ActorEvent::ApprovalGranted(_) |
            ActorEvent::ApprovalDenied(_) |
            ActorEvent::ApprovalTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new().into()
            }
        }
    }

    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: Arc<dyn Actor>) -> () {
        self.actors.remove_actor(agent);
    }
}


#[derive(Debug)]
pub struct RoundRobinModerator {
    actors: ModeratorActors,
    next_agent_index: usize,
}

impl RoundRobinModerator {
    pub fn new() -> Self {
        Self {
            actors: ModeratorActors::new(),
            next_agent_index: 0,
        }
    }

    async fn handle_full_response(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            let mut routes = self.actors.forward_routes(event, messages);

            if !routes.is_empty() {
                // Pick the next actor for response in round-robin order
                let mut possible_actors = self.actors.actor_candidates(messages);
                possible_actors.sort_unstable_by_key(|id| id.id());

                if !possible_actors.is_empty() {
                    let index = self.next_agent_index % possible_actors.len();
                    let id = possible_actors[index];
                    self.next_agent_index = (index + 1) % possible_actors.len();

                    let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                    routes.push((id, request_response_event));
                } else {
                    self.next_agent_index = 0;
                }
            }

            routes
        }
        else {
            Vec::new()
        }
    }
}

#[async_trait]
impl Moderator for RoundRobinModerator {
    async fn route(&mut self, event: &Envelope) -> RouteDecision {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await.into()
            }
            ActorEvent::ApprovalRequested(_) |
            ActorEvent::ApprovalGranted(_) |
            ActorEvent::ApprovalDenied(_) |
            ActorEvent::ApprovalTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new().into()
            }
        }
    }

    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: Arc<dyn Actor>) -> () {
        self.actors.remove_actor(agent);
        if self.actors.agent_actors.is_empty() {
            self.next_agent_index = 0;
        }
        else {
            self.next_agent_index %= self.actors.agent_actors.len();
        }
    }
}


fn remove_tools(messages:&[Message]) -> Vec<Message> {
    let mut result = Vec::new();
    for m in messages {
        if m.content.is_some() && m.role != MessageRole::Tool {
            let mut m2 = m.clone();
            m2.tool_calls = Vec::new();
            m2.tool_results = Vec::new();
            result.push(m2);
        }
    }
    result
}

fn replace_role(messages:&Vec<Message>, role:MessageRole) -> Vec<Message> {
    let mut result = messages.clone();
    result.iter_mut().for_each(|m| { m.role = role.clone() });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::{ApprovalRequest, ApprovalResponse};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::UserId;
    use uuid::Uuid;

    fn envelope(sender_id: ActorId, event: ActorEvent) -> Envelope {
        let context = EventContext::new(Default::default(), Default::default(), sender_id, "test".to_string());
        Envelope::new(SecurityContext::default(), context, event)
    }

    fn approval_request() -> ApprovalRequest {
        ApprovalRequest {
            id: Default::default(),
            session_id: Default::default(),
            channel_id: Default::default(),
            agent_id: AgentId::new(),
            tool_call: ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}) },
            description: "test".to_string(),
        }
    }

    #[test]
    fn approval_requested_from_agent_is_internal_only() {
        let actors = ModeratorActors::new();
        let event = envelope(ActorId::Agent(AgentId::new()), ActorEvent::ApprovalRequested(approval_request()));
        let decision = actors.route_approval_events(&event);
        assert!(!decision.broadcast);
    }

    #[test]
    fn approval_requested_from_approver_is_broadcast() {
        let actors = ModeratorActors::new();
        let event = envelope(ActorId::Approver(Uuid::new_v4()), ActorEvent::ApprovalRequested(approval_request()));
        let decision = actors.route_approval_events(&event);
        assert!(decision.broadcast);
    }

    #[test]
    fn approval_granted_from_user_is_broadcast() {
        let actors = ModeratorActors::new();
        let response = ApprovalResponse { id: Default::default(), agent_id: AgentId::new() };
        let event = envelope(ActorId::User(UserId::new()), ActorEvent::ApprovalGranted(response));
        let decision = actors.route_approval_events(&event);
        assert!(decision.broadcast);
    }

    #[test]
    fn approval_granted_from_approver_is_internal_only() {
        let actors = ModeratorActors::new();
        let response = ApprovalResponse { id: Default::default(), agent_id: AgentId::new() };
        let event = envelope(ActorId::Approver(Uuid::new_v4()), ActorEvent::ApprovalGranted(response));
        let decision = actors.route_approval_events(&event);
        assert!(!decision.broadcast);
    }
}
