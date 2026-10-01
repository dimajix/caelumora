//! The `policy_moderator` component: the moderator every channel has unless
//! it is configured with another (issue 174). It routes the way every
//! channel routes - every message to every other participant, approvals,
//! failed turns and commands to whom they concern, and a response request to
//! every agent a user `@`-mentioned - and asks the `ResponderPolicy` of its
//! channel, its slot, whom to ask when nobody was mentioned (issue 44).

use crate::actor::Actor;
use crate::component::{ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry};
use crate::moderator::{Moderator, RouteDecision};
use crate::moderator_impl::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::message::{Message, MessageRole};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::warn;

pub const POLICY_MODERATOR_KIND: &str = "policy_moderator";

/// A moderator that routes like every other and asks whom `policy` chooses
/// to respond when a message mentions nobody.
#[derive(Debug)]
pub struct PolicyModerator {
    actors: Mutex<ModeratorActors>,
    policy: ComponentImpl<dyn ResponderPolicy>,
}

impl PolicyModerator {
    pub fn new(policy: ComponentImpl<dyn ResponderPolicy>) -> Self {
        Self { actors: Mutex::new(ModeratorActors::new()), policy }
    }

    /// The actors, locked only while they are read or changed, never while
    /// the policy is asked.
    fn actors(&self) -> std::sync::MutexGuard<'_, ModeratorActors> {
        self.actors.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn route_full_response(&self, event: &Envelope, messages: &Vec<Message>) -> Vec<(ActorId, Envelope)> {
        let (mut routes, mentioned, candidates) = {
            let actors = self.actors();
            (actors.forward_routes(event, messages), actors.mentioned_responders(event, messages), actors.actor_candidates(messages))
        };
        if routes.is_empty() {
            // Nothing anyone could answer, e.g. only tool calls.
            return routes;
        }

        // Mentioned agents respond instead of those the policy would choose;
        // the policy is not consulted, so a rotation is left as it is.
        let responders = match mentioned {
            Some(mentioned) => mentioned,
            None => self.policy.choose(event.context.sender_id, candidates).await,
        };
        for id in responders {
            routes.push((id, event.forward(ActorEvent::ResponseRequested)));
        }
        routes
    }
}

#[async_trait]
impl Moderator for PolicyModerator {
    async fn route(&self, event: &Envelope) -> RouteDecision {
        match &event.event {
            ActorEvent::FullResponseGenerated(messages) => {
                self.route_full_response(event, messages).await.into()
            }
            ActorEvent::ApprovalRequested(_) |
            ActorEvent::ApprovalGranted(_) |
            ActorEvent::ApprovalDenied(_) |
            ActorEvent::ApprovalTimedOut(_) |
            ActorEvent::ApprovalRequestWithdrawn { .. } => {
                self.actors().route_approval_events(event)
            }
            ActorEvent::TurnFailed { .. } | ActorEvent::CommandExecuted { .. } | ActorEvent::ParticipantRemoved { .. } => {
                self.actors().route_to_users(event)
            }
            _ => {
                Vec::new().into()
            }
        }
    }

    fn add_actor(&self, actor: Arc<dyn Actor>) {
        self.actors().add_actor(actor);
    }

    fn remove_actor(&self, actor: ActorId) {
        self.actors().remove_actor(actor);
    }
}

/// The actors of a channel, by kind. Plain maps behind the moderator's one
/// lock (issue 114).
#[derive(Debug, Default)]
struct ModeratorActors {
    agent_actors: HashMap<ActorId, Arc<dyn Actor>>,
    user_actors: HashMap<ActorId, Arc<dyn Actor>>,
    approval_actors: HashMap<ActorId, Arc<dyn Actor>>,
}

impl ModeratorActors {
    fn new() -> Self {
        Self::default()
    }

    /// Files `actor` by its kind, which its id tells.
    fn add_actor(&mut self, actor: Arc<dyn Actor>) {
        let id = actor.id();
        match id {
            ActorId::User(_) => { self.user_actors.insert(id, actor); }
            ActorId::Approver(_) => { self.approval_actors.insert(id, actor); }
            ActorId::Agent(_) => { self.agent_actors.insert(id, actor); }
            ActorId::None | ActorId::System => warn!("Actor {} takes no part in routing", id),
        }
    }

    fn remove_actor(&mut self, id: ActorId) {
        self.agent_actors.remove(&id);
        self.user_actors.remove(&id);
        self.approval_actors.remove(&id);
    }

    fn forward_routes(&self, event: &Envelope, messages: &Vec<Message>) -> Vec<(ActorId, Envelope)> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let user_messages = as_user_messages(messages);

        if user_messages.is_empty() {
            return Vec::new();
        }

