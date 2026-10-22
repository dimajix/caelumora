//! The `policy_moderator` component: the moderator every channel has unless
//! it is configured with another (issue 174). It lets every event go where
//! every channel delivers it (issue 418), asks every agent a message
//! `@`-mentions to respond, and asks the `ResponderPolicy` of its channel,
//! its slot, whom to ask when nobody was mentioned (issue 44). Whose mention
//! replaces the policy is `mentions_replace_policy` (parameter, default
//! `anyone`; issue 437): a mention of any participant, so that a message to
//! a person asks no agent the policy would choose, or of an agent alone.
//!
//! An agent's message that makes other agents respond is a hop of a chain
//! between agents, whether they were mentioned or chosen by the policy. The
//! moderator counts the hops since the last user's message
//! (`EventContext::agent_hops`) and asks nobody once they would exceed
//! `max_agent_hops` (parameter, default `DEFAULT_MAX_AGENT_HOPS`; issue 45).
//! Beneath that, it asks nobody to respond to an event as deep in its span
//! as `max_span_depth` (parameter, default `DEFAULT_MAX_SPAN_DEPTH`; issue
//! 422): the net against routing that goes on without end, as only an ask
//! makes anyone respond. Past either bound, what was said is delivered as
//! ever.
//!
//! It keeps the last `context_messages` messages heard (parameter, default
//! `DEFAULT_CONTEXT_MESSAGES`; issue 414) as they stand after every change,
//! and tells its policy the conversation: what was said before what it is
//! to choose for, and that.

use crate::actor::Actor;
use crate::component::{ComponentError, ComponentFacets, ComponentImpl, ComponentKind, ComponentRegistry};
use crate::moderator::{Moderator, Notice, ResponderPolicy, Routing};
use async_trait::async_trait;
use caelumora_model::actor::{ActorDescription, ActorEvent, ActorId, ActorState, Envelope, RequestReason};
use caelumora_model::message::Message;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use tracing::{debug, warn};

pub const POLICY_MODERATOR_KIND: &str = "policy_moderator";

/// How many hops a chain between agents may have since the last user's
/// message, unless the moderator is configured otherwise.
pub const DEFAULT_MAX_AGENT_HOPS: u32 = 3;

/// How deep in its span (`EventContext::span_depth`) an event may be for the
/// moderator to ask anyone to respond to it, unless it is configured
/// otherwise.
pub const DEFAULT_MAX_SPAN_DEPTH: i32 = 100;

/// How many messages heard the moderator keeps to tell its policy, unless
/// it is configured otherwise.
pub const DEFAULT_CONTEXT_MESSAGES: usize = 10;

/// Whose `@`-mention replaces the moderator's policy (issue 437): a message
/// that mentions them is addressed to them, and of those only the agents are
/// asked to respond. A mention of a sender of the message never counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MentionsReplacePolicy {
    /// Any participant: a message to a person asks nobody.
    #[default]
    Anyone,
    /// An agent alone: a message that mentions only people is answered by
    /// whom the policy chooses.
    Agents,
}

impl MentionsReplacePolicy {
    /// The parameter `mentions_replace_policy` of a moderator's `params`,
    /// `Anyone` if it has none.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` if it is neither `anyone` nor
    /// `agents`.
    fn of_params(params: &Value) -> Result<Self, ComponentError> {
        match params.get("mentions_replace_policy") {
            None => Ok(Self::default()),
            Some(value) => match value.as_str() {
                Some("anyone") => Ok(Self::Anyone),
                Some("agents") => Ok(Self::Agents),
                _ => Err(ComponentError::InvalidConfig("parameter 'mentions_replace_policy' must be 'anyone' or 'agents'".to_string())),
            },
        }
    }
}

/// A moderator that delivers like every other and asks whom `policy` chooses
/// to respond when a message mentions nobody whose mention replaces it
/// (`mentions_replace_policy`) - in a chain between agents only up to
/// `max_agent_hops` hops, and for no event `max_span_depth` deep -, told the
/// last `context_messages` messages heard before the message.
#[derive(Debug)]
pub struct PolicyModerator {
    actors: Mutex<ModeratorActors>,
    policy: ComponentImpl<dyn ResponderPolicy>,
    mentions_replace_policy: MentionsReplacePolicy,
    max_agent_hops: u32,
    max_span_depth: i32,
    /// The messages heard, the oldest first, at most `context_messages`.
    heard: Mutex<VecDeque<Message>>,
    context_messages: usize,
}

impl PolicyModerator {
    pub fn new(policy: ComponentImpl<dyn ResponderPolicy>, max_agent_hops: u32, context_messages: usize) -> Self {
        Self {
            actors: Mutex::new(ModeratorActors::new()),
            policy,
            mentions_replace_policy: MentionsReplacePolicy::default(),
            max_agent_hops,
            max_span_depth: DEFAULT_MAX_SPAN_DEPTH,
            heard: Mutex::default(),
            context_messages,
        }
    }

    /// This moderator, its policy replaced by the mentions of those
    /// `mentions_replace_policy` says.
    pub fn with_mentions_replace_policy(self, mentions_replace_policy: MentionsReplacePolicy) -> Self {
        Self { mentions_replace_policy, ..self }
    }

    /// This moderator, asking nobody to respond to an event `max_span_depth`
    /// deep in its span.
    pub fn with_max_span_depth(self, max_span_depth: i32) -> Self {
        Self { max_span_depth, ..self }
    }

