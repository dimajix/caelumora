//! Sub-agents through the whole kernel (issue 558): the plugin and the library joined to the world
//! of the security suite (`caelumora_kernel::testing`), an agent starting a sub-agent by its tool,
//! given back what it answered, asking it again and ending it through the writer bound to its work.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::testing::{
    agent_runtime, calling, register_answering, register_scripted, Answer,
};
use caelumora_agents::tool::ToolProvider;
use caelumora_api::library::Library;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{Agent, Personality, PersonalityId, PersonalityKind};
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::config::AppConfig;
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::query::Query;
use caelumora_domain::role::RoleId;
use caelumora_domain::scope::Scope;
use caelumora_domain::user::UserId;
use caelumora_domain::value::name::ScopedName;
use caelumora_kernel::channel::ChannelLoop;
use caelumora_kernel::component::testing::{built, descriptor};
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::testing::{participant_runtime_in, World, STAFF};
use caelumora_subagents::component::{ASK_SUBAGENT, START_SUBAGENT, SUBAGENTS_KIND};
use caelumora_subagents::extension::{SubAgentError, SubAgentWriter};
use caelumora_subagents::library::SubagentsLibrary;
use caelumora_subagents::plugin::SubagentsPlugin;
use caelumora_subagents::SUBAGENTS;
use std::sync::Arc;

fn said(text: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        ..Message::new(Classification::general(Confidentiality::Internal))
    }
}

/// The world with sub-agents, whose model "leading" answers `leading`, one per call, and then
/// none - a turn that calls it again never ends -, and whose model "researching" answers what it
/// is given.
async fn world(leading: Vec<Answer>) -> World {
    configured_world(leading, |_| {}).await
}

/// The world `world` makes, its configuration changed by `configure` before the kernel is built.
async fn configured_world(leading: Vec<Answer>, configure: impl FnOnce(&mut AppConfig)) -> World {
    let libraries: [&dyn Library; 2] = [&AgentsLibrary, &SubagentsLibrary];
    let register = |registry: &mut ComponentRegistry| {
        register_answering(registry, "leading", leading);
        register_scripted(registry, "researching", vec![said("found it"), said("found more")]);
    };
    World::configured(&[&SubagentsPlugin], &libraries, register, configure).await
}

/// What the model "leading" answers to start a researcher, ask it again in the turn that reads
/// its first answer, and leave it without a word in the turn that reads its second.
fn leading_a_researcher() -> Vec<Answer> {
    let fixed = |answer: Message| -> Answer { Box::new(move |_| answer.clone()) };
    let start = calling(
        START_SUBAGENT,
        serde_json::json!({ "personality": "researcher", "brief": "Find it." }),
    );
    let ask_again: Answer = Box::new(|messages| {
        let thread = started(messages);
        calling(ASK_SUBAGENT, serde_json::json!({ "thread": thread, "text": "And more?" }))
    });
    vec![
        fixed(start),
        fixed(said("started")),
        ask_again,
        fixed(said("asked")),
        fixed(said("thanks")),
    ]
}

