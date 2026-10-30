//! The tests of `ChannelLoop` itself, and the fixtures of every test of a running channel.

mod commands;

pub(super) use crate::channel::testing::*;

use super::*;
use crate::actor::Actor;
use crate::command::help_command::HelpCommandProvider;
use crate::command::wakeup_command::WakeupCommandProvider;
use crate::command::{ChannelCommandProvider, CommandExecutor};
use crate::component::{ComponentImpl, ComponentRegistry};
use crate::moderator::{Moderator, Routing};
use crate::runtime::ChannelRuntime;
use crate::testing::{Script, ScriptedAgent, ScriptedAgents, StandIn, TestRuntime};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, RequestReason};
use caelumora_model::agent::testing::make_personality;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::testing::founded;
use caelumora_model::channel::testing::make_channel;
use caelumora_model::channel::{moderation_components, Channel, Participant};
use caelumora_model::component::{ComponentConfig, ComponentFailPolicy};
use caelumora_model::history::Audience;
use caelumora_model::security::{
    AccessPolicy, ActionType, AllowAllPolicy, Policies, SecurityContext,
};
use caelumora_model::session::testing::bare_session;
use caelumora_model::session::SessionId;
use caelumora_model::store::EntityStores;
use caelumora_model::store::Persistence;
use caelumora_model::user::testing::make_user;
use caelumora_model::user::testing::record_users;
use caelumora_model::user::User;
use caelumora_model::user::UserId;
use std::any::Any;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

/// A channel of `users`, who are recorded in the store its actors are
/// read from.
pub(super) async fn channel_with(
    users: &[&User],
    access_policy: Arc<dyn AccessPolicy>,
) -> Arc<ChannelLoop> {
    let store = Persistence::in_memory();
    let participants =
        record_users(&EntityStores::open(&store, &Policies::permitting_all()).unwrap(), users)
            .await;
    channel_of(store, participants, access_policy).await
}

/// A channel of `participants`, whose actors are read from `store`.
async fn channel_of(
    store: Persistence,
    participants: Vec<Participant>,
    access_policy: Arc<dyn AccessPolicy>,
) -> Arc<ChannelLoop> {
    channel_built(
        store,
        participants,
        access_policy,
        |_| {},
        moderation_components(None, Some("broadcast_policy")),
    )
    .await
}

/// A channel of `participants` with `components`, of the kinds `register` adds besides the
/// built-in ones.
async fn channel_built(
    store: Persistence,
    participants: Vec<Participant>,
    access_policy: Arc<dyn AccessPolicy>,
    register: impl FnOnce(&mut ComponentRegistry),
    components: ComponentConfig,
) -> Arc<ChannelLoop> {
    let runtime =
        TestRuntime { store, access_policy, components: Box::new(register), ..Default::default() }
            .build();
    let channel = Channel { components, participants, ..make_channel(SessionId::new(), "main") };
    let builtin = |id: &str, facet: Arc<dyn ChannelCommandProvider>| ComponentImpl {
        id: id.to_string(),
        fail_policy: ComponentFailPolicy::FailOpen,
        facet,
    };
    let executor = CommandExecutor::new(
        Arc::clone(runtime.access_policy()),
        vec![
            builtin("help", Arc::new(HelpCommandProvider)),
            builtin("wakeup", Arc::new(WakeupCommandProvider)),
        ],
    )
    .unwrap();
    // Its participants' joins recorded, as if they had founded it (issue 420).
    runtime.state_store().put_channel_state(channel.id, &mut founded(&channel)).await.unwrap();
    let session =
        runtime.controller_manager().new_session_controller(bare_session(channel.session_id));
    let channel = runtime.controller_manager().new_channel_controller(channel);
    ChannelLoop::new(ChannelRuntime::new(
        runtime,
        Arc::new(executor),
        session,
        channel,
        Weak::new(),
    ))
    .await
    .unwrap()
}