    /// The actors, locked only while they are read or changed, never while
    /// the policy is asked.
    fn actors(&self) -> MutexGuard<'_, ModeratorActors> {
        self.actors.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The messages heard, locked only while they are read or changed,
    /// never while the policy is asked.
    fn heard(&self) -> MutexGuard<'_, VecDeque<Message>> {
        self.heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Keeps those of `messages` there is something to hear of
    /// (`Message::is_heard`), dropping the oldest beyond `context_messages`.
    /// A message is kept once its answer is complete, after whom to ask was
    /// decided: a message of a turn at the turn's end, so that a long turn
    /// does not push out what was said before it. What a turn given up said
    /// is not kept.
    fn hear(&self, messages: &[Message]) {
        let mut heard = self.heard();
        heard.extend(messages.iter().filter(|message| message.is_heard()).cloned());
        let excess = heard.len().saturating_sub(self.context_messages);
        heard.drain(..excess);
    }

    /// Applies the change of a message `event` tells, if it is one, to the
    /// message kept, by the rules the message keeps (issue 234). A change of
    /// the channel's classification is no event and does not reach the
    /// moderator; it only lowers a message's, so the messages kept are at
    /// worst classified higher than they are.
    fn apply_change(&self, event: &ActorEvent) {
        let mut heard = self.heard();
        let mut kept = heard.iter_mut();
        match event {
            ActorEvent::MessageModified(modified) => {
                if let Some(message) = kept.find(|message| message.id == modified.id) {
                    message.modify(modified);
                }
            }
            ActorEvent::MessageRevoked { message: id, at } => {
                if let Some(message) = kept.find(|message| message.id == *id) {
                    message.revoke(*at);
                }
            }
            ActorEvent::MessageReclassified { message: id, classification } => {
                if let Some(message) = kept.find(|message| message.id == *id) {
                    message.reclassify(classification);
                }
            }
            _ => {}
        }
    }

    /// Whom to ask to respond to the answer `messages` of `event` - a turn
    /// that finished, or a message said outside of any turn (issue 258) -
    /// and, at the limit of a chain, the notice that nobody is.
    async fn ask(&self, event: &Envelope, messages: &[Message]) -> Routing {
        // What there is to hear of it, by the rule every agent hears by
        // (issue 224): nothing, e.g. of a response of tool calls only or of
        // a turn cancelled before it said anything.
        let heard = messages.iter().any(Message::is_heard);
        let (mentioned, candidates, users) = {
            let actors = self.actors();
            (actors.mentioned_responders(messages, self.mentions_replace_policy), actors.actor_candidates(messages), actors.user_ids())
        };
        let conversation: Vec<Message> = self.heard().iter().chain(messages).cloned().collect();
        if !heard {
            // Nothing anyone could answer: nobody is asked.
            return Routing::default();
        }
        // Routed that deep, something goes on without end: nobody is asked,
        // whatever the chain between agents.
        if event.context.span_depth >= self.max_span_depth {
            warn!(span_depth = event.context.span_depth, "Asking nobody to respond: the event is as deep in its span as the moderator allows");
            return Routing::default();
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
            return self.cut_off(event, messages, mentioned.unwrap_or_default(), users);
        }

        // Mentioned agents respond instead of those the policy would choose -
        // nobody, if the message is addressed to people alone; the policy is
        // not consulted, so a rotation is left as it is.
        let (ask, reason) = match mentioned {
            Some(mentioned) => (mentioned, RequestReason::Mentioned),
            None => (self.policy.choose(event, &conversation, candidates).await, RequestReason::Chosen),
        };
        Routing { ask, reason, agent_hops, ..Routing::default() }
    }

    /// Tells that the response `messages` of `event` made nobody respond,
    /// since its chain reached the limit (issue 200): a `ChainCutOff` to the
    /// `users` and to the agents involved - its sender and the agents it
    /// mentioned, `not_asked`.
    fn cut_off(&self, event: &Envelope, messages: &[Message], not_asked: Vec<ActorId>, users: Vec<ActorId>) -> Routing {
        let Some(last) = messages.last() else {
            return Routing::default();
        };
        let notice = Notice::ChainCutOff { message: last.id, max_agent_hops: self.max_agent_hops, not_asked: not_asked.clone() };
        let mut recipients = users;
        recipients.push(event.context.sender_id);
        recipients.extend(not_asked);
        Routing { notices: vec![(recipients, notice)], ..Routing::default() }
    }
}

#[async_trait]
impl Moderator for PolicyModerator {
    /// Every event goes where every channel delivers it; what is decided
    /// here is whom to ask to respond, and what to tell when a chain ends.
    async fn route(&self, event: &Envelope) -> Routing {
        match &event.event {
            // The end of a turn: whoever is to respond is asked (issue 258).
            ActorEvent::TurnFinished { messages, .. } => {
                let routing = self.ask(event, messages).await;
                self.hear(messages);
                routing
            }
            // A message said: one of no turn - a user's - is answered by
            // itself; one of a turn with the turn's end. The kind of its
            // sender does not matter.
            ActorEvent::CompleteMessageGenerated(message) if message.turn_id.is_none() => {
                let messages = std::slice::from_ref(message);
                let routing = self.ask(event, messages).await;
                self.hear(messages);
                routing
            }
            // A user nudges someone (issue 278): they alone are asked to
            // respond, as if a message of the user had mentioned them - no
            // policy is consulted, and no chain between agents counted.
            ActorEvent::NudgeRequested { participant } => {
                let ask = self.actors().takes_part(*participant).then_some(*participant).into_iter().collect();
                Routing { ask, reason: RequestReason::Nudged, agent_hops: event.context.agent_hops, ..Routing::default() }
            }
            // A change to what was said, in what the moderator keeps of
            // what was heard (issue 234).
            ActorEvent::MessageModified(_) | ActorEvent::MessageRevoked { .. } | ActorEvent::MessageReclassified { .. } => {
                self.apply_change(&event.event);
                Routing::default()
            }
            _ => Routing::default(),
        }
    }

