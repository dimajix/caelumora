//! The `policy_moderator` component: the moderator every channel has unless
//! it is configured with another (issue 174). It routes the way every
//! channel routes - every message to every other participant, approvals,
//! failed turns and commands to whom they concern, and a response request to
//! every agent a message `@`-mentions - and asks the `ResponderPolicy` of its
//! channel, its slot, whom to ask when nobody was mentioned (issue 44).
//!
//! An agent's message that makes other agents respond is a hop of a chain
//! between agents, whether they were mentioned or chosen by the policy. The
//! moderator counts the hops since the last user's message
//! (`EventContext::agent_hops`) and asks nobody once they would exceed
//! `max_agent_hops` (parameter, default `DEFAULT_MAX_AGENT_HOPS`; issue 45).

use crate::actor::Actor;
use crate::component::{ComponentError, ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry};
use crate::moderator::Moderator;
use crate::moderator_impl::ResponderPolicy;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::message::Message;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

pub const POLICY_MODERATOR_KIND: &str = "policy_moderator";

/// How many hops a chain between agents may have since the last user's
/// message, unless the moderator is configured otherwise.
pub const DEFAULT_MAX_AGENT_HOPS: u32 = 3;

/// A moderator that routes like every other and asks whom `policy` chooses
/// to respond when a message mentions nobody - in a chain between agents
/// only up to `max_agent_hops` hops.
#[derive(Debug)]
pub struct PolicyModerator {
    actors: Mutex<ModeratorActors>,
    policy: ComponentImpl<dyn ResponderPolicy>,
    max_agent_hops: u32,
}

impl PolicyModerator {
    pub fn new(policy: ComponentImpl<dyn ResponderPolicy>, max_agent_hops: u32) -> Self {
        Self { actors: Mutex::new(ModeratorActors::new()), policy, max_agent_hops }
    }

    /// The actors, locked only while they are read or changed, never while
    /// the policy is asked.
    fn actors(&self) -> std::sync::MutexGuard<'_, ModeratorActors> {
        self.actors.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn route_full_response(&self, event: &Envelope, messages: &Vec<Message>) -> Vec<(ActorId, Envelope)> {
        // What there is to hear of it, by the rule every agent hears by
        // (issue 224): nothing, e.g. of a response of tool calls only or of
        // a turn cancelled before it said anything.
        let heard = messages.iter().any(Message::is_heard);
        let (mut routes, mentioned, candidates, users) = {
            let actors = self.actors();
            (actors.forward_routes(event, messages, heard), actors.mentioned_responders(messages), actors.actor_candidates(messages), actors.user_ids())
        };
        if !heard {
            // Nothing anyone could answer: the users learn that the answer
            // ended, and nobody is asked.
            return routes;
        }

        // A user's message starts a chain; whoever responds to an agent's
        // is one hop further from it. Beyond the limit nobody is asked, and
        // the policy is not consulted either, so a rotation is left as it is.
        let agent_hops = match event.context.sender_id {
            ActorId::Agent(_) => event.context.agent_hops + 1,
            _ => 0,
        };
        if agent_hops > self.max_agent_hops {
            debug!(agent_hops, "Asking nobody to respond: the chain between agents has reached its limit");
            routes.extend(self.cut_off(event, messages, mentioned.unwrap_or_default(), users));
            return routes;
        }

        // Mentioned agents respond instead of those the policy would choose;
        // the policy is not consulted, so a rotation is left as it is.
        let responders = match mentioned {
            Some(mentioned) => mentioned,
            None => self.policy.choose(event.context.sender_id, candidates).await,
        };
        let mut request = event.forward(ActorEvent::ResponseRequested);
        request.context.agent_hops = agent_hops;
        for id in responders {
            routes.push((id, request.clone()));
        }
        routes
    }

    /// Tells that the response `messages` of `event` made nobody respond,
    /// since its chain reached the limit (issue 200): a `ChainCutOff` in the
    /// name of the system, to the `users` and to the agents involved - its
    /// sender and the agents it mentioned, `not_asked`.
    fn cut_off(&self, event: &Envelope, messages: &[Message], not_asked: Vec<ActorId>, users: Vec<ActorId>) -> Vec<(ActorId, Envelope)> {
        let Some(last) = messages.last() else {
            return Vec::new();
        };
        let mut notice = event.forward(ActorEvent::ChainCutOff { message: last.id, max_agent_hops: self.max_agent_hops, not_asked: not_asked.clone() });
        notice.context.sender_id = ActorId::System;
        notice.context.sender_name = "system".to_string();
        let mut recipients = users;
        recipients.push(event.context.sender_id);
        recipients.extend(not_asked);
        recipients.into_iter().map(|id| (id, notice.clone())).collect()
    }
}