#[tokio::test]
async fn a_participant_that_no_longer_exists_is_left_out() {
    let mut alice = make_user("alice");
    let store = Persistence::in_memory();
    EntityStores::open(&store, &Policies::permitting_all())
        .unwrap()
        .users
        .create(&SecurityContext::system(), &mut alice)
        .await
        .unwrap();
    let (gone_user, gone_agent) = (UserId::new(), AgentId::new());

    let channel = channel_of(
        store,
        vec![
            Participant::User(alice.id),
            Participant::User(gone_user),
            Participant::Agent(gone_agent),
        ],
        Arc::new(AllowAllPolicy::new()),
    )
    .await;

    assert!(channel.user_actor(alice.id).is_some(), "the others take part");
    assert!(channel.actor(ActorId::User(gone_user)).is_none(), "a deleted user has no actor");
    assert!(channel.actor(ActorId::Agent(gone_agent)).is_none(), "nor has a deleted agent");
}

#[tokio::test]
async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
    let alice = make_user("alice");
    let bob = make_user("bob");
    let channel = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;

    for participant in [&alice, &bob] {
        let actor = channel.user_actor(participant.id).expect("a participant has an actor");
        assert_eq!(actor.id(), ActorId::User(participant.id));
        assert_eq!(
            actor.name(),
            participant.username.as_str(),
            "the actor writes in this user's name"
        );
    }
    assert!(
        channel.user_actor(UserId::new()).is_none(),
        "someone who is not a participant has no actor"
    );
}

/// Answers every event with the same decision: tell `to` what was said,
/// and ask it to respond.
#[derive(Debug)]
pub(super) struct AskBack(pub(super) ActorId);

#[async_trait::async_trait]
impl Moderator for AskBack {
    async fn route(&self, event: &Envelope) -> Routing {
        Routing {
            audience: Audience::Only(vec![self.0]),
            ask: vec![self.0],
            reason: RequestReason::Mentioned,
            agent_hops: event.context.agent_hops,
            ..Routing::default()
        }
    }
    fn add_actor(&self, _: Arc<dyn Actor>) {}
    fn remove_actor(&self, _: ActorId) {}
}

/// A channel of `users` whose moderator is `moderator`: a component of a
/// kind of its own (issue 174).
pub(super) async fn channel_moderated_by(
    users: &[&User],
    moderator: Arc<dyn Moderator>,
) -> Arc<ChannelLoop> {
    use crate::component::{ComponentFacets, ComponentKind};
    let store = Persistence::in_memory();
    let participants =
        record_users(&EntityStores::open(&store, &Policies::permitting_all()).unwrap(), users)
            .await;
    let register = move |registry: &mut ComponentRegistry| {
        registry.register(
            ComponentKind::new("test_moderator").provides::<dyn Moderator>(),
            move |_, _| {
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn Moderator>(Arc::clone(&moderator));
                Ok(facets)
            },
        );
    };
    let components = moderation_components(Some("test_moderator"), None);
    channel_built(store, participants, Arc::new(AllowAllPolicy::new()), register, components).await
}

/// An agent asleep (`StandIn`), that tells whether it was woken or
/// cancelled.
pub(super) fn sleeper() -> Arc<StandIn> {
    sleeper_named("sleeper")
}

fn sleeper_named(name: &str) -> Arc<StandIn> {
    Arc::new(StandIn::sleeping(ActorId::Agent(AgentId::new()), name))
}

/// A channel of alice's with the sleeping agents `names`, as approvers -
/// who they are does not matter to finding them.
pub(super) async fn channel_with_sleepers(
    names: &[&str],
) -> (Arc<ChannelLoop>, UserId, Vec<Arc<StandIn>>) {
    let alice = make_user("alice");
    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let sleepers: Vec<_> = names.iter().map(|name| sleeper_named(name)).collect();
    for sleeper in &sleepers {
        channel
            .adopt(
                Arc::<StandIn>::clone(sleeper),
                ResolvedParticipant::approver(channel.session_id, channel.id),
                None,
                None,
            )
            .await;
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
pub(super) async fn scripted_channel(
    script: Script,
) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    let store = Persistence::in_memory();
    let (context, agent, alice) =
        alice_and_an_agent(&store, "helper", make_personality("plain")).await;
    let runtime =
        TestRuntime { store, plugins: vec![&ScriptedAgents(script)], ..Default::default() }.build();
    (channel_of_alice_and(runtime, alice, agent).await, context, agent, alice)
}

/// The scripted agent `actor` is (`ScriptedAgents`, issue 370).
pub(super) fn scripted(actor: &Arc<dyn Actor>) -> &ScriptedAgent {
    let any: &dyn Any = actor.as_ref();
    any.downcast_ref::<ScriptedAgent>().expect("a scripted agent")
}
