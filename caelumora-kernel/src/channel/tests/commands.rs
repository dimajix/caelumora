//! The user commands in a running channel - `/help`, `/agent wakeup`, `/participant nudge`,
//! `/participant add`, `/agent create`, `/agent usage`, `/channel notes` - and the nudges they ask
//! for. The commands an agent carries out itself are the agents' (`agent_commands`).

use super::*;
use crate::command::CommandContext;
use crate::error::is_forbidden;
use crate::session::ChannelSpec;
use crate::testing::{attributes, quiet, shown, World};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::actor::Rejection;
use caelumora_domain::agent::{AgentId, Personality};
use caelumora_domain::command::{CommandOutput, CommandVisibility};
use caelumora_domain::message::TokenUsage;
use caelumora_domain::query::Query;
use caelumora_domain::scope::Scope;
use caelumora_domain::user::testing::make_user;
use caelumora_domain::value::name::ScopedName;
use caelumora_domain::value::text::{Briefing, Summary};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::time;

#[tokio::test]
async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
    let alice = make_user("alice");
    // `/agent wakeup` is hidden from everybody.
    let policy = Policy(
        |action| !matches!(action, ActionType::ShowCommand { command, .. } if command.name.as_str() == "agent wakeup"),
    );
    let channel = channel_with(&[&alice], Arc::new(policy)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    let reply = actor.execute_command("help").await.expect("/help runs");
    assert_eq!(reply.visibility, CommandVisibility::Private, "/help answers the issuer alone");
    let text = reply.output.expect("/help answers here");
    assert!(text.contains("/help"), "{text}");
    assert!(!text.contains("wakeup"), "a hidden command is not listed: {text}");
    let names: Vec<String> =
        actor.visible_commands().await.into_iter().map(|c| c.name.to_string()).collect();
    let participant =
        ["participant add", "participant list", "participant nudge", "participant remove"];
    let agent = ["agent create", "agent delete", "agent list", "agent usage"];
    assert_eq!(names, [&agent[..], &["help"], &participant].concat());

    for hidden in ["agent wakeup", "wakeup", "help agent wakeup", "help wakeup"] {
        let error = actor.execute_command(hidden).await.expect_err("hidden");
        assert!(
            matches!(as_command_error(&error), CommandError::Unknown(_)),
            "a hidden command looks unknown, by its name and its short form: /{hidden}: {error}"
        );
    }
    // A group is listed by `/help` as far as it may be seen (issue 560).
    let reply = actor.execute_command("help agent").await.expect("/help agent runs");
    let text = reply.output.expect("/help answers here");
    assert!(text.contains("Commands of `/agent`") && text.contains("/agent create"), "{text}");
    assert!(!text.contains("wakeup") && !text.contains("/help"), "{text}");
    let error = actor.execute_command("help nonsense").await.expect_err("no such group");
    assert!(matches!(as_command_error(&error), CommandError::Unknown(_)), "{error}");

    // A group of which nothing may be seen is unknown as well.
    let hide_agents = Policy(
        |action| !matches!(action, ActionType::ShowCommand { command, .. } if command.name.as_str().starts_with("agent ")),
    );
    let hidden = channel_with(&[&alice], Arc::new(hide_agents)).await;
    let error = hidden.user_actor(alice.id).unwrap().execute_command("help agent").await;
    let error = error.expect_err("a hidden group");
    assert!(matches!(as_command_error(&error), CommandError::Unknown(_)), "{error}");
    // Given arguments it takes none of, a hidden command is unknown still (issue 574).
    let error = hidden.user_actor(alice.id).unwrap().execute_command("agent list all").await;
    let error = error.expect_err("hidden");
    assert!(matches!(as_command_error(&error), CommandError::Unknown(_)), "{error}");

    let quiet = time::timeout(Duration::from_millis(200), events.recv()).await;
    assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
    assert!(channel.history().await.is_empty(), "commands never enter the history");
    running.abort();
}