    fn add_actor(&self, actor: Arc<dyn Actor>) {
        self.actors().add_actor(actor);
    }

    fn remove_actor(&self, actor: ActorId) {
        self.actors().remove_actor(actor);
    }
}

/// The actors of a channel, by kind: whom a policy chooses from, who is
/// mentioned and who is told a chain ended. Plain maps behind the
/// moderator's one lock (issue 114).
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

    /// Whether `participant`, of whatever kind, is in the channel.
    fn takes_part(&self, participant: ActorId) -> bool {
        [&self.agent_actors, &self.user_actors, &self.approval_actors].iter().any(|actors| actors.contains_key(&participant))
    }

    /// The agents a policy may choose from, as they are described to
    /// everyone in the channel (issue 403): awake, and not among the
    /// senders. Leaving the sleeping out matters beyond the policy: the
    /// channel wakes whomever the moderator asks to respond, so a sleeping
    /// agent chosen here would be woken without being addressed (issue 45).
    fn actor_candidates(&self, messages: &[Message]) -> Vec<ActorDescription> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        self.agent_actors.iter()
            .filter(|(id, actor)| actor.state() != ActorState::Sleeping && !sender_ids.contains(id))
            .map(|(_, actor)| actor.describe())
            .collect()
    }

    /// The agents explicitly addressed by `@`-mentions in `messages` (issue
    /// 29), if the messages are addressed by a mention that `replacing` says
    /// replaces the moderator's policy - of an agent, or of any participant
    /// (issue 437) -, else `None`, in which case the moderator applies its
    /// policy. A mention overrides that policy: every mentioned agent is
    /// asked to respond, and nobody else - nobody at all for a message to
    /// people alone -, whether a user or an agent wrote it; the limit on
    /// hops between agents bounds the latter (issue 45). A mention of a
    /// sender of the messages counts for nothing. Sleeping agents are not
    /// filtered out here - the channel wakes whom the moderator asks to
    /// respond before it delivers anything of the decision.
    fn mentioned_responders(&self, messages: &[Message], replacing: MentionsReplacePolicy) -> Option<Vec<ActorId>> {
        let sender_ids: HashSet<ActorId> = messages.iter().map(|m| m.sender_id).collect();
        let mut addressed = false;
        let mut responders: Vec<ActorId> = Vec::new();
        for id in messages.iter().flat_map(|m| m.mentions.iter()).filter(|id| !sender_ids.contains(id)) {
            let agent = self.agent_actors.contains_key(id);
            addressed |= agent || (replacing == MentionsReplacePolicy::Anyone && self.takes_part(*id));
            if agent && !responders.contains(id) {
                responders.push(*id);
            }
        }

        addressed.then_some(responders)
    }

    /// The users in the channel.
    fn user_ids(&self) -> Vec<ActorId> {
        self.user_actors.keys().copied().collect()
    }
}


