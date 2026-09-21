pub mod broadcast;
pub mod random;
pub mod round_robin;

use crate::actor::Actor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::user::UserActor;
use crate::moderator::RouteDecision;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::message::{Message, MessageRole};
use dashmap::DashMap;
use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

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

    /// Agents explicitly addressed by `@`-mentions in `messages` (issue 29),
    /// or `None` if there are none - in which case the moderator applies its
    /// own policy. A mention overrides that policy: every mentioned agent is
    /// asked to respond, and nobody else. Only mentions written by a user are
    /// honoured; mentions in agent responses are rendered but not routed, so
    /// two agents cannot address each other indefinitely. Sleeping agents are
    /// not filtered out here - the channel wakes them before routing.
    fn mentioned_responders(&self, event: &Envelope, messages: &[Message]) -> Option<Vec<ActorId>> {
        if !matches!(event.context.sender_id, ActorId::User(_)) {
            return None;
        }

        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let mut responders: Vec<ActorId> = Vec::new();
        for id in messages.iter().flat_map(|m| m.mentions.iter()) {
            if self.agent_actors.contains_key(id) && !sender_ids.contains(id) && !responders.contains(id) {
                responders.push(*id);
            }
        }

        if responders.is_empty() { None } else { Some(responders) }
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
    use crate::moderator::Moderator;
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

    #[derive(Debug)]
    struct TestActor {
        id: ActorId,
        state: ActorState,
    }

    #[async_trait::async_trait]
    impl Actor for TestActor {
        fn id(&self) -> ActorId { self.id }
        fn kind(&self) -> String { "test".to_string() }
        fn name(&self) -> String { "test".to_string() }
        fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
        async fn run(&self, _rx: Box<dyn crate::transport::EventReceiver>, _tx: Box<dyn crate::transport::EventSender>) -> anyhow::Result<()> {
            Ok(())
        }
        fn state(&self) -> ActorState { self.state }
    }

    /// Three agents, the middle one asleep, registered with `moderator`.
    fn add_agents(moderator: &mut dyn Moderator) -> Vec<ActorId> {
        let states = [ActorState::Idle, ActorState::Sleeping, ActorState::Idle];
        states.iter().map(|state| {
            let id = ActorId::Agent(AgentId::new());
            moderator.add_actor(Arc::new(TestActor { id, state: *state }));
            id
        }).collect()
    }

    fn message_from(sender_id: ActorId, content: &str, mentions: Vec<ActorId>) -> Envelope {
        let message = Message {
            sender_id,
            role: MessageRole::User,
            content: Some(content.to_string()),
            mentions,
            ..Message::default()
        };
        envelope(sender_id, ActorEvent::FullResponseGenerated(vec![message]))
    }

    fn responders(decision: &RouteDecision) -> Vec<ActorId> {
        decision.forwards.iter()
            .filter(|(_, e)| matches!(e.event, ActorEvent::ResponseRequested))
            .map(|(id, _)| *id)
            .collect()
    }

    fn receivers(decision: &RouteDecision) -> HashSet<ActorId> {
        decision.forwards.iter()
            .filter(|(_, e)| matches!(e.event, ActorEvent::FullResponseGenerated(_)))
            .map(|(id, _)| *id)
            .collect()
    }

    fn moderators() -> Vec<Box<dyn Moderator>> {
        vec![
            Box::new(broadcast::BroadcastModerator::new()),
            Box::new(random::RandomModerator::new()),
            Box::new(round_robin::RoundRobinModerator::new()),
        ]
    }

    #[tokio::test]
    async fn every_moderator_asks_exactly_the_mentioned_agents_to_respond() {
        for mut moderator in moderators() {
            let agents = add_agents(moderator.as_mut());
            let user = ActorId::User(UserId::new());
            let event = message_from(user, "@b and @c, please", vec![agents[1], agents[2]]);

            let decision = moderator.route(&event).await;

            assert_eq!(responders(&decision), vec![agents[1], agents[2]], "{:?} must ask the mentioned agents, the sleeping one included", moderator);
            assert_eq!(receivers(&decision), agents.iter().copied().collect(), "{:?} must still deliver to every agent", moderator);
        }
    }

    #[tokio::test]
    async fn mentions_of_non_agents_fall_back_to_the_moderator_policy() {
        for mut moderator in moderators() {
            let agents = add_agents(moderator.as_mut());
            let user = ActorId::User(UserId::new());
            let other_user = ActorId::User(UserId::new());
            let event = message_from(user, "@other", vec![other_user]);

            let decision = moderator.route(&event).await;

            let asked = responders(&decision);
            assert!(!asked.is_empty(), "{:?} must apply its own policy", moderator);
            assert!(!asked.contains(&agents[1]), "{:?} must not ask the sleeping agent without a mention", moderator);
        }
    }

    #[tokio::test]
    async fn mentions_written_by_agents_are_not_routed() {
        let mut moderator = broadcast::BroadcastModerator::new();
        let agents = add_agents(&mut moderator);
        let event = message_from(agents[0], "@b, your turn", vec![agents[1]]);

        let decision = moderator.route(&event).await;

        assert_eq!(responders(&decision), vec![agents[2]], "an agent's mention must not override the policy");
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