/// A command of the channel is issued by its name or its short form, and `/help` tells both,
/// describes it by either, and lists a group (issue 560).
#[tokio::test]
async fn a_command_is_issued_and_described_by_its_name_or_its_short_form() {
    let alice = make_user("alice");
    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();

    let reply = actor.execute_command("agent   wakeup").await.expect("by its name");
    let answer = (reply.command.as_str(), reply.output.as_deref());
    assert_eq!(answer, ("agent wakeup", Some("No agent was asleep.")));
    let error = actor.execute_command("participant nudge").await.expect_err("nobody named");
    assert!(
        matches!(
            as_command_error(&error),
            CommandError::InvalidArguments { command, reason }
                if command == "participant nudge" && reason == "it takes <participant>"
        ),
        "refused by what it takes (issue 583): {error}"
    );

    let help = |line: &'static str| {
        let actor = Arc::clone(&actor);
        async move { actor.execute_command(line).await.expect("/help runs").output.unwrap() }
    };
    let all = help("help").await;
    assert!(all.contains("- `/agent wakeup [agent]` (also `/wakeup`) — Wakes"), "{all}");
    assert!(all.contains("- `/participant add <name>` (also `/add`) — Brings"), "{all}");
    assert!(all.contains("- `/agent create <personality> [as <name>]` — Makes"), "{all}");
    let by_short = help("help add").await;
    assert!(by_short.starts_with("`/participant add <name>` (also `/add`) — Brings"), "{by_short}");
    assert_eq!(help("help participant add").await, by_short, "by its name as by its short form");
    let group = help("help participant").await;
    assert!(group.contains("/participant add") && group.contains("/participant nudge"), "{group}");
    assert!(!group.contains("/agent") && !group.contains("`/help"), "{group}");
}

#[tokio::test]
async fn wakeup_is_announced_to_the_channel_in_the_issuers_name() {
    let alice = make_user("alice");
    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let mut events = channel.observe();
    let mut following = actor.watch();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    // By its short form (issue 560): what is announced is its name.
    let reply = actor.execute_command("wakeup").await.expect("/wakeup runs");
    assert_eq!(reply.command, "agent wakeup");
    let shown = time::timeout(Duration::from_secs(2), following.recv())
        .await
        .expect("shown")
        .map(|e| e.event);
    assert!(
        matches!(shown, Ok(ActorEvent::CommandExecuted { .. })),
        "the issuer sees it from its own actor (issue 220)"
    );
    assert_eq!(reply.visibility, CommandVisibility::Channel, "the answer goes to the channel");
    assert_eq!(
        reply.output.as_deref(),
        Some("No agent was asleep."),
        "and back to the issuer, whom the channel does not route it to"
    );

    let envelope = time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("the announcement arrives")
        .unwrap();
    assert_eq!(envelope.context.sender_id, ActorId::User(alice.id));
    match envelope.event {
        ActorEvent::CommandExecuted { command, output, .. } => {
            assert_eq!(command, "agent wakeup");
            assert_eq!(output, "No agent was asleep.");
        }
        other => panic!("expected CommandExecuted, got {:?}", other.kind()),
    }
    assert!(channel.history().await.is_empty(), "commands never enter the history");
    running.abort();
}

#[tokio::test]
async fn wakeup_reports_refusals_and_unknown_agents() {
    let alice = make_user("alice");
    let refuse_waking = Policy(|action| !matches!(action, ActionType::WakeChannel(_)));
    let channel = channel_with(&[&alice], Arc::new(refuse_waking)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup").await.expect_err("waking is refused");
    assert!(matches!(as_command_error(&error), CommandError::Forbidden { .. }), "{error}");
    assert!(is_forbidden(&error), "{error}");

    let refuse_running = Policy(
        |action| !matches!(action, ActionType::ExecuteCommand { command, .. } if command.name.as_str() == "agent wakeup"),
    );
    let channel = channel_with(&[&alice], Arc::new(refuse_running)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup").await.expect_err("running it is refused");
    assert!(
        matches!(as_command_error(&error), CommandError::Forbidden { .. }),
        "visible but not runnable: {error}"
    );

    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup @nobody").await.expect_err("no such agent");
    assert!(matches!(as_command_error(&error), CommandError::InvalidArguments { .. }), "{error}");
    let error = actor.execute_command("").await.expect_err("empty");
    assert!(matches!(as_command_error(&error), CommandError::Empty), "{error}");
    let error = actor.execute_command("nonsense").await.expect_err("unknown");
    assert!(matches!(as_command_error(&error), CommandError::Unknown(_)), "{error}");
}

#[tokio::test]
async fn wakeup_wakes_the_agent_named_and_refuses_an_ambiguous_name() {
    let (channel, alice, sleepers) = channel_with_sleepers(&["Sassy", "Dr. Who", "Dr-Who"]).await;
    let actor = channel.user_actor(alice).unwrap();

    let reply = actor.execute_command("wakeup @Sassy").await.expect("found regardless of case");
    assert_eq!(reply.output.as_deref(), Some("Woke Sassy."));
    assert!(sleepers[0].woken() && !sleepers[1].woken(), "it alone");
    let reply = actor.execute_command("wakeup Dr. Who").await.expect("by its name, spaces and all");
    assert_eq!(reply.output.as_deref(), Some("Woke Dr. Who."));

    let error = actor.execute_command("wakeup @dr-who").await.expect_err("ambiguous");
    assert!(
        matches!(as_command_error(&error), CommandError::InvalidArguments { reason, .. } if reason.contains("Dr-Who and Dr. Who")),
        "{error}"
    );
    assert!(!sleepers[2].woken(), "nobody is picked");
}

/// Runs `/participant nudge` with `args` in `channel` as alice.
async fn nudge(channel: &ChannelLoop, alice: UserId, args: &str) -> CommandResult<CommandOutput> {
    use crate::command::participant;
    let executor = CommandExecutor::empty(Arc::new(AllowAllPolicy::new()));
    let issuer = channel.user_actor(alice).expect("alice takes part");
    let context = CommandContext { issuer: &issuer, channel, executor: &executor };
    let commands = participant::commands();
    let nudge = commands
        .iter()
        .find(|c| c.description().name.as_str() == "participant nudge")
        .expect("a command of the group");
    nudge.execute(args, &context).await
}

/// The next end of a turn of `agent` on `events`, with its envelope.
async fn turn_finished(events: &mut Receiver<Envelope>, agent: AgentId) -> Envelope {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::TurnFinished { .. })
            {
                break envelope;
            }
        }
    })
    .await
    .expect("the agent answers")
}

