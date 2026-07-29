use crate::actor::Actor;
use crate::moderator::{Moderator, RouteDecision};
use crate::moderator_impl::ModeratorActors;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use std::sync::Arc;


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
            ActorEvent::PlanApprovalRequested(_) |
            ActorEvent::PlanApprovalGranted(_) |
            ActorEvent::PlanApprovalDenied(_) |
            ActorEvent::PlanApprovalTimedOut(_) => {
                self.actors.route_plan_approval_events(event)
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