#[async_trait]
impl Moderator for PolicyModerator {
    async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        match &event.event {
            ActorEvent::FullResponseGenerated(messages) => {
                self.route_full_response(event, messages).await
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
            // A change to what was said reaches everyone who may hold the
            // message (issue 234); each applies it if it does.
            ActorEvent::MessageModified(_) | ActorEvent::MessageRevoked { .. } => {
                self.actors().route_to_participants(event)
            }
            // Whom a request to answer again names, and what it answers, to
            // the users (issue 234).
            ActorEvent::RetryRequested { author, .. } => {
                self.actors().route_to_agent(*author, event)
            }
            ActorEvent::RetryAccepted { .. } | ActorEvent::RetryRejected { .. } => {
                self.actors().route_to_users(event)
            }
            // How an answer comes about - streamed, step by step - is for
            // the users watching (issue 220); the agents hear the answer.
            ActorEvent::ResponseStarted |
            ActorEvent::PartialMessageGenerated(_) |
            ActorEvent::CompleteMessageGenerated(_) |
            ActorEvent::IntermediatePartialGenerated(_) |
            ActorEvent::IntermediateMessageGenerated(_) |
            ActorEvent::PlanStepStarted { .. } |
            ActorEvent::PlanStepFinished { .. } => {
                self.actors().route_to_users(event)
            }
            _ => {
                Vec::new()
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

    /// Forwards the response `event`, of `messages`, as it was said, to
    /// everyone but its senders: to every user, who learns that the answer
    /// ended even when there is nothing to hear of it (issue 224), and to
    /// every agent if there is something to hear - `heard` - which each
    /// takes in by the rule all agents hear by (`Message::as_heard`).
    fn forward_routes(&self, event: &Envelope, messages: &[Message], heard: bool) -> Vec<(ActorId, Envelope)> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let agents = heard.then(|| self.agent_actors.keys()).into_iter().flatten();
        self.user_actors.keys()
            .chain(agents)
            .filter(|id| !sender_ids.contains(id))
            .map(|id| (*id, event.clone()))
            .collect()
    }

    /// The agents a policy may choose from: awake, and not among the
    /// senders. Leaving the sleeping out matters beyond the policy: the
    /// channel wakes whomever the moderator asks to respond, so a sleeping
    /// agent chosen here would be woken without being addressed (issue 45).
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
    /// asked to respond, and nobody else - whether a user or an agent wrote
    /// it; the limit on hops between agents bounds the latter (issue 45).
    /// Sleeping agents are not filtered out here - the channel wakes whom
    /// the moderator asks to respond before it delivers anything of the
    /// decision.
    fn mentioned_responders(&self, messages: &[Message]) -> Option<Vec<ActorId>> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let mut responders: Vec<ActorId> = Vec::new();
        for id in messages.iter().flat_map(|m| m.mentions.iter()) {
            if self.agent_actors.contains_key(id) && !sender_ids.contains(id) && !responders.contains(id) {
                responders.push(*id);
            }
        }

        if responders.is_empty() { None } else { Some(responders) }
    }

    /// The users in the channel.
    fn user_ids(&self) -> Vec<ActorId> {
        self.user_actors.keys().copied().collect()
    }

    /// Forwards `event` to every user and every agent in the channel but its
    /// sender, e.g. a change to a message said (issue 234), which everyone
    /// who heard the message takes in.
    fn route_to_participants(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        self.user_actors.keys()
            .chain(self.agent_actors.keys())
            .filter(|id| **id != event.context.sender_id)
            .map(|id| (*id, event.clone()))
            .collect()
    }

    /// Forwards `event` to the agent `agent` alone, if it is in the channel:
    /// e.g. a request to answer again instead of its message (issue 234).
    fn route_to_agent(&self, agent: ActorId, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        self.agent_actors.contains_key(&agent).then(|| (agent, event.clone())).into_iter().collect()
    }

    /// Forwards `event` to every user in the channel, e.g. a `TurnFailed`
    /// or a streamed part of an answer, which concern the users watching
    /// and none of the other agents.
    fn route_to_users(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        self.user_actors.keys()
            .map(|id| (*id, event.clone()))
            .collect()
    }

    /// Routes the approval of a tool call (issues 81, 148): the agent's
    /// request and its withdrawal go to the approvers, which ask the users
    /// and tell them; the users' answers go to the approvers; an approver's
    /// decision goes to the agent that asked, and to the users, who learn
    /// so that the approval is over.
    fn route_approval_events(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        let to_approvers = || self.approval_actors.keys().map(|id| (*id, event.clone())).collect::<Vec<_>>();
        let to_users = || self.user_actors.keys().map(|id| (*id, event.clone())).collect::<Vec<_>>();
        let sender = event.context.sender_id;
        match &event.event {
            ActorEvent::ApprovalRequested(_) | ActorEvent::ApprovalRequestWithdrawn { .. } => match sender {
                // Agent -> ApproverActor: internal plumbing, not shown to users yet.
                ActorId::Agent(_) => to_approvers(),
                // ApproverActor -> Users: what users act on, or no longer.
                ActorId::Approver(_) => to_users(),
                _ => Vec::new(),
            },
            ActorEvent::ApprovalGranted(response) | ActorEvent::ApprovalDenied(response) | ActorEvent::ApprovalTimedOut(response) => match sender {
                // User -> ApproverActor. A timeout is the approver's alone.
                ActorId::User(_) if !matches!(event.event, ActorEvent::ApprovalTimedOut(_)) => to_approvers(),
                // ApproverActor -> Agent, and the users: the decision.
                ActorId::Approver(_) => {
                    let agent_id = ActorId::Agent(response.agent_id);
                    let mut forwards = to_users();
                    if self.agent_actors.contains_key(&agent_id) {
                        forwards.push((agent_id, event.clone()));
                    }
                    forwards
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}


pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(POLICY_MODERATOR_KIND).provides::<dyn Moderator>(),
        |descriptor, context| async move {
            let max_agent_hops = match descriptor.params.get("max_agent_hops") {
                None => DEFAULT_MAX_AGENT_HOPS,
                Some(value) => value.as_u64().and_then(|hops| u32::try_from(hops).ok())
                    .ok_or_else(|| ComponentError::InvalidConfig("parameter 'max_agent_hops' must be a number of at least 0".to_string()))?,
            };
            // The channel's policy, or the default one.
            let policy = context.one::<dyn ResponderPolicy>().await?;
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn Moderator>(Arc::new(PolicyModerator::new(policy, max_agent_hops)));
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
    use caelumora_model::message::MessageRole;
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
        fn stop(&self) {}
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

    fn responders(decision: &[(ActorId, Envelope)]) -> Vec<ActorId> {
        decision.iter()
            .filter(|(_, e)| matches!(e.event, ActorEvent::ResponseRequested))
            .map(|(id, _)| *id)
            .collect()
    }

    fn receivers(decision: &[(ActorId, Envelope)]) -> HashSet<ActorId> {
        decision.iter()
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
            PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), DEFAULT_MAX_AGENT_HOPS),
            PolicyModerator::new(policy(random_policy::RandomPolicy::new()), DEFAULT_MAX_AGENT_HOPS),
            PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default()), DEFAULT_MAX_AGENT_HOPS),
            PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy), DEFAULT_MAX_AGENT_HOPS),
        ]
    }

    fn moderators() -> Vec<PolicyModerator> {
        let mut all = answering_moderators();
        all.push(PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy), DEFAULT_MAX_AGENT_HOPS));
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
        let secrets = Arc::new(caelumora_model::secret::SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
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
    async fn every_moderator_asks_the_agents_an_agent_mentions_one_hop_further() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let event = message_from(agents[0], "@b, your turn", vec![agents[1]]);

            let decision = moderator.route(&event).await;

            assert_eq!(responders(&decision), vec![agents[1]], "{:?} must ask the agent an agent mentioned", moderator);
            let request = decision.iter().find(|(_, e)| matches!(e.event, ActorEvent::ResponseRequested)).map(|(_, e)| e);
            assert_eq!(request.map(|e| e.context.agent_hops), Some(1), "{:?} must count the hop", moderator);
        }
    }

    /// A message of `sender` in a chain `agent_hops` hops from the last
    /// user's message.
    fn hops_from(sender_id: ActorId, agent_hops: u32, mentions: Vec<ActorId>) -> Envelope {
        let mut event = message_from(sender_id, "and you?", mentions);
        event.context.agent_hops = agent_hops;
        event
    }

    #[tokio::test]
    async fn a_chain_between_agents_ends_at_the_limit_and_a_user_starts_a_new_one() {
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), 2);
        let agents = add_agents(&moderator);
        let hops_asked = |decision: &[(ActorId, Envelope)]| decision.iter()
            .filter(|(_, e)| matches!(e.event, ActorEvent::ResponseRequested))
            .map(|(_, e)| e.context.agent_hops)
            .collect::<Vec<_>>();

        let decision = moderator.route(&hops_from(agents[0], 1, Vec::new())).await;
        assert_eq!(hops_asked(&decision), vec![2], "the policy's choice is a hop, too");

        let decision = moderator.route(&hops_from(agents[0], 2, vec![agents[2]])).await;
        assert!(responders(&decision).is_empty(), "beyond the limit nobody is asked, mentioned or not");
        assert_eq!(receivers(&decision), HashSet::from([agents[1], agents[2]]), "but the message is still delivered");

        let user = ActorId::User(UserId::new());
        let decision = moderator.route(&hops_from(user, 2, Vec::new())).await;
        assert_eq!(hops_asked(&decision), vec![0, 0], "a user's message starts a new chain");
    }

    #[tokio::test]
    async fn how_an_answer_comes_about_reaches_the_users_only() {
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), DEFAULT_MAX_AGENT_HOPS);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        moderator.add_actor(Arc::new(TestActor { id: user, state: ActorState::Idle }));
        let message = || Message::new(caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT);
        let step = || (0, 2, "first".to_string());
        let events = [
            ActorEvent::ResponseStarted,
            ActorEvent::PartialMessageGenerated(message()),
            ActorEvent::CompleteMessageGenerated(message()),
            ActorEvent::IntermediatePartialGenerated(message()),
            ActorEvent::IntermediateMessageGenerated(message()),
            ActorEvent::PlanStepStarted { step_index: step().0, total_steps: step().1, description: step().2 },
            ActorEvent::PlanStepFinished { step_index: step().0, total_steps: step().1, description: step().2 },
        ];
        for event in events {
            let kind = event.kind();
            let decision = moderator.route(&envelope(agents[0], event)).await;
            let to: Vec<ActorId> = decision.iter().map(|(id, _)| *id).collect();
            assert_eq!(to, vec![user], "{kind} goes to the users, not to the agents (issue 220)");
        }
    }

    #[tokio::test]
    async fn a_chain_cut_off_is_told_to_the_users_and_the_agents_involved() {
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), 2);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        moderator.add_actor(Arc::new(TestActor { id: user, state: ActorState::Idle }));
        let cut_off = |decision: &[(ActorId, Envelope)]| decision.iter()
            .filter_map(|(id, e)| match &e.event {
                ActorEvent::ChainCutOff { max_agent_hops, not_asked, .. } => Some((*id, e.context.sender_id, *max_agent_hops, not_asked.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        let decision = moderator.route(&hops_from(agents[0], 1, vec![agents[2]])).await;
        assert!(cut_off(&decision).is_empty(), "below the limit, nothing is cut off");

        let event = hops_from(agents[0], 2, vec![agents[2]]);
        let decision = moderator.route(&event).await;
        let told = cut_off(&decision);
        let recipients: HashSet<ActorId> = told.iter().map(|(id, ..)| *id).collect();
        assert_eq!(recipients, HashSet::from([user, agents[0], agents[2]]), "the users, the sender and whom it mentioned - not agents[1]");
        assert!(told.iter().all(|(_, sender, hops, not_asked)| *sender == ActorId::System && *hops == 2 && *not_asked == vec![agents[2]]));
        let ActorEvent::FullResponseGenerated(messages) = &event.event else { unreachable!() };
        assert!(decision.iter().any(|(_, e)| matches!(&e.event, ActorEvent::ChainCutOff { message, .. } if *message == messages[0].id)), "it names the message cut off");

        let decision = moderator.route(&hops_from(agents[0], 2, Vec::new())).await;
        let told = cut_off(&decision);
        assert_eq!(told.iter().map(|(id, ..)| *id).collect::<HashSet<_>>(), HashSet::from([user, agents[0]]));
        assert!(told.iter().all(|(.., not_asked)| not_asked.is_empty()), "told even when nobody was mentioned");
    }

    #[tokio::test]
    async fn the_limit_of_hops_is_a_parameter_of_the_moderator() {
        use crate::component::{ComponentContainer, ComponentContext};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::security::DummySecurityManager;
        let store = Arc::new(caelumora_model::store_impl::memory::MemoryStore::new());
        let registry = Arc::new(ComponentRegistry::built_in(store));
        let security = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(caelumora_model::secret::SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&security)));
        let context = ComponentContext::new(SecurityContext::default(), security, secrets);
        let configured = |max_agent_hops: serde_json::Value| ComponentConfig { component_nodes: vec![ComponentDescriptor {
            params: serde_json::json!({ "max_agent_hops": max_agent_hops }),
            ..ComponentDescriptor::new("moderator", POLICY_MODERATOR_KIND)
        }] };

        let container = ComponentContainer::from_config(&configured(serde_json::json!(0)), &registry, &context).await.unwrap();
        let moderator = container.one::<dyn Moderator>().await.unwrap().facet;
        let agents = add_agents(moderator.as_ref());
        let decision = moderator.route(&message_from(agents[0], "@c", vec![agents[2]])).await;
        assert!(responders(&decision).is_empty(), "with no hops allowed, agents answer only users");

        let invalid = ComponentContainer::from_config(&configured(serde_json::json!(-1)), &registry, &context).await;
        assert!(invalid.is_err(), "a negative limit is refused");
    }

    #[tokio::test]
    async fn mentions_only_asks_nobody_without_a_mention() {
        let moderator = PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy), DEFAULT_MAX_AGENT_HOPS);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());

        let decision = moderator.route(&message_from(user, "hello, everyone", Vec::new())).await;
        assert!(responders(&decision).is_empty(), "nobody was mentioned, so nobody answers");
        assert_eq!(receivers(&decision), agents.iter().copied().collect(), "every agent still receives the message");

        let decision = moderator.route(&message_from(agents[0], "my answer", Vec::new())).await;
        assert!(responders(&decision).is_empty(), "nor does anybody answer an agent without a mention");
    }

    #[tokio::test]
    async fn user_driven_asks_every_agent_for_a_user_and_nobody_for_an_agent() {
        let moderator = PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy), DEFAULT_MAX_AGENT_HOPS);
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
        let moderator = PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default()), DEFAULT_MAX_AGENT_HOPS);
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
            let user = ActorId::User(UserId::new());
            moderator.add_actor(Arc::new(TestActor { id: user, state: ActorState::Idle }));
            let event = envelope(agents[0], ActorEvent::TurnFailed { message: "boom".to_string() });

            let decision = moderator.route(&event).await;

            assert_eq!(decision.iter().map(|(to, _)| *to).collect::<Vec<_>>(), vec![user], "{:?} must route a failed turn to the users only", moderator);
        }
    }

    #[tokio::test]
    async fn every_moderator_forwards_an_announced_command_to_the_users_only() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let (user, other) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(TestActor { id: other, state: ActorState::Idle }));
            let event = envelope(user, ActorEvent::CommandExecuted { command: "wakeup".to_string(), output: "Woke b.".to_string() });

            let decision = moderator.route(&event).await;

            // Commands address the system: the agents must not see them.
            assert!(decision.iter().all(|(to, _)| !agents.contains(to)), "{:?} must not route a command to agents", moderator);
            assert!(decision.iter().any(|(to, _)| *to == other), "{:?} must tell the other users", moderator);
        }
    }

    #[tokio::test]
    async fn every_moderator_forwards_a_removal_to_the_users_only() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let (user, other) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(TestActor { id: other, state: ActorState::Idle }));
            let event = envelope(user, ActorEvent::ParticipantRemoved { participant: agents[0] });

            let decision = moderator.route(&event).await;

            assert!(decision.iter().all(|(to, _)| !agents.contains(to)), "{:?} must not route a removal to agents", moderator);
            assert!(decision.iter().any(|(to, _)| *to == other), "{:?} must tell the users", moderator);
        }
    }

    #[tokio::test]
    async fn a_response_is_forwarded_as_said_and_one_with_nothing_to_hear_only_to_the_users() {
        use caelumora_model::tool::{ToolCall, ToolCallResult, ToolVisibility};
        let classified = caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT;
        let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN };
        let result = ToolCallResult::of(&call, Default::default(), "done".to_string(), classified.clone());
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let (sender, user) = (agents[0], ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(TestActor { id: user, state: ActorState::Idle }));
            let said = |messages: Vec<Message>| envelope(sender, ActorEvent::FullResponseGenerated(messages));
            let calling = Message { sender_id: sender, role: MessageRole::Assistant, content: Some("Let me check.".to_string()), tool_calls: vec![call.clone()], ..Message::new(classified.clone()) };
            let tool = Message { sender_id: sender, role: MessageRole::Tool, content: Some("done".to_string()), tool_results: vec![result.clone()], ..Message::new(classified.clone()) };
            let silent = Message { content: None, ..calling.clone() };

            let decision = moderator.route(&said(vec![calling.clone(), tool.clone()])).await;
            assert_eq!(receivers(&decision), HashSet::from([agents[1], agents[2], user]), "{:?}: to everyone but its sender", moderator);
            let forwarded = decision.iter().find_map(|(_, e)| match &e.event { ActorEvent::FullResponseGenerated(m) => Some(m.clone()), _ => None }).unwrap();
            assert!(forwarded[0].role == MessageRole::Assistant && forwarded[0].tool_calls.len() == 1 && forwarded[1].role == MessageRole::Tool, "{:?}: as it was said", moderator);

            // A response of tool calls only, or a turn cancelled before it
            // said anything: the users learn that it ended, nobody else
            // hears anything, and nobody is asked.
            for nothing in [vec![silent.clone(), tool.clone()], Vec::new()] {
                let decision = moderator.route(&said(nothing)).await;
                assert_eq!(receivers(&decision), HashSet::from([user]), "{:?}: the users alone", moderator);
                assert!(responders(&decision).is_empty(), "{:?}: nobody is asked", moderator);
            }
        }
    }

    /// The actors of a channel with one agent, one user and one approver.
    fn approval_parties() -> (ModeratorActors, AgentId, ActorId, ActorId) {
        let mut actors = ModeratorActors::new();
        let agent = AgentId::new();
        let user = ActorId::User(UserId::new());
        let approver = ActorId::Approver(caelumora_model::approval::ApproverId::new());
        for id in [ActorId::Agent(agent), user, approver] {
            actors.add_actor(Arc::new(TestActor { id, state: ActorState::Idle }));
        }
        (actors, agent, user, approver)
    }

    /// To whom of `actors` an approval event of `sender` is routed.
    fn routed_to(actors: &ModeratorActors, sender: ActorId, event: ActorEvent) -> HashSet<ActorId> {
        actors.route_approval_events(&envelope(sender, event)).into_iter().map(|(to, _)| to).collect()
    }

    #[test]
    fn a_request_reaches_the_users_only_through_the_approver() {
        let (actors, agent, user, approver) = approval_parties();
        let requested = || ActorEvent::ApprovalRequested(approval_request());
        assert_eq!(routed_to(&actors, ActorId::Agent(agent), requested()), HashSet::from([approver]), "internal: not shown to the users yet");
        assert_eq!(routed_to(&actors, approver, requested()), HashSet::from([user]), "what the users act on");
    }

    #[test]
    fn a_users_answer_goes_to_the_approver() {
        let (actors, agent, _, approver) = approval_parties();
        let response = ApprovalResponse { id: Default::default(), agent_id: agent };
        assert_eq!(routed_to(&actors, ActorId::User(UserId::new()), ActorEvent::ApprovalGranted(response)), HashSet::from([approver]));
    }

    #[test]
    fn a_decision_of_the_approver_reaches_the_agent_and_the_users() {
        let (actors, agent, user, approver) = approval_parties();
        let response = ApprovalResponse { id: Default::default(), agent_id: agent };
        for decided in [ActorEvent::ApprovalGranted(response.clone()), ActorEvent::ApprovalDenied(response.clone()), ActorEvent::ApprovalTimedOut(response.clone())] {
            let kind = decided.kind();
            assert_eq!(routed_to(&actors, approver, decided), HashSet::from([user, ActorId::Agent(agent)]), "{kind} ends the approval for the users");
        }
    }

    #[test]
    fn a_withdrawn_request_goes_to_the_approvers_and_from_them_to_the_users() {
        let (actors, agent, user, approver) = approval_parties();
        let withdrawn = || ActorEvent::ApprovalRequestWithdrawn { id: Default::default() };
        assert_eq!(routed_to(&actors, ActorId::Agent(agent), withdrawn()), HashSet::from([approver]));
        assert_eq!(routed_to(&actors, approver, withdrawn()), HashSet::from([user]));
        assert!(routed_to(&actors, user, withdrawn()).is_empty(), "only an agent withdraws");
    }
}