#[tokio::test]
async fn a_nudged_agent_responds_in_the_name_of_who_nudged_it_and_the_history_keeps_nothing_of_the_nudge(
) {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Again.".to_string())).await).await;
    let alice = alice_of(&context);
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let mut following = channel.user_actor(alice).unwrap().watch();

    let output = nudge(&channel, alice, "@helper").await.expect("/nudge runs");
    assert_eq!(
        (output.text.as_str(), output.visibility),
        ("Asked helper to respond.", CommandVisibility::Private),
        "told to the issuer alone"
    );

    let finished = turn_finished(&mut events, agent).await;
    let ActorEvent::TurnFinished { messages, .. } = &finished.event else { unreachable!() };
    assert_eq!(
        messages.iter().map(|m| m.content.as_deref()).collect::<Vec<_>>(),
        [Some("Again.")],
        "it responds without being told anything new"
    );
    assert_eq!(
        (finished.context.sender_id, finished.context.initiator_id),
        (ActorId::Agent(agent), ActorId::User(alice)),
        "by the agent, on her word"
    );
    let said: Vec<_> = channel.history().await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["hello", "Again.", "Again."], "the nudge leaves nothing in the history");
    while let Ok(Some(envelope)) = following.try_recv() {
        assert!(
            !matches!(
                envelope.event,
                ActorEvent::NudgeRequested { .. } | ActorEvent::ResponseRequested { .. }
            ),
            "nor is it shown: {}",
            envelope.event.kind()
        );
    }
    running.abort();
}

#[tokio::test]
async fn a_nudge_wakes_the_agent_it_names_and_ends_a_quiet_channel() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        start(scripted_channel(Script::Answers("Awake.".to_string())).await).await;
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    channel.cancel(&context, None).await.expect("permitted");

    nudge(&channel, alice_of(&context), "helper").await.expect("/nudge runs");

    let finished = turn_finished(&mut events, agent).await;
    let ActorEvent::TurnFinished { messages, .. } = &finished.event else { unreachable!() };
    assert_eq!(
        messages.iter().map(|m| m.content.as_deref()).collect::<Vec<_>>(),
        [Some("Awake.")],
        "asked, though the channel was quiet"
    );
    assert!(scripted(&helper).woken(), "and woken");
    assert!(!channel.is_quiet(ThreadId::line(channel.id())), "its thread is quiet no longer");
    running.abort();
}

/// Whether an agent asked to respond is answering already is the agent's to tell, not the
/// issuer's (issue 567): an agent in a turn is asked as any.
#[tokio::test]
async fn an_agent_in_a_turn_is_asked_as_any() {
    let Stuck { channel, context, running, .. } = stuck_in_a_turn().await;
    let alice = alice_of(&context);

    let output = nudge(&channel, alice, "@helper").await.expect("answered, not failed");
    assert_eq!(output.text, "Asked helper to respond.");
    running.abort();
}

