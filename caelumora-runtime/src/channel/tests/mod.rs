//! The tests of `ChannelLoop` itself, and the fixtures of every test of a running channel.

use super::*;
use crate::actor::Actor;
use crate::component::{ComponentImpl, ComponentRegistry};
use crate::backend::Backends;
use crate::runtime::{ChannelRuntime, RuntimeBuilder, RuntimeManager};
use crate::testing::{test_registry, Script, ScriptedAgent, ScriptedAgents};
use crate::transport::{EventSender, EventReceiver};
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::channel::{Channel, ChannelId, Participant, moderation_components};
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext, AccessPolicy};
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use caelumora_model::value::name::Name;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::command::help_command::HelpCommandProvider;
use crate::command::wakeup_command::WakeupCommandProvider;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use caelumora_model::agent::AgentId;
use caelumora_model::component::{ComponentConfig, ComponentFailPolicy};
use caelumora_model::confidentiality::{ConfidentialityRules, Classification};
use caelumora_model::config::AppConfig;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::AllowAllPolicy;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::user::User;
use crate::moderator::Moderator;
use std::any::Any;

mod commands;

pub(super) use crate::session::testing::user;
pub(super) use crate::channel::testing::*;

/// A channel of `users`, who are recorded in the store its actors are
/// read from.
pub(super) async fn channel_with(users: &[&User], access_policy: Arc<dyn AccessPolicy>) -> Arc<ChannelLoop> {
    let store = Arc::new(MemoryStore::new());
    let mut participants = Vec::new();
    for user in users {
        store.entity_stores().users.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    channel_of(store, participants, access_policy).await
}

/// A channel of `participants`, whose actors are read from `store`.
async fn channel_of(store: Arc<MemoryStore>, participants: Vec<Participant>, access_policy: Arc<dyn AccessPolicy>) -> Arc<ChannelLoop> {
    let registry = test_registry(Arc::<MemoryStore>::clone(&store));
    channel_built(store, participants, access_policy, registry, moderation_components(None, Some("broadcast_policy"))).await
}

/// A channel of `participants` with `components`, built by `registry`.
async fn channel_built(store: Arc<MemoryStore>, participants: Vec<Participant>, access_policy: Arc<dyn AccessPolicy>, registry: ComponentRegistry, components: ComponentConfig) -> Arc<ChannelLoop> {
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&access_policy)));
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), registry, &[&ScriptedAgents(Script::Answers("ok".to_string()))]).unwrap());
    let channel = Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId::new(),
        session_id: SessionId::new(),
        name: Name::new("main").unwrap(),
        components,
        participants,
        security_attributes: SecurityAttributes::default(),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        deleted: None,
        former: Vec::new(),
    };
    let builtin = |id: &str, facet: Arc<dyn ChannelCommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
    let executor = CommandExecutor::new(
        Arc::clone(runtime.access_policy()),
        vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
    ).unwrap();
    let session_id = channel.session_id;
    ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await.unwrap()
}

#[tokio::test]
async fn a_participant_that_no_longer_exists_is_left_out() {
    let alice = user("alice");
    let store = Arc::new(MemoryStore::new());
    store.entity_stores().users.create(&SecurityContext::system(), &alice).await.unwrap();
    let (gone_user, gone_agent) = (UserId::new(), AgentId::new());

    let channel = channel_of(
        store,
        vec![Participant::User(alice.id), Participant::User(gone_user), Participant::Agent(gone_agent)],
        Arc::new(AllowAllPolicy::new()),
    ).await;

    assert!(channel.user_actor(alice.id).is_some(), "the others take part");
    assert!(channel.actor(ActorId::User(gone_user)).is_none(), "a deleted user has no actor");
    assert!(channel.actor(ActorId::Agent(gone_agent)).is_none(), "nor has a deleted agent");
}

#[tokio::test]
async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
    let alice = user("alice");
    let bob = user("bob");
    let channel = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;

    for participant in [&alice, &bob] {
        let actor = channel.user_actor(participant.id).expect("a participant has an actor");
        assert_eq!(actor.id(), ActorId::User(participant.id));
        assert_eq!(actor.name(), participant.username.as_str(), "the actor writes in this user's name");
    }
    assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
}




/// Answers every event with the same decision: tell `to` what was said,
/// and ask it to respond.
#[derive(Debug)]
pub(super) struct AskBack(pub(super) ActorId);

