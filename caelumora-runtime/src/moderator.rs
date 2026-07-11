use async_trait::async_trait;
use rand::seq::IndexedRandom;
use std::any::Any;
use std::collections::HashSet;
use std::fmt::Debug;
use std::sync::Arc;

use crate::actor::Actor;
use crate::actor_impl::approval::ApprovalActor;
use crate::actor_impl::user::UserActor;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use caelumora_model::agent::ProcessingState;
use caelumora_model::message::{Message, MessageRole};
use dashmap::DashMap;
use rand::rngs::StdRng;

#[async_trait]
pub trait Moderator : Send + Sync + Debug {
    async fn route(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)>;
    fn add_actor(&mut self, actor: Arc<dyn Actor>) -> ();
    fn remove_actor(&mut self, actor: Arc<dyn Actor>) -> ();
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
        else if any_actor.downcast_ref::<ApprovalActor>().is_some() {
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
            .filter(|r| r.value().state() != ProcessingState::Sleep)
            .map(|r| *r.key())
            .filter(|id| !sender_ids.contains(id))
            .collect()
    }

    fn route_approval_events(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match &event.event {
            ActorEvent::ToolExecutionRequested(_) => {
                if matches!(event.context.sender_id, ActorId::Agent(_)) {
                    // Agent -> ApprovalActor
                    self.approval_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect()
                } else if matches!(event.context.sender_id, ActorId::Approval(_)) {
                    // ApprovalActor -> Users
                    self.user_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            ActorEvent::ToolExecutionGranted(response) | ActorEvent::ToolExecutionDenied(response) => {
                if matches!(event.context.sender_id, ActorId::User(_)) {
                    // User -> ApprovalActor
                    self.approval_actors.iter()
                        .map(|r| (*r.key(), event.clone()))
                        .collect()
                } else if matches!(event.context.sender_id, ActorId::Approval(_)) {
                    // ApprovalActor -> Agent
                    let agent_id = ActorId::Agent(response.agent_id);
                    if self.agent_actors.contains_key(&agent_id) {
                        vec![(agent_id, event.clone())]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            ActorEvent::ToolExecutionRequestTimedOut(response) => {
                if matches!(event.context.sender_id, ActorId::Approval(_)) {
                    // ApprovalActor -> Agent
                    let agent_id = ActorId::Agent(response.agent_id);
                    if self.agent_actors.contains_key(&agent_id) {
                        vec![(agent_id, event.clone())]
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new()
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
    async fn route(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await
            }
            ActorEvent::ToolExecutionRequested(_) |
            ActorEvent::ToolExecutionGranted(_) |
            ActorEvent::ToolExecutionDenied(_) |
            ActorEvent::ToolExecutionRequestTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new()
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
    async fn route(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await
            }
            ActorEvent::ToolExecutionRequested(_) |
            ActorEvent::ToolExecutionGranted(_) |
            ActorEvent::ToolExecutionDenied(_) |
            ActorEvent::ToolExecutionRequestTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new()
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
    async fn route(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match event.event {
            ActorEvent::FullResponseGenerated(_) => {
                self.handle_full_response(event).await
            }
            ActorEvent::ToolExecutionRequested(_) |
            ActorEvent::ToolExecutionGranted(_) |
            ActorEvent::ToolExecutionDenied(_) |
            ActorEvent::ToolExecutionRequestTimedOut(_) => {
                self.actors.route_approval_events(event)
            }
            _ => {
                Vec::new()
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