/// Whether a turn can be answered again the channel decides as it admits the request (issue
/// 567): one that is neither the agent's turn under way nor its last given up is rejected, and the
/// user told - their actor asks nothing of the agent.
#[tokio::test]
async fn a_turn_that_cannot_be_answered_again_is_rejected_at_the_door() {
    let Stuck { channel, context, agent, running, .. } = stuck_in_a_turn().await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let mut hers = alice.watch();
    alice.request_retry_of_turn(ActorId::Agent(agent), TurnId::new()).await.expect("on its way");
    let ActorEvent::EventRejected { rejected, reason, .. } =
        shown(&mut hers, "EventRejected").await
    else {
        unreachable!()
    };
    assert_eq!((rejected.as_str(), reason), ("RetryRequested", Rejection::Invalid));
    assert!(channel.turn_of(ActorId::Agent(agent)).is_some(), "its turn goes on");
    running.abort();
}

#[tokio::test]
async fn only_someone_else_in_the_channel_is_nudged() {
    let Stuck { channel, context, running, .. } = stuck_in_a_turn().await;
    let alice = alice_of(&context);

    for args in ["@nobody", "alice"] {
        let error = nudge(&channel, alice, args).await.expect_err("names nobody else here");
        assert!(matches!(error, CommandError::InvalidArguments { .. }), "{args:?}: {error}");
    }
    let actor = channel.user_actor(alice).unwrap();
    let error = actor.request_response(ActorId::User(alice)).await.expect_err("herself");
    assert!(
        matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NudgesItself { .. })),
        "{error}"
    );
    // Whether whom it asks takes part is the channel's to decide, as it admits the request.
    let mut hers = actor.watch();
    actor.request_response(ActorId::Agent(AgentId::new())).await.expect("on its way");
    let ActorEvent::EventRejected { rejected, reason, .. } =
        shown(&mut hers, "EventRejected").await
    else {
        unreachable!()
    };
    assert_eq!((rejected.as_str(), reason), ("NudgeRequested", Rejection::Invalid), "a stranger");
    running.abort();
}

#[tokio::test]
async fn a_nudged_user_is_shown_who_asks_them_to_respond() {
    let (alice, bob) = (make_user("alice"), make_user("bob"));
    let channel = channel_with(&[&alice, &bob], Arc::new(AllowAllPolicy::new())).await;
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let mut his = channel.user_actor(bob.id).unwrap().watch();

    let output = nudge(&channel, alice.id, "Bob").await.expect("/nudge runs");
    assert_eq!(output.text, "Asked bob to respond.");

    let asked =
        time::timeout(Duration::from_secs(5), his.recv()).await.expect("he is asked").unwrap();
    assert!(matches!(asked.event, ActorEvent::ResponseRequested { .. }), "{}", asked.event.kind());
    assert_eq!(asked.context.sender_id, ActorId::User(alice.id), "by her");
    assert!(channel.history().await.is_empty(), "the history keeps nothing of it");
    running.abort();
}

