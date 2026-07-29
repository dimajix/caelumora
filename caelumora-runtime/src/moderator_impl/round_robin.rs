use crate::actor::Actor;
use crate::moderator::{Moderator, RouteDecision};
use crate::moderator_impl::ModeratorActors;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use std::sync::Arc;

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
        if self.actors.agent_actors.is_empty() {
            self.next_agent_index = 0;
        }
        else {
            self.next_agent_index %= self.actors.agent_actors.len();
        }
    }
}

