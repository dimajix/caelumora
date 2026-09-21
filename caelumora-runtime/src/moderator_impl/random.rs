use crate::actor::Actor;
use crate::moderator::{Moderator, RouteDecision};
use crate::moderator_impl::ModeratorActors;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use rand::prelude::StdRng;
use rand::seq::IndexedRandom;
use std::sync::Arc;

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

            if let Some(mentioned) = self.actors.mentioned_responders(event, messages) {
                // Mentioned agents respond instead of a random one
                for id in mentioned {
                    let request_response_event = event.forward_with_event(ActorEvent::ResponseRequested);
                    routes.push((id, request_response_event));
                }
            } else if !routes.is_empty() {
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