/// The thread `start_subagent` returned, as the model was given its result among `messages`.
fn started(messages: &[Message]) -> ThreadId {
    let results = messages.iter().flat_map(|message| &message.tool_results);
    let thread = results.filter(|result| result.tool_name == START_SUBAGENT).find_map(|result| {
        let result: serde_json::Value = serde_json::from_str(&result.content).ok()?;
        serde_json::from_value(result.get("thread")?.clone()).ok()
    });
    thread.unwrap()
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

/// The writer of sub-agents bound to the work of `actor` in `main`, asked by `user`.
async fn writer_of(
    main: &Arc<ChannelLoop>,
    actor: ActorId,
    user: UserId,
) -> Arc<dyn SubAgentWriter> {
    let runtime = agent_runtime(participant_runtime_in(main, actor));
    let internal = Classification::general(Confidentiality::Internal);
    let writer = runtime.writer::<dyn SubAgentWriter>(ActorId::User(user), internal).await;
    writer.unwrap()
}

/// A session of alice's with a lead of hers, whose agents do not answer - no turn ends, so that no
/// sub-agent is let go meanwhile -, and a personality "researcher" to start them of: the session,
/// its main channel and the lead.
async fn silent_lead(world: &World) -> (Arc<SessionLoop>, Arc<ChannelLoop>, ActorId) {
    let silent = || vec![ComponentDescriptor::new("model", "leading")];
    let internal = PersonalityKind::Internal;
    personality(world, "researcher", world.alice, internal, silent()).await;
    let leading = personality(world, "leading", world.alice, internal, silent()).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    (session, main, ActorId::Agent(lead))
}

/// The world `world` makes with `max_open` and `max_depth` as the plugin's section of the
/// configuration.
async fn limited_world(leading: Vec<Answer>, max_open: usize, max_depth: usize) -> World {
    let limits = serde_json::json!({ "max_open": max_open, "max_depth": max_depth });
    let configure = |config: &mut AppConfig| {
        config.plugins.insert(SUBAGENTS.to_string(), limits);
    };
    configured_world(leading, configure).await
}

/// Issue 558: an agent starts a sub-agent of a personality its tool lists - a new agent, named by
/// an adjective and the personality, in a thread of its own at the call -, is given back to read
/// what it answered, and asks it again in the turn that read it (issue 598), and is given that back
/// too; the turn that reads the second answer leaves it without a word, which lets it go: the
/// session follower closes its thread with it - let go, the agent deleted -, and nothing is left
/// to ask or to end. A personality of an external agent is no sub-agent's, and a thread without
/// one of the work's is none to ask.
#[tokio::test]
async fn an_agent_starts_a_sub_agent_asks_it_again_as_it_reads_its_answer_and_lets_it_go() {
    let world = world(leading_a_researcher()).await;
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
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);

    main.user_actor(world.alice)
        .expect("alice takes part")
        .send_message(None, Some("@lead find it".to_string()), None, Vec::new())
        .await
        .expect("said");
    let given_back = |text: &'static str| {
        move |e: &HistoryEntry| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == lead && message.content.as_deref() == Some(text))
        }
    };
    // The lead's turn that reads the second answer ends without a word: the thread is closed.
    let closed =
        |e: &HistoryEntry| matches!(e, HistoryEntry::ThreadClosed { by, .. } if *by != lead);
    let entries = main.history_once(|entries| entries.iter().any(closed)).await;
    let thread = entries
        .iter()
        .find_map(|e| match e {
            HistoryEntry::ThreadOpened { thread, by, .. } if *by == lead => Some(*thread),
            _ => None,
        })
        .expect("opened by the lead");
    let briefs: Vec<(ActorId, Option<&str>)> = entries
        .iter()
        .filter_map(|e| match e {
            HistoryEntry::GivenToRead { recipient, message, by: ActorId::Delegate(t), .. }
                if *t == thread && message.sender_id == lead =>
            {
                Some((*recipient, message.content.as_deref()))
            }
            _ => None,
        })
        .collect();
    let texts: Vec<Option<&str>> = briefs.iter().map(|(_, text)| *text).collect();
    assert_eq!(texts, [Some("Find it."), Some("And more?")], "asked again as it read the first");
    let second = entries.iter().position(given_back("found more")).expect("given back");
    let finished = entries[second..]
        .iter()
        .position(|e| matches!(e, HistoryEntry::TurnFinished { actor, .. } if *actor == lead))
        .expect("the turn that read it ended");
    let closing = entries[second..].iter().position(closed).expect("closed");
    assert!(finished < closing, "closed once the turn that read it ended: {entries:?}");
    let sub_agent = briefs[0].0.as_agent_id().expect("an agent");
    let agents = world.entity_store::<Agent>();
    let deleted = agents.get_deleted(&World::system(), sub_agent).await.expect("deleted");
    let name = deleted.entity.name.name.to_string();
    assert!(name.ends_with("-researcher"), "an adjective and the personality: {name}");
    assert!(!main.channel().lists(ActorId::Agent(sub_agent)), "let go");

    let writer = writer_of(&main, lead, world.alice).await;
    let refused = writer.ask(thread, "Still there?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "none to ask: {refused}");
    let refused = writer.end(thread).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "nor to end: {refused}");

    let origin = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let refused = writer.start("remote", origin, "Find it.".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::External(_)), "{refused}");
    let refused = writer.start("nobody", origin, "Find it.".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::UnknownPersonality(_)), "{refused}");
    let elsewhere = ThreadId::new();
    let refused = writer.ask(elsewhere, "Hello?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "{refused}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 558: the agent of a participant who does not own the session starts a sub-agent - made by
/// its owner, founded into the thread and delegated to in the kernel's name -, without the right to
/// change the session or the channel; and it alone asks or ends it, not another of the same owner.
#[tokio::test]
async fn an_agent_of_one_who_does_not_own_the_session_starts_a_sub_agent() {
    // A lead that does not answer: the sub-agents' work is its writer's alone.
    let world = world(Vec::new()).await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let bob = world.bob;
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", bob, internal, vec![model("researching")]).await;
    let leading = personality(&world, "leading", bob, internal, vec![model("leading")]).await;
    let lead = world.agent("bobs-lead", world.bob, &leading).await;
    let other = world.agent("bobs-other", world.bob, &leading).await;
    let session = world.shared_session(&[lead, other], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);

    let writer = writer_of(&main, lead, world.bob).await;
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
    let theirs = writer_of(&main, ActorId::Agent(other), world.bob).await;
    let refused = theirs.ask(thread, "Mine now?".to_string()).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "not another's to ask: {refused}");
    let refused = theirs.end(thread).await.unwrap_err();
    assert!(matches!(refused, SubAgentError::NoSubAgent(_)), "nor to end: {refused}");
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
    // A lead that does not answer: the sub-agents' work is its writer's alone.
    let world = world(Vec::new()).await;
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

    let writer = writer_of(&main, lead, world.alice).await;
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
    // A lead that does not answer: the sub-agents' work is its writer's alone.
    let world = world(Vec::new()).await;
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", world.alice, internal, vec![model("researching")]).await;
    let leading =
        personality(&world, "leading", world.alice, internal, vec![model("leading")]).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let writer = writer_of(&main, ActorId::Agent(lead), world.alice).await;
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
    let world = world(Vec::new()).await;
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

/// Issue 597: unless the operator allows more, a sub-agent starts none of its own - refused before
/// anything is made.
#[tokio::test]
async fn a_sub_agent_starts_none_of_its_own_by_default() {
    let world = world(Vec::new()).await;
    let (session, main, lead) = silent_lead(&world).await;
    let line = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };

    let led = writer_of(&main, lead, world.alice).await;
    let thread = led.start("researcher", line, "Find it.".to_string()).await.expect("started");
    let sub_agent = main.delegation(thread).await.expect("delegated").recipient;
    let theirs = writer_of(&main, sub_agent, world.alice).await;
    let origin = ThreadOrigin { thread, message: MessageId::new() };
    let refused = theirs.start("researcher", origin, "And you?".to_string()).await;
    assert!(matches!(refused, Err(SubAgentError::TooDeep(1))), "{refused:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 597: sub-agents are started within the limits of the plugin's section of the
/// configuration - no deeper than `max_depth`, no more open at once in a tree than `max_open`,
/// each refused before an agent is made -; one ended makes room for another.
#[tokio::test]
async fn sub_agents_are_started_within_the_limits_the_operator_set() {
    let world = limited_world(Vec::new(), 2, 2).await;
    let (session, main, lead) = silent_lead(&world).await;
    let agents = || async {
        let listed = world.entity_store::<Agent>().list(&World::system(), &Query::all()).await;
        listed.unwrap().items.len()
    };
    let deleted = || async {
        let agents = world.entity_store::<Agent>();
        let listed = agents.list_deleted(&World::system(), &Query::all()).await;
        listed.unwrap().items.len()
    };
    let origin = |thread| ThreadOrigin { thread, message: MessageId::new() };
    let line = origin(ThreadId::line(main.id()));
    let brief = || "Find it.".to_string();

    let led = writer_of(&main, lead, world.alice).await;
    let first = led.start("researcher", line, brief()).await.expect("started");
    let first_agent = main.delegation(first).await.expect("delegated").recipient;
    let firsts = writer_of(&main, first_agent, world.alice).await;
    let second = firsts.start("researcher", origin(first), brief()).await.expect("two deep");
    let second_agent = main.delegation(second).await.expect("delegated").recipient;
    let seconds = writer_of(&main, second_agent, world.alice).await;
    let before = (agents().await, deleted().await);

    let refused = seconds.start("researcher", origin(second), brief()).await;
    assert!(matches!(refused, Err(SubAgentError::TooDeep(2))), "{refused:?}");
    let refused = led.start("researcher", line, brief()).await;
    assert!(matches!(refused, Err(SubAgentError::TooMany(2))), "two open in the tree: {refused:?}");
    assert_eq!((agents().await, deleted().await), before, "no agent made, nor deleted again");

    firsts.end(second).await.expect("ended");
    led.start("researcher", line, brief()).await.expect("room for another");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 597: a sub-agent let go - left without a word in the turn that read its answer (issue
/// 598) - counts no more, though its thread is not closed yet: here with nobody to close it.
#[tokio::test]
async fn a_sub_agent_let_go_counts_no_more() {
    let world = limited_world(leading_a_researcher(), 1, 1).await.unfollowed();
    let model = |kind: &str| ComponentDescriptor::new("model", kind);
    let internal = PersonalityKind::Internal;
    personality(&world, "researcher", world.alice, internal, vec![model("researching")]).await;
    let subagents = ComponentDescriptor {
        params: serde_json::json!({ "personalities": ["researcher"] }),
        ..ComponentDescriptor::new("subagents", SUBAGENTS_KIND)
    };
    let components = vec![model("leading"), subagents];
    let leading = personality(&world, "leading", world.alice, internal, components).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let lead = ActorId::Agent(lead);

    main.user_actor(world.alice)
        .expect("alice takes part")
        .send_message(None, Some("@lead find it".to_string()), None, Vec::new())
        .await
        .expect("said");
    // The lead's turn that read the second answer ended: its delegation let go.
    let read_the_second = |entries: &[HistoryEntry]| {
        let given_back = entries.iter().position(|e| {
            matches!(e, HistoryEntry::GivenToRead { recipient, message, .. }
                if *recipient == lead && message.content.as_deref() == Some("found more"))
        });
        given_back.is_some_and(|at| {
            entries[at..]
                .iter()
                .any(|e| matches!(e, HistoryEntry::TurnFinished { actor, .. } if *actor == lead))
        })
    };
    let entries = main.history_once(read_the_second).await;
    let thread = entries
        .iter()
        .find_map(|e| match e {
            HistoryEntry::ThreadOpened { thread, by, .. } if *by == lead => Some(*thread),
            _ => None,
        })
        .expect("opened by the lead");
    let delegation = main.delegation(thread).await.expect("not closed: nobody closes it");
    assert!(delegation.is_let_go(), "let go");

    let line = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let led = writer_of(&main, lead, world.alice).await;
    led.start("researcher", line, "Find more.".to_string())
        .await
        .expect("the one let go not counted");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 597: a sub-agent is cleared no wider than the agent that starts it - of a lead limited to
/// a role, one of a personality that limits nothing, or that names another role besides, even one
/// its owner does not hold, is refused; one limited as the lead is, started.
#[tokio::test]
async fn a_sub_agent_is_cleared_no_wider_than_the_agent_that_starts_it() {
    let world = world(Vec::new()).await;
    let silent = || vec![ComponentDescriptor::new("model", "leading")];
    let limited = async |name: &str, roles: Option<Vec<RoleId>>| {
        let internal = PersonalityKind::Internal;
        let mut limited = Personality {
            roles,
            ..personality(&world, name, world.alice, internal, silent()).await
        };
        let personalities = &world.kernel.entity_stores().personalities;
        personalities.update(&World::system(), &mut limited).await.unwrap();
        limited
    };
    let leading = limited("leading", Some(vec![*STAFF])).await;
    limited("unlimited", None).await;
    limited("other", Some(vec![*STAFF, RoleId::new()])).await;
    limited("staff", Some(vec![*STAFF])).await;
    let lead = world.agent("lead", world.alice, &leading).await;
    let session = world.session_of(&[], &[lead], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let line = ThreadOrigin { thread: ThreadId::line(main.id()), message: MessageId::new() };
    let writer = writer_of(&main, ActorId::Agent(lead), world.alice).await;

    for wider in ["unlimited", "other"] {
        let refused = writer.start(wider, line, "Find it.".to_string()).await;
        assert!(matches!(refused, Err(SubAgentError::ClearedWider(_))), "{wider}: {refused:?}");
    }
    writer.start("staff", line, "Find it.".to_string()).await.expect("limited as the lead");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}
