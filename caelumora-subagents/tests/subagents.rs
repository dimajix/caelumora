//! Sub-agents through the whole kernel (issue 558): the plugin and the library joined to the world
//! of the security suite (`caelumora_kernel::testing`), an agent starting a sub-agent by its tool,
//! given back what it answered, asking it again and ending it through the writer bound to its work.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::testing::{agent_runtime, calling, register_scripted};
use caelumora_agents::tool::ToolProvider;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{Agent, Personality, PersonalityId, PersonalityKind};
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::query::Query;
use caelumora_domain::scope::Scope;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::ScopedName;
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::component::testing::{built, descriptor};
use caelumora_kernel::testing::{participant_runtime_in, World};
use caelumora_subagents::component::{START_SUBAGENT, SUBAGENTS_KIND};
use caelumora_subagents::extension::{SubAgentError, SubAgentWriter};
use caelumora_subagents::library::SubagentsLibrary;
use caelumora_subagents::plugin::SubagentsPlugin;
use std::sync::Arc;

fn said(text: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        ..Message::new(Classification::general(Confidentiality::Internal))
    }
}

/// The world with sub-agents, whose model "leading" starts a researcher, and whose model
/// "researching" answers what it is given.
async fn world() -> World {
    let start = calling(
        START_SUBAGENT,
        serde_json::json!({ "personality": "researcher", "brief": "Find it." }),
    );
    World::with(&[&SubagentsPlugin], &[&AgentsLibrary, &SubagentsLibrary], |registry| {
        let leading = vec![start, said("started"), said("thanks"), said("thanks again")];
        register_scripted(registry, "leading", leading);
        register_scripted(registry, "researching", vec![said("found it"), said("found more")]);
    })
    .await
}

/// The personality `name` of `owner`, with `components` and of `kind`.
async fn personality(
    world: &World,
    name: &str,
    owner: UserId,
    kind: PersonalityKind,
    components: Vec<ComponentDescriptor>,
) -> Personality {
    let mut personality = Personality {
        kind,
        components: ComponentConfig { component_nodes: components },
        ..world.personality(name, Some(owner), &[]).await
    };
    let personalities = &world.kernel.entity_stores().personalities;
    personalities.update(&World::system(), &mut personality).await.unwrap();
    personality
}

/// What the channel recorded, once `done` holds of it.
async fn recorded(
    channel: &ChannelLoop,
    done: impl Fn(&HistoryEntry) -> bool,
) -> Vec<HistoryEntry> {
    channel.history_once(|entries| entries.iter().any(&done)).await
}