/// `/participant add` and `/agent create` (issue 555) refuse what they cannot use before they look
/// anything up: nothing named, a text that is no name, nothing after `as`.
#[tokio::test]
async fn participant_add_and_agent_create_refuse_what_they_cannot_use() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let actor = session.channels()[0].user_actor(world.alice).unwrap();
    let too_long = "x".repeat(101);
    let named_too_long = format!("plain as {too_long}");
    for (command, args) in [
        ("add", ""),
        ("add", too_long.as_str()),
        ("agent create", ""),
        ("agent create", "plain as "),
        ("agent create", too_long.as_str()),
        ("agent create", named_too_long.as_str()),
    ] {
        let error =
            actor.execute_command(&format!("{command} {args}")).await.expect_err("unusable");
        assert!(
            matches!(as_command_error(&error), CommandError::InvalidArguments { .. }),
            "/{command} {args}: {error}"
        );
    }
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The commands of the groups refuse what they cannot use (issue 560) before they change anything:
/// the lists take nothing, `/participant remove`, `/agent delete`, `/agent usage`,
/// `/channel describe` and `/channel notes` refuse what names nobody - `/agent usage` a user,
/// too -, `/channel rename`, `summary` and `briefing` a text that does not fit.
#[tokio::test]
async fn the_commands_of_the_groups_refuse_what_they_cannot_use() {
    let world = World::new().await;
    let rename_too_long = format!("channel rename {}", "x".repeat(101));
    let create_too_long = format!("channel create {}", "x".repeat(101));
    let summary_too_long = format!("channel summary {}", "x".repeat(Summary::MAX_CHARS + 1));
    let briefing_too_long = format!("channel briefing {}", "x".repeat(Briefing::MAX_CHARS + 1));
    let session = world.shared_session(&[], &[]).await;
    let actor = session.channels()[0].user_actor(world.alice).unwrap();
    for line in [
        "participant list everyone",
        "participant remove",
        "participant remove zed",
        "agent list everyone",
        "agent delete",
        "agent delete zed",
        "agent usage zed",
        "agent usage alice",
        "channel list everyone",
        "channel describe nowhere",
        "channel notes nowhere",
        "channel create",
        &create_too_long,
        "channel delete",
        "channel delete nowhere",
        "channel join",
        "channel join nowhere",
        "channel leave zed",
        "channel leave bob",
        "channel rename",
        &rename_too_long,
        &summary_too_long,
        &briefing_too_long,
    ] {
        let error = actor.execute_command(line).await.expect_err("unusable");
        assert!(
            matches!(as_command_error(&error), CommandError::InvalidArguments { .. }),
            "/{line}: {error}"
        );
    }
    let in_main = world.participants(world.alice, session.id(), session.channels()[0].id()).await;
    assert_eq!(in_main.len(), 2, "nobody was removed");
    assert_eq!(session.channels().len(), 1, "no channel was made");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// `/agent create` names the agent after the personality, unless the last `as` names it - so
/// that a personality's name may hold one (issue 555).
#[tokio::test]
async fn agent_create_names_the_agent_after_the_personality_or_after_the_last_as() {
    let world = World::new().await;
    world.personality("plain", Some(world.alice), &[]).await;
    world.personality("Writer as Editor", Some(world.alice), &[]).await;
    let session = world.shared_session(&[], &[]).await;
    let alice = session.channels()[0].user_actor(world.alice).unwrap();
    let made = |args: &'static str| {
        let alice = Arc::clone(&alice);
        async move {
            let line = format!("agent create {args}");
            alice.execute_command(&line).await.unwrap().output.unwrap()
        }
    };
    let plain = made("plain").await;
    assert!(plain.starts_with("Made the agent plain of plain"), "{plain}");
    let bob = made("Writer as Editor as Bob").await;
    assert!(bob.starts_with("Made the agent Bob of Writer as Editor"), "{bob}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// `/agent create` finds a personality from the channel, the innermost first, and makes an agent
/// of one of the channel's scope there - the only place it makes agents (issue 555).
#[tokio::test]
async fn agent_create_makes_the_agent_of_a_personality_of_the_channel_in_the_channel() {
    let world = World::new().await;
    world.personality("plain", Some(world.alice), &[]).await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let in_main = Scope::ChannelScope { session_id: session.id(), channel_id: main.id() };
    let mut own = Personality {
        name: ScopedName::new("plain", in_main.clone()).unwrap(),
        security_attributes: attributes(Some(world.alice), &[]),
        ..make_personality("plain")
    };
    world.kernel.entity_stores().personalities.create(&World::system(), &mut own).await.unwrap();

    let alice = main.user_actor(world.alice).unwrap();
    alice.execute_command("agent create plain as Scout").await.expect("made");
    let stores = world.kernel.entity_stores();
    let agents = stores.agents.list(&World::system(), &Query::all()).await.unwrap().items;
    let scout = agents.into_iter().find(|agent| agent.name.name == "scout").unwrap();
    assert_eq!(scout.personality_id, own.id, "the channel's, nearer than the global one");
    assert_eq!(scout.name.scope, in_main, "in the channel's scope");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// `/agent usage` tells each agent taking part how full its context is and what its calls used,
/// input and output apart, and adds up the calls of them all; named, it tells one agent alone
/// (issue 573) - its context's size where its model says it, and in a channel without agents, that
/// there are none.
#[tokio::test]
async fn agent_usage_tells_the_context_and_the_calls_of_each_agent_and_their_totals() {
    // Agents that run, so that they take part.
    let answering = ScriptedAgents(Script::Answers("ok".to_string()));
    let world = World::with(&[], &[&answering], |_| {}).await;
    let plain = world.personality("plain", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &plain).await;
    let scout = world.agent("scout", world.alice, &plain).await;
    let runner = world.agent("runner", world.alice, &plain).await;
    let session = world.shared_session(&[helper, scout, runner], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let states = world.kernel.state_store();
    let mut state = states.get_agent_channel_state(helper, main.id()).await.unwrap();
    state.used = Some(TokenUsage {
        calls: 3,
        prompt_tokens: 9000,
        completion_tokens: 600,
        context_size: Some(8192),
        context_tokens: 2048,
    });
    states.put_agent_channel_state(helper, main.id(), &mut state).await.unwrap();
    // One whose model does not say how large its context is.
    let mut state = states.get_agent_channel_state(runner, main.id()).await.unwrap();
    state.used = Some(TokenUsage {
        calls: 1,
        prompt_tokens: 500,
        completion_tokens: 20,
        context_size: None,
        context_tokens: 520,
    });
    states.put_agent_channel_state(runner, main.id(), &mut state).await.unwrap();
    let alice = main.user_actor(world.alice).unwrap();

    let reply = alice.execute_command("agent usage").await.expect("hers to read");
    assert_eq!(reply.visibility, CommandVisibility::Private, "answered to her alone");
    assert_eq!(
        reply.output.as_deref(),
        Some(
            "What the agents of this channel used:\n\n\
             - helper (@helper): 2048 of 8192 tokens in its context (25 %); 3 calls used 9000 \
             input and 600 output tokens\n\
             - scout (@scout): no calls counted\n\
             - runner (@runner): 520 tokens in its context, its size unknown; 1 call used 500 \
             input and 20 output tokens\n\
             \nIn all: 4 calls used 9500 input and 620 output tokens."
        )
    );
    let one = alice.execute_command("agent usage @scout").await.unwrap().output.unwrap();
    assert_eq!(one, "scout (@scout): no calls counted", "the one named alone, without a total");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let without = world.shared_session(&[], &[]).await;
    let alice = without.channels()[0].user_actor(world.alice).unwrap();
    let none = alice.execute_command("agent usage").await.unwrap().output.unwrap();
    assert_eq!(none, "No agent takes part in this channel.", "nothing to add up");
    world.manager.stop(&World::as_user(world.alice), without.id()).await.unwrap();
}

/// `/channel notes` shows what the agents noted in the channel, by their authors - one that noted
/// nothing left out -, or in the channel of the session named (issue 574), where nothing may be
/// noted; in a channel where nothing is noted, that nothing is.
#[tokio::test]
async fn channel_notes_shows_what_the_agents_noted_by_their_authors() {
    // Agents that run, so that they take part.
    let answering = ScriptedAgents(Script::Answers("ok".to_string()));
    let world = World::with(&[], &[&answering], |_| {}).await;
    let plain = world.personality("plain", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &plain).await;
    let scout = world.agent("scout", world.alice, &plain).await;
    let runner = world.agent("runner", world.alice, &plain).await;
    let session = world.shared_session(&[helper, scout, runner], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.note(helper, main.id(), &["The build needs Rust 1.80.", "Tests run with nextest."]).await;
    world.note(runner, main.id(), &["Deploys on Fridays are off."]).await;
    let alice = main.user_actor(world.alice).unwrap();

    let reply = alice.execute_command("channel notes").await.expect("hers to read");
    assert_eq!(reply.visibility, CommandVisibility::Private, "answered to her alone");
    let board = "Noted in main:\n\n\
                 helper:\n\
                 - #1 The build needs Rust 1.80.\n\
                 - #2 Tests run with nextest.\n\n\
                 runner:\n\
                 - #1 Deploys on Fridays are off.";
    assert_eq!(reply.output.as_deref(), Some(board), "scout, who noted nothing, left out");
    let named = alice.execute_command("channel notes main").await.unwrap().output.unwrap();
    assert_eq!(named, board, "the channel named, the same");
    let side = ChannelSpec { components: quiet(), ..ChannelSpec::new("side") };
    session.create_channel(&World::as_user(world.alice), side).await.expect("hers to make");
    let other = alice.execute_command("channel notes side").await.unwrap().output.unwrap();
    assert_eq!(other, "Nothing is noted in side.", "another channel named, not this one");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let without = world.shared_session(&[], &[]).await;
    let alice = without.channels()[0].user_actor(world.alice).unwrap();
    let none = alice.execute_command("channel notes").await.unwrap().output.unwrap();
    assert_eq!(none, "Nothing is noted in main.");
    world.manager.stop(&World::as_user(world.alice), without.id()).await.unwrap();
}