#[async_trait::async_trait]
impl Moderator for AskBack {
    async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        vec![(self.0, event.clone()), (self.0, event.forward(ActorEvent::ResponseRequested))]
    }
    fn add_actor(&self, _: Arc<dyn Actor>) {}
    fn remove_actor(&self, _: ActorId) {}
}

/// A channel of `users` whose moderator is `moderator`: a component of a
/// kind of its own (issue 174).
pub(super) async fn channel_moderated_by(users: &[&User], moderator: Arc<dyn Moderator>) -> Arc<ChannelLoop> {
    use crate::component::{ComponentFacets, ComponentKind};
    let store = Arc::new(MemoryStore::new());
    let mut participants = Vec::new();
    for user in users {
        store.entity_stores().users.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
    registry.register(ComponentKind::new("test_moderator").provides::<dyn Moderator>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn Moderator>(Arc::clone(&moderator));
        Ok(facets)
    });
    let components = moderation_components(Some("test_moderator"), None);
    channel_built(store, participants, Arc::new(AllowAllPolicy::new()), registry, components).await
}

/// An agent asleep that only tells whether it was woken or cancelled.
#[derive(Debug)]
pub(super) struct Sleeper {
    pub(super) id: ActorId,
    name: &'static str,
    pub(super) woken: AtomicBool,
    pub(super) cancelled: AtomicBool,
}

#[async_trait::async_trait]
impl Actor for Sleeper {
    fn id(&self) -> ActorId { self.id }
    fn name(&self) -> Name { Name::new(self.name).expect("a name") }
    fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
    async fn run(&self, _rx: Box<dyn EventReceiver>, _tx: Box<dyn EventSender>) -> Result<()> { Ok(()) }
    fn state(&self) -> ActorState {
        if self.woken.load(Ordering::SeqCst) { ActorState::Idle } else { ActorState::Sleeping }
    }
    fn wakeup(&self) { self.woken.store(true, Ordering::SeqCst); }
    fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); }
    fn stop(&self) {}
}

pub(super) fn sleeper() -> Arc<Sleeper> {
    sleeper_named("sleeper")
}

fn sleeper_named(name: &'static str) -> Arc<Sleeper> {
    Arc::new(Sleeper { id: ActorId::Agent(AgentId::new()), name, woken: AtomicBool::new(false), cancelled: AtomicBool::new(false) })
}

/// A channel of alice's with the sleeping agents `names`, as approvers -
/// who they are does not matter to finding them.
pub(super) async fn channel_with_sleepers(names: &[&'static str]) -> (Arc<ChannelLoop>, UserId, Vec<Arc<Sleeper>>) {
    let alice = user("alice");
    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let sleepers: Vec<_> = names.iter().map(|name| sleeper_named(name)).collect();
    for sleeper in &sleepers {
        channel.adopt(Arc::<Sleeper>::clone(sleeper), ResolvedParticipant::approver(channel.session_id, channel.id)).await;
    }
    (channel, alice.id, sleepers)
}

/// A running channel with alice and a scripted agent in a turn that does
/// not end by itself (`Script::Hangs`): what the channel does to an agent
/// it cancels, asks again or removes, whatever the agent does in its turns.
pub(super) async fn stuck_in_a_turn() -> Stuck {
    let stuck = start(scripted_channel(Script::Hangs).await).await;
    settled(&stuck.helper, ActorState::Planning).await;
    stuck
}

/// A channel of alice and an agent whose answers follow `script`
/// (`ScriptedAgents`, issue 370), built but not running: for what the
/// channel does with an agent among its participants, whatever the agent
/// does in its turns.
pub(super) async fn scripted_channel(script: Script) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    let store = Arc::new(MemoryStore::new());
    let (context, agent, alice) = alice_and_an_agent(&store, ComponentConfig::default()).await;
    let access_policy: Arc<dyn AccessPolicy> = Arc::new(AllowAllPolicy::new());
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&access_policy)));
    let mut builder = RuntimeBuilder::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&store)), Backends::built_in()).unwrap();
    builder.plugin(&ScriptedAgents(script)).unwrap();
    (channel_of_alice_and(Arc::new(builder.build().unwrap()), alice, agent).await, context, agent, alice)
}



/// The scripted agent `actor` is (`ScriptedAgents`, issue 370).
pub(super) fn scripted(actor: &Arc<dyn Actor>) -> &ScriptedAgent {
    let any: &dyn Any = actor.as_ref();
    any.downcast_ref::<ScriptedAgent>().expect("a scripted agent")
}