        let forward_event = event.forward(ActorEvent::FullResponseGenerated(user_messages));
        self.agent_actors.keys()
            .chain(self.user_actors.keys())
            .filter(|id| !sender_ids.contains(id))
            .map(|id| (*id, forward_event.clone()))
            .collect()
    }

    fn actor_candidates(&self, messages: &[Message]) -> Vec<ActorId> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        self.agent_actors.iter()
            .filter(|(_, actor)| actor.state() != ActorState::Sleeping)
            .map(|(id, _)| *id)
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

    /// Forwards `event` to every user in the channel, e.g. a `TurnFailed`,
    /// which concerns the users watching and none of the other agents.
    fn route_to_users(&self, event: &Envelope) -> RouteDecision {
        self.user_actors.keys()
            .map(|id| (*id, event.clone()))
            .collect::<Vec<_>>()
            .into()
    }

    /// Routes the approval of a tool call (issues 81, 148): the agent's
    /// request and its withdrawal go to the approvers, which ask the users
    /// and tell them; the users' answers go to the approvers; an approver's
    /// decision goes to the agent that asked, and to the users, who learn
    /// so that the approval is over.
    fn route_approval_events(&self, event: &Envelope) -> RouteDecision {
        let to_approvers = || self.approval_actors.keys().map(|id| (*id, event.clone())).collect::<Vec<_>>();
        let to_users = || self.user_actors.keys().map(|id| (*id, event.clone())).collect::<Vec<_>>();
        let sender = event.context.sender_id;
        match &event.event {
            ActorEvent::ApprovalRequested(_) | ActorEvent::ApprovalRequestWithdrawn { .. } => match sender {
                // Agent -> ApproverActor: internal plumbing, not shown to users yet.
                ActorId::Agent(_) => RouteDecision { forwards: to_approvers(), broadcast: false },
                // ApproverActor -> Users: what users act on, or no longer.
                ActorId::Approver(_) => RouteDecision { forwards: to_users(), broadcast: true },
                _ => RouteDecision { forwards: Vec::new(), broadcast: false },
            },
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) | ActorEvent::ApprovalTimedOut(response) => match sender {
                // User -> ApproverActor: lets other observers see the answer
                // too. A timeout is the approver's alone.
                ActorId::User(_) if !matches!(event.event, ActorEvent::ApprovalTimedOut(_)) => {
                    RouteDecision { forwards: to_approvers(), broadcast: true }
                }
                // ApproverActor -> Agent, and the users: the decision.
                ActorId::Approver(_) => {
                    let agent_id = ActorId::Agent(response.agent_id);
                    let mut forwards = to_users();
                    if self.agent_actors.contains_key(&agent_id) {
                        forwards.push((agent_id, event.clone()));
                    }
                    RouteDecision { forwards, broadcast: true }
                }
                _ => RouteDecision { forwards: Vec::new(), broadcast: false },
            },
            _ => RouteDecision { forwards: Vec::new(), broadcast: false }
        }
    }
}


/// What `messages` are to the other participants, in one pass: the text of
/// each, as said by a user - without tool calls and tool results, which are
/// the sender's own business. Tool messages are left out entirely.
fn as_user_messages(messages: &[Message]) -> Vec<Message> {
    messages.iter()
        .filter(|m| m.content.is_some() && m.role != MessageRole::Tool)
        .map(|m| Message { role: MessageRole::User, tool_calls: Vec::new(), tool_results: Vec::new(), ..m.clone() })
        .collect()
}


pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(POLICY_MODERATOR_KIND).provides::<dyn Moderator>(),
        |_descriptor, context| async move {
            // The channel's policy, or the default one.
            let policy = context.one::<dyn ResponderPolicy>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn Moderator>(Arc::new(PolicyModerator::new(policy)));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moderator::Moderator;
    use crate::moderator_impl::{broadcast_policy, mentions_only_policy, random_policy, round_robin_policy, user_driven_policy};
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::{ApprovalRequest, ApprovalResponse};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::UserId;

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
            tool_call: ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default() },
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
    fn add_agents(moderator: &dyn Moderator) -> Vec<ActorId> {
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
            ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT)
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

    /// `policy` as a component.
    fn policy(policy: impl ResponderPolicy + 'static) -> ComponentImpl<dyn ResponderPolicy> {
        ComponentImpl { id: "responders".to_string(), fail_policy: Default::default(), facet: Arc::new(policy) }
    }

    /// The moderators that ask someone to respond to a user when nobody is
    /// mentioned.
    fn answering_moderators() -> Vec<PolicyModerator> {
        vec![
            PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy)),
            PolicyModerator::new(policy(random_policy::RandomPolicy::new())),
            PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default())),
            PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy)),
        ]
    }

    fn moderators() -> Vec<PolicyModerator> {
        let mut all = answering_moderators();
        all.push(PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy)));
        all
    }

    /// The moderator built from the channel components `config`, as a
    /// channel builds it (issue 174).
    async fn built(config: &[(&str, &str)]) -> Arc<dyn Moderator> {
        use crate::component::{ComponentContainer, ComponentContext};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::security::DummySecurityManager;
        let store = Arc::new(caelumora_model::store_impl::memory::MemoryStore::new());
        let registry = Arc::new(ComponentRegistry::built_in(store));
        let security = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(caelumora_model::secret::SecretManager::new(vec![], security.clone()));
        let context = ComponentContext::new(SecurityContext::default(), security, secrets);
        let config = ComponentConfig { component_nodes: config.iter().map(|(id, kind)| ComponentDescriptor::new(id, kind)).collect() };
        let container = ComponentContainer::from_config(&config, &registry, &context).await.unwrap();
        container.one::<dyn Moderator>().await.unwrap().facet
    }

    #[tokio::test]
    async fn the_policy_moderator_asks_the_policy_of_its_channel_or_the_default() {
        let user = ActorId::User(UserId::new());

        let broadcast = built(&[("responders", broadcast_policy::BROADCAST_POLICY_KIND)]).await;
        let agents = add_agents(broadcast.as_ref());
        let asked = responders(&broadcast.route(&message_from(user, "hello", Vec::new())).await);
        assert_eq!(asked.into_iter().collect::<HashSet<_>>(), HashSet::from([agents[0], agents[2]]), "every awake agent");

        let default = built(&[]).await;
        let agents = add_agents(default.as_ref());
        let first = responders(&default.route(&message_from(user, "one", Vec::new())).await);
        let second = responders(&default.route(&message_from(user, "two", Vec::new())).await);
        assert_eq!(first.len(), 1, "one agent at a time");
        assert_ne!(first, second, "round robin by default");
        assert!(!first.contains(&agents[1]) && !second.contains(&agents[1]), "not the sleeping one");
    }

    #[tokio::test]
    async fn every_moderator_asks_exactly_the_mentioned_agents_to_respond() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            let event = message_from(user, "@b and @c, please", vec![agents[1], agents[2]]);

            let decision = moderator.route(&event).await;

            assert_eq!(responders(&decision), vec![agents[1], agents[2]], "{:?} must ask the mentioned agents, the sleeping one included", moderator);
            assert_eq!(receivers(&decision), agents.iter().copied().collect(), "{:?} must still deliver to every agent", moderator);
        }
    }

    #[tokio::test]
    async fn mentions_of_non_agents_fall_back_to_the_moderator_policy() {
        for moderator in answering_moderators() {
            let agents = add_agents(&moderator);
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
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy));
        let agents = add_agents(&moderator);
        let event = message_from(agents[0], "@b, your turn", vec![agents[1]]);

        let decision = moderator.route(&event).await;

        assert_eq!(responders(&decision), vec![agents[2]], "an agent's mention must not override the policy");
    }

    #[tokio::test]
    async fn mentions_only_asks_nobody_without_a_mention() {
        let moderator = PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy));
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());

        let decision = moderator.route(&message_from(user, "hello, everyone", Vec::new())).await;
        assert!(responders(&decision).is_empty(), "nobody was mentioned, so nobody answers");
        assert_eq!(receivers(&decision), agents.iter().copied().collect(), "every agent still receives the message");

        let decision = moderator.route(&message_from(agents[0], "@b, your turn", vec![agents[1]])).await;
        assert!(responders(&decision).is_empty(), "an agent's mention is not routed");
    }

    #[tokio::test]
    async fn user_driven_asks_every_agent_for_a_user_and_nobody_for_an_agent() {
        let moderator = PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy));
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());

        let decision = moderator.route(&message_from(user, "hello, everyone", Vec::new())).await;
        assert_eq!(responders(&decision).into_iter().collect::<HashSet<_>>(), HashSet::from([agents[0], agents[2]]), "every awake agent answers a user");

        let decision = moderator.route(&message_from(user, "@b", vec![agents[1]])).await;
        assert_eq!(responders(&decision), vec![agents[1]], "a user's mention asks only the mentioned agent");

        let decision = moderator.route(&message_from(agents[0], "my answer", Vec::new())).await;
        assert!(responders(&decision).is_empty(), "an agent's message asks nobody");
        assert_eq!(receivers(&decision), HashSet::from([agents[1], agents[2]]), "but is delivered to the other agents");
    }

    #[tokio::test]
    async fn round_robin_takes_turns_and_a_mention_does_not_advance_it() {
        let moderator = PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default()));
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        let mut awake = vec![agents[0], agents[2]];
        awake.sort_unstable_by_key(|id| id.id());

        let first = responders(&moderator.route(&message_from(user, "one", Vec::new())).await);
        let mentioned = responders(&moderator.route(&message_from(user, "@b", vec![agents[1]])).await);
        let second = responders(&moderator.route(&message_from(user, "two", Vec::new())).await);
        let third = responders(&moderator.route(&message_from(user, "three", Vec::new())).await);

        assert_eq!(first, vec![awake[0]]);
        assert_eq!(mentioned, vec![agents[1]]);
        assert_eq!(second, vec![awake[1]], "the mention must not have taken a turn");
        assert_eq!(third, vec![awake[0]]);
    }

    #[tokio::test]
    async fn every_moderator_forwards_a_failed_turn_to_the_users_only() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let event = envelope(agents[0], ActorEvent::TurnFailed { message: "boom".to_string() });

            let decision = moderator.route(&event).await;

            // TestActor registers as an agent, so there are no users here:
            // nothing may be routed to the agents, but the UI broadcast stays.
            assert!(decision.forwards.is_empty(), "{:?} must not route a failed turn to agents", moderator);
            assert!(decision.broadcast, "{:?} must broadcast a failed turn", moderator);
        }
    }

    #[tokio::test]
    async fn every_moderator_forwards_an_announced_command_to_the_users_only() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            let event = envelope(user, ActorEvent::CommandExecuted { command: "wakeup".to_string(), output: "Woke b.".to_string() });

            let decision = moderator.route(&event).await;

            // Commands address the system: the agents must not see them.
            assert!(decision.forwards.iter().all(|(to, _)| !agents.contains(to)), "{:?} must not route a command to agents", moderator);
            assert!(decision.broadcast, "{:?} must broadcast an announced command", moderator);
        }
    }

    #[tokio::test]
    async fn every_moderator_forwards_a_removal_to_the_users_only() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            let event = envelope(user, ActorEvent::ParticipantRemoved { participant: agents[0] });

            let decision = moderator.route(&event).await;

            assert!(decision.forwards.iter().all(|(to, _)| !agents.contains(to)), "{:?} must not route a removal to agents", moderator);
            assert!(decision.broadcast, "{:?} must broadcast a removal", moderator);
        }
    }

    #[test]
    fn others_hear_what_was_said_as_a_user_without_the_tool_traffic() {
        use caelumora_model::tool::{ToolCallResult, ToolVisibility};
        let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN };
        let result = ToolCallResult::of(&call, Default::default(), "done".to_string(), caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT);
        let said = vec![
            Message { role: MessageRole::Assistant, content: Some("Let me check.".to_string()), tool_calls: vec![call], ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) },
            Message { role: MessageRole::Tool, content: Some("done".to_string()), tool_results: vec![result], ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) },
            Message { role: MessageRole::Assistant, content: None, ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) },
            Message { role: MessageRole::Assistant, content: Some("All done.".to_string()), ..Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT) },
        ];

        let heard = as_user_messages(&said);

        let texts: Vec<_> = heard.iter().map(|m| m.content.as_deref().unwrap()).collect();
        assert_eq!(texts, ["Let me check.", "All done."], "neither the tool message nor an empty one");
        assert!(heard.iter().all(|m| m.role == MessageRole::User && m.tool_calls.is_empty() && m.tool_results.is_empty()));
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
        let event = envelope(ActorId::Approver(caelumora_model::approval::ApproverId::new()), ActorEvent::ApprovalRequested(approval_request()));
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
    fn a_decision_of_the_approver_reaches_the_users_too() {
        let actors = ModeratorActors::new();
        let response = ApprovalResponse { id: Default::default(), agent_id: AgentId::new() };
        for decided in [ActorEvent::ApprovalGranted(response.clone()), ActorEvent::ApprovalDenied(response.clone()), ActorEvent::ApprovalTimedOut(response.clone())] {
            let event = envelope(ActorId::Approver(caelumora_model::approval::ApproverId::new()), decided);
            assert!(actors.route_approval_events(&event).broadcast, "{} ends the approval for the users", event.event.kind());
        }
    }

    #[test]
    fn a_withdrawn_request_goes_to_the_approvers_and_from_them_to_the_users() {
        let actors = ModeratorActors::new();
        let withdrawn = ActorEvent::ApprovalRequestWithdrawn { id: Default::default() };
        assert!(!actors.route_approval_events(&envelope(ActorId::Agent(AgentId::new()), withdrawn.clone())).broadcast);
        assert!(actors.route_approval_events(&envelope(ActorId::Approver(caelumora_model::approval::ApproverId::new()), withdrawn.clone())).broadcast);
        let decision = actors.route_approval_events(&envelope(ActorId::User(UserId::new()), withdrawn));
        assert!(decision.forwards.is_empty() && !decision.broadcast, "only an agent withdraws");
    }
}