/// Issue 558: an agent starts a sub-agent of a personality its tool lists - a new agent, named by
/// an adjective and the personality, in a thread of its own at the call -, is given back to read
/// what it answered, asks it again through the same writer and is given that back too, and ends
/// it: let go, its thread closed, the agent deleted. A personality of an external agent is no
/// sub-agent's, and a thread without one of the work's is none to ask.
#[tokio::test]
async fn an_agent_starts_a_sub_agent_is_given_back_its_answer_asks_it_again_and_ends_it() {
    let world = world().await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let alice = world.alice;
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", alice, internal, vec![model("researching")]).await;
    let external = PersonalityKind::External;
    personality(&world, "remote", alice, external, vec![model("researching")]).await;
    let subagents = ComponentDescriptor {
        params: serde_json::json!({ "personalities": ["researcher"] }),
        ..ComponentDescriptor::new("subagents", SUBAGENTS_KIND)
    };
    let leading =
        personality(&world, "leading", alice, internal, vec![model("leading"), subagents]).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let other = world.agent("other", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead, other], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);

    main.user_actor(world.alice)
        .expect("alice takes part")
        .send_message(Some("@lead find it".to_string()), None, Vec::new())
        .await
        .expect("said");
    let given_back = |text: &'static str| {
        move |e: &HistoryEntry| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == lead && message.content.as_deref() == Some(text))
        }
    };
    let entries = recorded(&main, given_back("found it")).await;
    let thread = entries
        .iter()
        .find_map(|e| match e {
            HistoryEntry::ThreadOpened { thread, by, .. } if *by == lead => Some(*thread),
            _ => None,
        })
        .expect("opened by the lead");
    let held = main.delegation(thread).await.expect("delegated into");
    let sub_agent = held.recipient.as_agent_id().expect("an agent");
    let stored: Agent =
        world.entity_store::<Agent>().get(&World::system(), sub_agent).await.unwrap();
    let name = stored.name.name.to_string();
    assert!(name.ends_with("-researcher"), "an adjective and the personality: {name}");
    assert_eq!(held.giver, lead, "given by the lead");

    let runtime = agent_runtime(participant_runtime_in(&main, lead));
    let internal = Classification::general(Confidentiality::Internal);
    let alice = ActorId::User(world.alice);
    let writer = runtime.writer::<dyn SubAgentWriter>(alice, internal).await.expect("bound");
    writer.ask(thread, "And more?".to_string()).await.expect("asked again");
    recorded(&main, given_back("found more")).await;

    let origin = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let refused = writer.start("remote", origin, "Find it.".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::External(_)), "{refused}");
    let refused = writer.start("nobody", origin, "Find it.".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::UnknownPersonality(_)), "{refused}");
    let elsewhere = ThreadId::new();
    let refused = writer.ask(elsewhere, "Hello?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "{refused}");
    let others = agent_runtime(participant_runtime_in(&main, ActorId::Agent(other)));
    let internal = Classification::general(Confidentiality::Internal);
    let theirs = others.writer::<dyn SubAgentWriter>(alice, internal).await.expect("bound");
    let refused = theirs.ask(thread, "Mine now?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "not another's to ask: {refused}");
    let refused = theirs.end(thread).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "nor to end: {refused}");

    writer.end(thread).await.expect("ended");
    recorded(&main, |e| matches!(e, HistoryEntry::ThreadClosed { thread: t, .. } if *t == thread))
        .await;
    assert!(!main.channel().lists(ActorId::Agent(sub_agent)), "let go");
    let agents = world.entity_store::<Agent>();
    agents.get_deleted(&World::system(), sub_agent).await.expect("deleted, not purged");
    let refused = writer.ask(thread, "Still there?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "nothing more to ask: {refused}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 558: the agent of a participant who does not own the session starts a sub-agent - made by
/// its owner, founded into the thread and delegated to in the kernel's name -, without the right to
/// change the session or the channel.
#[tokio::test]
async fn an_agent_of_one_who_does_not_own_the_session_starts_a_sub_agent() {
    let world = world().await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let bob = world.bob;
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", bob, internal, vec![model("researching")]).await;
    let leading = personality(&world, "leading", bob, internal, vec![model("leading")]).await;
    let lead = world.agent("bobs-lead", world.bob, &leading).await;
    let session = world.shared_session(&[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);

    let runtime = agent_runtime(participant_runtime_in(&main, lead));
    let internal = Classification::general(Confidentiality::Internal);
    let bob = ActorId::User(world.bob);
    let writer = runtime.writer::<dyn SubAgentWriter>(bob, internal).await.expect("bound");
    let origin = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let (started, asked) =
        world.asked(writer.start("researcher", origin, "Find it.".to_string())).await;
    started.expect("started");
    for line in
        ["bob as bobs-lead: CreateAgent(bob's) yes", "bob as bobs-lead: OpenThread(alice's) yes"]
    {
        assert!(asked.contains(&line.to_string()), "{line}: {asked:?}");
    }
    assert!(
        !asked.iter().any(|line| line.contains(": Update")),
        "neither the session nor the channel is changed in bob's name: {asked:?}"
    );

    let thread = main
        .history_entries()
        .await
        .iter()
        .find_map(|e| match e {
            HistoryEntry::ThreadOpened { thread, .. } => Some(*thread),
            _ => None,
        })
        .expect("opened");
    let (ended, asked) = world.asked(writer.end(thread)).await;
    ended.expect("ended by bob's agent");
    for asks in [": LeaveSession(", ": DeleteAgent("] {
        assert!(asked.iter().any(|line| line.contains(asks)), "{asks}: {asked:?}");
    }
    assert!(
        !asked.iter().any(|line| line.contains(": Update")),
        "ended without changing the session or the channel in bob's name: {asked:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 558: the personality of a sub-agent is found as the kernel finds one by name from the
/// work's channel - the innermost its owner may read, one of the channel's over a global one; one
/// they may not read is none.
#[tokio::test]
async fn a_personality_is_found_from_the_channel_innermost_first() {
    let world = world().await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let internal = PersonalityKind::Internal;
    let outer =
        personality(&world, "researcher", world.alice, internal, vec![model("researching")]).await;
    personality(&world, "secret", world.bob, internal, vec![model("researching")]).await;
    let leading =
        personality(&world, "leading", world.alice, internal, vec![model("leading")]).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);
    let place = Scope::ChannelScope { session_id: session.id(), channel_id: main.id() };
    let mut inner = Personality {
        id: PersonalityId::new(),
        name: ScopedName::new("researcher", place).unwrap(),
        ..outer
    };
    let personalities = &world.kernel.entity_stores().personalities;
    personalities.create(&World::system(), &mut inner).await.unwrap();

    let runtime = agent_runtime(participant_runtime_in(&main, lead));
    let internal = Classification::general(Confidentiality::Internal);
    let alice = ActorId::User(world.alice);
    let writer = runtime.writer::<dyn SubAgentWriter>(alice, internal).await.expect("bound");
    let origin = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let refused = writer.start("secret", origin, "Find it.".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::UnknownPersonality(_)), "not hers: {refused}");
    let thread = writer.start("researcher", origin, "Find it.".to_string()).await.expect("started");
    let sub_agent = main.delegation(thread).await.and_then(|held| held.recipient.as_agent_id());
    let agents = world.entity_store::<Agent>();
    let stored = agents.get(&World::system(), sub_agent.expect("started")).await.unwrap();
    assert_eq!(stored.personality_id, inner.id, "the channel's researcher, not the global one");

    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 593: starting a sub-agent is one procedure, undone if a step fails - here opening its
/// thread, at a thread that is not open, after its agent was made: no agent of it is left.
#[tokio::test]
async fn a_start_that_fails_leaves_no_agent() {
    let world = world().await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", world.alice, internal, vec![model("researching")]).await;
    let leading =
        personality(&world, "leading", world.alice, internal, vec![model("leading")]).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let runtime = agent_runtime(participant_runtime_in(&main, ActorId::Agent(lead)));
    let classification = Classification::general(Confidentiality::Internal);
    let alice = ActorId::User(world.alice);
    let writer = runtime.writer::<dyn SubAgentWriter>(alice, classification).await.expect("bound");
    let agents = || async {
        let listed = world.entity_store::<Agent>().list(&World::system(), &Query::all()).await;
        listed.unwrap().items.len()
    };
    let before = agents().await;

    let nowhere = ThreadOrigin { thread: ThreadId::new(), message: MessageId::new() };
    writer.start("researcher", nowhere, "Find it.".to_string()).await.expect_err("not open");
    assert_eq!(agents().await, before, "the agent it made is deleted again");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 558: starting a sub-agent asks for approval only where the component is configured so;
/// asking and ending one never do; a component without a personality to start is no component.
#[tokio::test]
async fn starting_a_sub_agent_asks_for_approval_as_configured() {
    let world = world().await;
    let tools = async |params: serde_json::Value| {
        built::<dyn ToolProvider>(&world.kernel, descriptor("subagents", SUBAGENTS_KIND, params))
            .await
    };
    let asks = async |mode: serde_json::Value| {
        let params =
            serde_json::json!({ "personalities": ["researcher"], "requires_approval": mode });
        let provider = tools(params).await.unwrap();
        provider.tools().iter().map(|tool| tool.description().requires_approval).collect::<Vec<_>>()
    };
    assert_eq!(asks(serde_json::json!(true)).await, [true, false, false], "start alone");
    assert_eq!(asks(serde_json::json!("unless_read_only")).await, [true, false, false]);
    assert_eq!(asks(serde_json::json!(false)).await, [false, false, false]);
    let unconfigured = tools(serde_json::json!({ "personalities": ["researcher"] })).await.unwrap();
    assert!(!unconfigured.tools()[0].description().requires_approval, "unless configured");
    assert!(tools(serde_json::json!({ "personalities": [] })).await.is_err(), "one to start");
}