pub fn register(registry: &mut ComponentRegistry) {
    registry.register_async(
        ComponentKind::new(POLICY_MODERATOR_KIND).provides::<dyn Moderator>().needs::<dyn ResponderPolicy>(),
        |descriptor, context| async move {
            let max_agent_hops = match descriptor.params.get("max_agent_hops") {
                None => DEFAULT_MAX_AGENT_HOPS,
                Some(value) => value.as_u64().and_then(|hops| u32::try_from(hops).ok())
                    .ok_or_else(|| ComponentError::InvalidConfig("parameter 'max_agent_hops' must be a number of at least 0".to_string()))?,
            };
            let max_span_depth = match descriptor.params.get("max_span_depth") {
                None => DEFAULT_MAX_SPAN_DEPTH,
                Some(value) => value.as_u64().and_then(|depth| i32::try_from(depth).ok())
                    .ok_or_else(|| ComponentError::InvalidConfig("parameter 'max_span_depth' must be a number of at least 0".to_string()))?,
            };
            let mentions_replace_policy = MentionsReplacePolicy::of_params(&descriptor.params)?;
            let context_messages = match descriptor.params.get("context_messages") {
                None => DEFAULT_CONTEXT_MESSAGES,
                Some(value) => value.as_u64().and_then(|count| usize::try_from(count).ok())
                    .ok_or_else(|| ComponentError::InvalidConfig("parameter 'context_messages' must be a number of at least 0".to_string()))?,
            };
            // The channel's policy, or the default one.
            let policy = context.one::<dyn ResponderPolicy>().await?;
            let mut facets = ComponentFacets::empty();
            let moderator = PolicyModerator::new(policy, max_agent_hops, context_messages)
                .with_max_span_depth(max_span_depth)
                .with_mentions_replace_policy(mentions_replace_policy);
            facets.insert::<dyn Moderator>(Arc::new(moderator));
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moderator::Moderator;
    use crate::channel::ChannelLoop;
    use crate::testing::test_registry;
    use crate::moderator::{broadcast_policy, mentions_only_policy, random_policy, round_robin_policy, user_driven_policy};
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::ApprovalRequest;
    use caelumora_model::history::Audience;
    use caelumora_model::message::{MessageId, MessageRole};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::UserId;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::store::Persistence;
    use caelumora_model::turn::GivenUpReason;
    use caelumora_model::work::TurnId;
    use crate::testing::StandIn;
    use chrono::Utc;
    use serde_json::Value;

    fn envelope(sender_id: ActorId, event: ActorEvent) -> Envelope {
        let context = EventContext::new(Default::default(), Default::default(), sender_id, "test".to_string());
        Envelope::new(SecurityContext::default(), context, event)
    }

    /// Three agents, the middle one asleep, registered with `moderator`.
    fn add_agents(moderator: &dyn Moderator) -> Vec<ActorId> {
        let states = [ActorState::Idle, ActorState::Sleeping, ActorState::Idle];
        states.iter().map(|state| {
            let id = ActorId::Agent(AgentId::new());
            moderator.add_actor(Arc::new(StandIn::new(id, "test", *state)));
            id
        }).collect()
    }

    fn text_from(sender_id: ActorId, content: &str, mentions: Vec<ActorId>) -> Message {
        Message {
            sender_id,
            role: MessageRole::User,
            content: Some(content.to_string()),
            mentions,
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    /// The end of the answer `content` of `sender_id`, whom the moderator
    /// decides from whom to ask.
    fn message_from(sender_id: ActorId, content: &str, mentions: Vec<ActorId>) -> Envelope {
        envelope(sender_id, ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![text_from(sender_id, content, mentions)] })
    }

    /// The message `content` of `sender_id` as it is said (issue 258).
    fn said_from(sender_id: ActorId, content: &str, mentions: Vec<ActorId>) -> Envelope {
        envelope(sender_id, ActorEvent::CompleteMessageGenerated(text_from(sender_id, content, mentions)))
    }

    /// Whether `routing` delivers its event as every channel does, asks
    /// nobody and tells nothing of its own.
    fn as_every_channel(routing: &Routing) -> bool {
        routing.audience == Audience::Everybody && routing.ask.is_empty() && routing.notices.is_empty()
    }

    /// `policy` as a component.
    fn policy(policy: impl ResponderPolicy + 'static) -> ComponentImpl<dyn ResponderPolicy> {
        ComponentImpl { id: "responders".to_string(), fail_policy: Default::default(), facet: Arc::new(policy) }
    }

    /// What a policy was told once: the contents of the conversation and
    /// the candidates.
    type Told = (Vec<String>, Vec<ActorDescription>);

    /// A policy that asks nobody and records what it was told.
    #[derive(Debug, Clone, Default)]
    struct Recording(Arc<Mutex<Vec<Told>>>);

    #[async_trait]
    impl ResponderPolicy for Recording {
        async fn choose(&self, _event: &Envelope, messages: &[Message], candidates: Vec<ActorDescription>) -> Vec<ActorId> {
            let said = messages.iter().filter_map(|message| message.content.clone()).collect();
            self.0.lock().unwrap().push((said, candidates));
            Vec::new()
        }
    }

    impl Recording {
        /// What it was told last.
        fn last_told(&self) -> Told {
            self.0.lock().unwrap().last().cloned().expect("told something")
        }
    }

    /// Issue 403: a policy is told what was said, and the candidates as
    /// they are described to everyone in the channel - the agents awake
    /// that did not say it.
    #[tokio::test]
    async fn a_policy_is_told_what_was_said_and_who_the_candidates_are() {
        let recording = Recording::default();
        let moderator = PolicyModerator::new(policy(recording.clone()), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);

        moderator.route(&message_from(agents[0], "who knows?", vec![])).await;

        let told = recording.0.lock().unwrap().clone();
        let [(said, candidates)] = told.as_slice() else { panic!("asked once: {told:?}") };
        assert_eq!(said, &["who knows?"], "nothing was said before");
        assert_eq!(candidates.iter().map(|candidate| candidate.id).collect::<Vec<_>>(), [agents[2]], "awake, and not who said it");
        assert_eq!(candidates[0].name.as_str(), "test", "as it is described");
    }

    /// Issue 414: a policy is told the conversation - the last messages
    /// heard before, as they stand, and what it is to choose for: a message
    /// of a turn once the turn ended, nothing there is nothing to hear of,
    /// at most as many before as the moderator keeps.
    #[tokio::test]
    async fn a_policy_is_told_the_last_messages_heard_before_as_they_stand() {
        let recording = Recording::default();
        let moderator = PolicyModerator::new(policy(recording.clone()), DEFAULT_MAX_AGENT_HOPS, 3);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        let said = |content: &str| envelope(user, ActorEvent::CompleteMessageGenerated(Message { id: MessageId::new(), ..text_from(user, content, vec![]) }));

        moderator.route(&said("one")).await;
        let in_turn = Message { id: MessageId::new(), turn_id: Some(TurnId::new()), ..text_from(agents[0], "two", vec![]) };
        moderator.route(&envelope(agents[0], ActorEvent::CompleteMessageGenerated(in_turn.clone()))).await;
        moderator.route(&said("three")).await;
        assert_eq!(recording.last_told().0, ["one", "three"], "the turn has not ended yet");

        let tool_calls_only = Message { id: MessageId::new(), content: None, ..text_from(agents[0], "", vec![]) };
        moderator.route(&envelope(agents[0], ActorEvent::TurnFinished { turn_id: TurnId::new(), messages: vec![in_turn.clone(), tool_calls_only] })).await;
        assert_eq!(recording.last_told().0, ["one", "three", "two"], "what the turn said, last");

        moderator.route(&said("four")).await;
        assert_eq!(recording.last_told().0, ["one", "three", "two", "four"], "the turn's once it ended");

        let five = said("five");
        moderator.route(&five).await;
        assert_eq!(recording.last_told().0, ["three", "two", "four", "five"], "at most three before, the oldest dropped");

        let ActorEvent::CompleteMessageGenerated(five) = five.event else { unreachable!() };
        moderator.route(&envelope(user, ActorEvent::MessageModified(Message { content: Some("FIVE".to_string()), ..five }))).await;
        moderator.route(&envelope(agents[0], ActorEvent::MessageRevoked { message: in_turn.id, at: Utc::now() })).await;
        moderator.route(&said("six")).await;
        assert_eq!(recording.last_told().0, ["four", "FIVE", "six"], "modified, and the revoked one says nothing");
    }

    /// The moderators that ask someone to respond to a user when nobody is
    /// mentioned.
    fn answering_moderators() -> Vec<PolicyModerator> {
        vec![
            PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES),
            PolicyModerator::new(policy(random_policy::RandomPolicy::new()), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES),
            PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default()), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES),
            PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES),
        ]
    }

    fn moderators() -> Vec<PolicyModerator> {
        let mut all = answering_moderators();
        all.push(PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES));
        all
    }

    /// The moderator built from the channel components `config`, as a
    /// channel builds it (issue 174).
    async fn built(config: &[(&str, &str)]) -> Arc<dyn Moderator> {
        use crate::component::{ComponentContainer, ComponentContext};
        use crate::runtime::Services;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let store = Persistence::in_memory();
        let registry = Arc::new(test_registry(store.component_store().unwrap()));
        let context = ComponentContext::new(SecurityContext::default(), Services::empty());
        let config = ComponentConfig { component_nodes: config.iter().map(|(id, kind)| ComponentDescriptor::new(id, kind)).collect() };
        let container = ComponentContainer::from_config(&config, ChannelLoop::COMPONENT_FACETS, &registry, &context).await.unwrap();
        container.one::<dyn Moderator>().await.unwrap().facet
    }

    #[tokio::test]
    async fn the_policy_moderator_asks_the_policy_of_its_channel_or_the_default() {
        let user = ActorId::User(UserId::new());

        let broadcast = built(&[("responders", broadcast_policy::BROADCAST_POLICY_KIND)]).await;
        let agents = add_agents(broadcast.as_ref());
        let asked = broadcast.route(&message_from(user, "hello", Vec::new())).await.ask;
        assert_eq!(asked.into_iter().collect::<HashSet<_>>(), HashSet::from([agents[0], agents[2]]), "every awake agent");

        let default = built(&[]).await;
        let agents = add_agents(default.as_ref());
        let first = default.route(&message_from(user, "one", Vec::new())).await.ask;
        let second = default.route(&message_from(user, "two", Vec::new())).await.ask;
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

            let routing = moderator.route(&event).await;

            assert_eq!(routing.ask, vec![agents[1], agents[2]], "{:?} must ask the mentioned agents, the sleeping one included", moderator);
            assert_eq!(routing.reason, RequestReason::Mentioned, "{:?} must say they were mentioned", moderator);
            assert!(routing.audience == Audience::Everybody, "{:?} must deliver the end of the turn as every channel does", moderator);
            let said = moderator.route(&said_from(user, "@b and @c, please", vec![agents[1], agents[2]])).await;
            assert!(said.audience == Audience::Everybody, "{:?} must deliver the message as every channel does", moderator);
        }
    }

    #[tokio::test]
    async fn mentions_of_non_agents_fall_back_to_the_moderator_policy() {
        for moderator in answering_moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            let other_user = ActorId::User(UserId::new());
            let event = message_from(user, "@other", vec![other_user]);

            let routing = moderator.route(&event).await;

            assert!(!routing.ask.is_empty(), "{:?} must apply its own policy", moderator);
            assert!(!routing.ask.contains(&agents[1]), "{:?} must not ask the sleeping agent without a mention", moderator);
            assert_eq!(routing.reason, RequestReason::Chosen, "{:?} must say its policy chose them", moderator);
        }
    }

    /// Issue 437: a message to a person is addressed to them - it asks no
    /// agent the policy would choose -, unless only a mention of an agent
    /// replaces the policy; mentioned beside them, an agent is asked, and a
    /// mention of the message's own sender counts for nothing.
    #[tokio::test]
    async fn a_message_to_a_person_asks_nobody_unless_only_agents_replace_the_policy() {
        for moderator in answering_moderators() {
            let agents = add_agents(&moderator);
            let (user, other) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(StandIn::new(user, "test", ActorState::Idle)));
            moderator.add_actor(Arc::new(StandIn::new(other, "test", ActorState::Idle)));

            let to_her = moderator.route(&message_from(user, "@other, look", vec![other])).await;
            assert!(to_her.ask.is_empty(), "{:?} must ask nobody for a message to a person", moderator);
            let to_both = moderator.route(&message_from(user, "@other and @c, look", vec![other, agents[2]])).await;
            assert_eq!(to_both.ask, vec![agents[2]], "{:?} must ask the agent mentioned beside her", moderator);
            assert_eq!(to_both.reason, RequestReason::Mentioned);
            let to_himself = moderator.route(&message_from(user, "@user, note to self", vec![user])).await;
            assert!(!to_himself.ask.is_empty(), "{:?} must apply its policy to a mention of the sender alone", moderator);
        }
        for moderator in answering_moderators() {
            let moderator = moderator.with_mentions_replace_policy(MentionsReplacePolicy::Agents);
            let agents = add_agents(&moderator);
            let (user, other) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(StandIn::new(other, "test", ActorState::Idle)));

            let to_her = moderator.route(&message_from(user, "@other, look", vec![other])).await;
            assert!(!to_her.ask.is_empty() && !to_her.ask.contains(&agents[1]), "{:?} must apply its policy, with only agents replacing it", moderator);
            assert_eq!(to_her.reason, RequestReason::Chosen);
        }
    }

    #[test]
    fn whose_mentions_replace_the_policy_is_configured() {
        let of = |params| MentionsReplacePolicy::of_params(&params);
        assert_eq!(of(serde_json::json!({})).unwrap(), MentionsReplacePolicy::Anyone, "anyone's by default");
        assert_eq!(of(serde_json::json!({ "mentions_replace_policy": "agents" })).unwrap(), MentionsReplacePolicy::Agents);
        assert!(of(serde_json::json!({ "mentions_replace_policy": "people" })).is_err());
    }

    #[tokio::test]
    async fn every_moderator_asks_the_agents_an_agent_mentions_one_hop_further() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let event = message_from(agents[0], "@b, your turn", vec![agents[1]]);

            let routing = moderator.route(&event).await;

            assert_eq!(routing.ask, vec![agents[1]], "{:?} must ask the agent an agent mentioned", moderator);
            assert_eq!(routing.agent_hops, 1, "{:?} must count the hop", moderator);
        }
    }

    #[tokio::test]
    async fn every_moderator_asks_every_agent_but_the_sender_when_everyone_is_mentioned() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            // `@all`, as the channel resolves it (issue 401): every participant, the sender included.
            let event = message_from(agents[0], "@all, what do you think?", vec![user, agents[0], agents[1], agents[2]]);

            let routing = moderator.route(&event).await;

            assert_eq!(routing.ask, vec![agents[1], agents[2]], "{:?} must ask every other agent, the sleeping one included", moderator);
            assert_eq!(routing.reason, RequestReason::Mentioned, "{:?} must say they were mentioned, as everyone", moderator);
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
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), 2, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);
        let asked = |routing: &Routing| (routing.ask.len(), routing.agent_hops);

        let routing = moderator.route(&hops_from(agents[0], 1, Vec::new())).await;
        assert_eq!(asked(&routing), (1, 2), "the policy's choice is a hop, too");

        let routing = moderator.route(&hops_from(agents[0], 2, vec![agents[2]])).await;
        assert!(routing.ask.is_empty(), "beyond the limit nobody is asked, mentioned or not");
        assert!(routing.audience == Audience::Everybody, "but the end of the turn is delivered as ever");
        let said = moderator.route(&said_from(agents[0], "and you?", vec![agents[2]])).await;
        assert!(said.audience == Audience::Everybody, "and so is the message");

        let user = ActorId::User(UserId::new());
        let routing = moderator.route(&hops_from(user, 2, Vec::new())).await;
        assert_eq!(asked(&routing), (2, 0), "a user's message starts a new chain");
    }

    #[tokio::test]
    async fn a_chain_cut_off_is_told_to_the_users_and_the_agents_involved() {
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), 2, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        moderator.add_actor(Arc::new(StandIn::new(user, "test", ActorState::Idle)));
        let told = |routing: &Routing| routing.notices.iter()
            .map(|(recipients, notice)| (recipients.iter().copied().collect::<HashSet<_>>(), notice.clone()))
            .collect::<Vec<_>>();

        let routing = moderator.route(&hops_from(agents[0], 1, vec![agents[2]])).await;
        assert!(routing.notices.is_empty(), "below the limit, nothing is cut off");

        let event = hops_from(agents[0], 2, vec![agents[2]]);
        let ActorEvent::TurnFinished { messages, .. } = &event.event else { unreachable!() };
        let cut_off = Notice::ChainCutOff { message: messages[0].id, max_agent_hops: 2, not_asked: vec![agents[2]] };
        assert_eq!(told(&moderator.route(&event).await), [(HashSet::from([user, agents[0], agents[2]]), cut_off)],
            "the users, the sender and whom it mentioned - not agents[1] -, naming the message cut off");

        let event = hops_from(agents[0], 2, Vec::new());
        let ActorEvent::TurnFinished { messages, .. } = &event.event else { unreachable!() };
        let cut_off = Notice::ChainCutOff { message: messages[0].id, max_agent_hops: 2, not_asked: Vec::new() };
        assert_eq!(told(&moderator.route(&event).await), [(HashSet::from([user, agents[0]]), cut_off)], "told even when nobody was mentioned");
    }

    #[tokio::test]
    async fn the_limit_of_hops_is_a_parameter_of_the_moderator() {
        use crate::component::{ComponentContainer, ComponentContext};
        use crate::runtime::Services;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let store = Persistence::in_memory();
        let registry = Arc::new(test_registry(store.component_store().unwrap()));
        let context = ComponentContext::new(SecurityContext::default(), Services::empty());
        let configured = |max_agent_hops: Value| ComponentConfig { component_nodes: vec![ComponentDescriptor {
            params: serde_json::json!({ "max_agent_hops": max_agent_hops }),
            ..ComponentDescriptor::new("moderator", POLICY_MODERATOR_KIND)
        }] };

        let container = ComponentContainer::from_config(&configured(serde_json::json!(0)), ChannelLoop::COMPONENT_FACETS, &registry, &context).await.unwrap();
        let moderator = container.one::<dyn Moderator>().await.unwrap().facet;
        let agents = add_agents(moderator.as_ref());
        let asked = moderator.route(&message_from(agents[0], "@c", vec![agents[2]])).await.ask;
        assert!(asked.is_empty(), "with no hops allowed, agents answer only users");

        let invalid = ComponentContainer::from_config(&configured(serde_json::json!(-1)), ChannelLoop::COMPONENT_FACETS, &registry, &context).await;
        assert!(invalid.is_err(), "a negative limit is refused");
    }

    /// Issue 422: routing goes on without end only by asks, so the net
    /// against it is the moderator's - past it nobody is asked, mentioned
    /// or chosen, and what was said is delivered as ever.
    #[tokio::test]
    async fn an_event_as_deep_as_the_moderator_allows_is_delivered_and_nobody_asked() {
        let moderator = PolicyModerator::new(policy(broadcast_policy::BroadcastPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES).with_max_span_depth(5);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        let at_depth = |depth: i32, mentions: Vec<ActorId>| {
            let mut event = message_from(user, "@b, please", mentions);
            event.context.span_depth = depth;
            event
        };

        assert_eq!(moderator.route(&at_depth(4, vec![agents[1]])).await.ask, vec![agents[1]], "beneath the bound, as ever");
        let deep = moderator.route(&at_depth(5, vec![agents[1]])).await;
        assert!(deep.ask.is_empty(), "at the bound nobody is asked, mentioned or not");
        assert!(deep.audience == Audience::Everybody, "but what was said is delivered");
        assert!(moderator.route(&at_depth(5, Vec::new())).await.ask.is_empty(), "nor whom the policy would choose");
    }

    #[tokio::test]
    async fn the_bound_on_span_depth_is_a_parameter_of_the_moderator() {
        use crate::component::{ComponentContainer, ComponentContext};
        use crate::runtime::Services;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let store = Persistence::in_memory();
        let registry = Arc::new(test_registry(store.component_store().unwrap()));
        let context = ComponentContext::new(SecurityContext::default(), Services::empty());
        let configured = |max_span_depth: Value| ComponentConfig { component_nodes: vec![ComponentDescriptor {
            params: serde_json::json!({ "max_span_depth": max_span_depth }),
            ..ComponentDescriptor::new("moderator", POLICY_MODERATOR_KIND)
        }] };

        let container = ComponentContainer::from_config(&configured(serde_json::json!(0)), ChannelLoop::COMPONENT_FACETS, &registry, &context).await.unwrap();
        let moderator = container.one::<dyn Moderator>().await.unwrap().facet;
        let agents = add_agents(moderator.as_ref());
        let asked = moderator.route(&message_from(ActorId::User(UserId::new()), "@b", vec![agents[1]])).await.ask;
        assert!(asked.is_empty(), "with no depth allowed, nobody is asked");

        let invalid = ComponentContainer::from_config(&configured(serde_json::json!(-1)), ChannelLoop::COMPONENT_FACETS, &registry, &context).await;
        assert!(invalid.is_err(), "a negative bound is refused");
    }

    #[tokio::test]
    async fn mentions_only_asks_nobody_without_a_mention() {
        let moderator = PolicyModerator::new(policy(mentions_only_policy::MentionsOnlyPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());

        let asked = moderator.route(&message_from(user, "hello, everyone", Vec::new())).await.ask;
        assert!(asked.is_empty(), "nobody was mentioned, so nobody answers");
        let said = moderator.route(&said_from(user, "hello, everyone", Vec::new())).await;
        assert!(said.audience == Audience::Everybody, "the message is still delivered as every channel does");

        let asked = moderator.route(&message_from(agents[0], "my answer", Vec::new())).await.ask;
        assert!(asked.is_empty(), "nor does anybody answer an agent without a mention");
    }

    #[tokio::test]
    async fn user_driven_asks_every_agent_for_a_user_and_nobody_for_an_agent() {
        let moderator = PolicyModerator::new(policy(user_driven_policy::UserDrivenPolicy), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());

        let asked = moderator.route(&message_from(user, "hello, everyone", Vec::new())).await.ask;
        assert_eq!(asked.into_iter().collect::<HashSet<_>>(), HashSet::from([agents[0], agents[2]]), "every awake agent answers a user");

        let asked = moderator.route(&message_from(user, "@b", vec![agents[1]])).await.ask;
        assert_eq!(asked, vec![agents[1]], "a user's mention asks only the mentioned agent");

        let asked = moderator.route(&message_from(agents[0], "my answer", Vec::new())).await.ask;
        assert!(asked.is_empty(), "an agent's message asks nobody");
        let said = moderator.route(&said_from(agents[0], "my answer", Vec::new())).await;
        assert!(said.audience == Audience::Everybody, "but is delivered as every channel does");
    }

    #[tokio::test]
    async fn round_robin_takes_turns_and_a_mention_does_not_advance_it() {
        let moderator = PolicyModerator::new(policy(round_robin_policy::RoundRobinPolicy::default()), DEFAULT_MAX_AGENT_HOPS, DEFAULT_CONTEXT_MESSAGES);
        let agents = add_agents(&moderator);
        let user = ActorId::User(UserId::new());
        let mut awake = [agents[0], agents[2]];
        awake.sort_unstable_by_key(|id| id.id());

        let first = moderator.route(&message_from(user, "one", Vec::new())).await.ask;
        let mentioned = moderator.route(&message_from(user, "@b", vec![agents[1]])).await.ask;
        let second = moderator.route(&message_from(user, "two", Vec::new())).await.ask;
        let third = moderator.route(&message_from(user, "three", Vec::new())).await.ask;

        assert_eq!(first, vec![awake[0]]);
        assert_eq!(mentioned, vec![agents[1]]);
        assert_eq!(second, vec![awake[1]], "the mention must not have taken a turn");
        assert_eq!(third, vec![awake[0]]);
    }

    /// What does not ask anyone to respond is delivered as every channel
    /// delivers it (issue 418): the moderator decides nothing of it.
    #[tokio::test]
    async fn every_moderator_lets_what_asks_nobody_go_where_every_channel_delivers_it() {
        let call = ToolCall { id: None, function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: Default::default(), presentation: Default::default() };
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            moderator.add_actor(Arc::new(StandIn::new(user, "test", ActorState::Idle)));
            let turn_id = TurnId::new();
            let in_turn = Message { turn_id: Some(turn_id), ..text_from(agents[0], "@c?", vec![agents[2]]) };
            let events = [
                (agents[0], ActorEvent::CompleteMessageGenerated(in_turn)),
                (agents[0], ActorEvent::TurnGivenUp { turn_id, reason: GivenUpReason::Failed { message: "boom".to_string() } }),
                (agents[0], ActorEvent::ActivityStarted { turn_id, activity: "compacting".to_string() }),
                (agents[0], ActorEvent::PlanStepStarted { turn_id, step_index: 0, total_steps: 2, description: "first".to_string() }),
                (user, ActorEvent::CommandExecuted { command: "wakeup".to_string(), args: String::new(), output: "Woke b.".to_string(), to: None, turn_id: None }),
                (agents[0], ActorEvent::CommandStarted { command: "compact".to_string(), args: String::new(), to: user, turn_id }),
                (user, ActorEvent::ParticipantJoined { participant: user, name: "user".to_string() }),
                (agents[0], ActorEvent::ApprovalRequested(ApprovalRequest {
                    suggested: None, explicit: false, id: Default::default(), session_id: Default::default(), channel_id: Default::default(),
                    agent_id: AgentId::new(), tool_call: call.clone(), description: "test".to_string(),
                })),
                (user, ActorEvent::MessageRevoked { message: MessageId::new(), at: Utc::now() }),
            ];
            for (sender, event) in events {
                let kind = event.kind();
                assert!(as_every_channel(&moderator.route(&envelope(sender, event)).await), "{:?}: {kind}", moderator);
            }
        }
    }

    #[tokio::test]
    async fn every_moderator_asks_whom_a_nudge_names_alone() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let (user, other) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
            moderator.add_actor(Arc::new(StandIn::new(user, "test", ActorState::Idle)));
            moderator.add_actor(Arc::new(StandIn::new(other, "test", ActorState::Idle)));

            // The sleeping one, too: the channel wakes whom a decision asks;
            // and a user as well as an agent.
            for nudged in [agents[1], other] {
                let routing = moderator.route(&envelope(user, ActorEvent::NudgeRequested { participant: nudged })).await;
                assert_eq!(routing.ask, vec![nudged], "{:?} must ask whom the nudge names, and no policy", moderator);
                assert_eq!(routing.reason, RequestReason::Nudged, "{:?} must say they were nudged", moderator);
                assert!(routing.audience == Audience::Everybody && routing.notices.is_empty(), "{:?} must tell nobody else", moderator);
            }
            let stranger = ActorId::Agent(AgentId::new());
            let routing = moderator.route(&envelope(user, ActorEvent::NudgeRequested { participant: stranger })).await;
            assert!(routing.ask.is_empty(), "{:?} must ask nobody it does not know", moderator);
        }
    }

    #[tokio::test]
    async fn a_message_of_no_turn_is_answered_by_itself_and_one_of_a_turn_with_the_turn_s_end() {
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let user = ActorId::User(UserId::new());
            let mention = || vec![agents[2]];

            // Whoever said it - a user, or an agent speaking unasked - a
            // message of no turn asks whom it mentions (issue 258).
            for sender in [user, agents[0]] {
                let asked = moderator.route(&said_from(sender, "@c?", mention())).await.ask;
                assert_eq!(asked, vec![agents[2]], "{:?}: {sender:?} said it outside of a turn", moderator);
            }

            // Within a turn it asks nobody yet: the turn's end does.
            let turn = TurnId::new();
            let in_turn = Message { turn_id: Some(turn), ..text_from(agents[0], "@c?", mention()) };
            let asked = moderator.route(&envelope(agents[0], ActorEvent::CompleteMessageGenerated(in_turn.clone()))).await.ask;
            assert!(asked.is_empty(), "{:?}: not before the turn ended", moderator);
            let asked = moderator.route(&envelope(agents[0], ActorEvent::TurnFinished { turn_id: turn, messages: vec![in_turn] })).await.ask;
            assert_eq!(asked, vec![agents[2]], "{:?}: at its end", moderator);
        }
    }

    #[tokio::test]
    async fn an_answer_of_nothing_to_hear_asks_nobody() {
        use caelumora_model::tool::{ToolCallResult, ToolVisibility};
        let classified = Classification::CHANNEL_DEFAULT;
        let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
        let result = ToolCallResult::of(&call, Default::default(), "done".to_string(), classified.clone());
        for moderator in moderators() {
            let agents = add_agents(&moderator);
            let sender = agents[0];
            let ended = |messages: Vec<Message>| envelope(sender, ActorEvent::TurnFinished { turn_id: TurnId::new(), messages });
            let calling = Message { sender_id: sender, role: MessageRole::Assistant, content: None, tool_calls: vec![call.clone()], mentions: vec![agents[2]], ..Message::new(classified.clone()) };
            let tool = Message { sender_id: sender, role: MessageRole::Tool, content: Some("done".to_string()), tool_results: vec![result.clone()], ..Message::new(classified.clone()) };

            // A response of tool calls only, or a turn cancelled before it
            // said anything: nobody is asked.
            for nothing in [vec![calling.clone(), tool.clone()], Vec::new()] {
                assert!(as_every_channel(&moderator.route(&ended(nothing)).await), "{:?}: nobody is asked", moderator);
            }
        }
    }
}
