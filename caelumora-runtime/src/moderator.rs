use async_trait::async_trait;
use std::any::Any;
use std::collections::{HashSet};
use std::fmt::Debug;
use rand::seq::{IndexedRandom};

use caelumora_model::message::{ActorId, Message, MessageRole};
use dashmap::DashSet;
use rand::rngs::StdRng;
use crate::actor::{Actor, ActorEvent, Envelope};
use crate::actor_impl::user::UserActor;

#[async_trait]
pub trait Moderator : Send + Sync + Debug {
    async fn route(&mut self, event: &Envelope) -> Vec<(ActorId, Envelope)>;
    fn add_actor(&mut self, actor: &dyn Actor) -> ();
    fn remove_actor(&mut self, actor: &dyn Actor) -> ();
}


#[derive(Debug)]
struct ModeratorActors {
    agent_actors: DashSet<ActorId>,
    user_actors: DashSet<ActorId>,
}

impl ModeratorActors {
    fn new() -> Self {
        Self {
            agent_actors: DashSet::new(),
            user_actors: DashSet::new(),
        }
    }

    fn add_actor(&mut self, actor: &dyn Actor) {
        let any_actor:&dyn Any = actor;
        if any_actor.downcast_ref::<UserActor>().is_some() {
            self.user_actors.insert(actor.id());
        }
        else {
            self.agent_actors.insert(actor.id());
        }
    }

    fn remove_actor(&mut self, actor: &dyn Actor) {
        self.agent_actors.remove(&actor.id());
        self.user_actors.remove(&actor.id());
    }

    fn forward_routes(&self, event: &Envelope, messages: &[Message]) -> Vec<(ActorId, Envelope)> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let user_messages = replace_role(messages, MessageRole::User);
        let forward_event = event.forward_with_response(user_messages);

        self.agent_actors.iter()
            .chain(self.user_actors.iter())
            .filter(|id| !sender_ids.contains(id))
            .map(|id| (*id, forward_event.clone()))
            .collect()
    }

    fn actor_candidates(&self, messages: &[Message]) -> Vec<ActorId> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        self.agent_actors.iter()
            .map(|id| *id)
            .filter(|id| !sender_ids.contains(id))
            .collect()
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

            // Pick random actor for response
            let possible_actors = self.actors.actor_candidates(messages);
            for id in possible_actors {
                let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                routes.push((id, request_response_event));
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
            _ => {
                Vec::new()
            }
        }
    }

    fn add_actor(&mut self, actor: &dyn Actor) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: &dyn Actor) -> () {
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

            // Pick random actor for response
            let possible_actors = self.actors.actor_candidates(messages);
            if let Some(id) = possible_actors.choose(&mut self.rng) {
                let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                routes.push((*id, request_response_event));
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
            _ => {
                Vec::new()
            }
        }
    }

    fn add_actor(&mut self, actor: &dyn Actor) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: &dyn Actor) -> () {
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

            // Pick the next actor for response in round-robin order
            let mut possible_actors = self.actors.actor_candidates(messages);
            possible_actors.sort_unstable_by_key(|id| id.id());

            if !possible_actors.is_empty() {
                let index = self.next_agent_index % possible_actors.len();
                let id = possible_actors[index];
                self.next_agent_index = (index + 1) % possible_actors.len();

                let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                routes.push((id, request_response_event));
            }
            else {
                self.next_agent_index = 0;
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
            _ => {
                Vec::new()
            }
        }
    }

    fn add_actor(&mut self, actor: &dyn Actor) -> () {
        self.actors.add_actor(actor);
    }

    fn remove_actor(&mut self, agent: &dyn Actor) -> () {
        self.actors.remove_actor(agent);
        if self.actors.agent_actors.is_empty() {
            self.next_agent_index = 0;
        }
        else {
            self.next_agent_index %= self.actors.agent_actors.len();
        }
    }
}


fn replace_role(messages:&[Message], role:MessageRole) -> Vec<Message> {
    let mut result = messages.to_vec();
    result.iter_mut().for_each(|m| { m.role = role.clone() });
    result
}
