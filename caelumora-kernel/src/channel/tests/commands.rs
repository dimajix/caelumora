//! The user commands in a running channel - `/help`, `/agent wakeup`, `/participant nudge`,
//! `/participant add`, `/agent create` - and the nudges they ask for. The commands an agent carries
//! out itself are the agents' (`agent_commands`).

use super::*;
use crate::command::CommandContext;
use crate::error::is_forbidden;
use crate::testing::{attributes, World};
use caelumora_api::command::{CommandError, CommandResult};
use caelumora_domain::agent::{AgentId, Personality};
use caelumora_domain::command::{CommandOutput, CommandVisibility};
use caelumora_domain::query::Query;
use caelumora_domain::scope::Scope;
use caelumora_domain::user::testing::make_user;
use caelumora_domain::value::command_name::CommandName;
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
    let agent = ["agent create", "agent delete", "agent list"];
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
        matches!(as_command_error(&error), CommandError::InvalidArguments { command, .. } if command == "participant nudge"),
        "{error}"
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
    use crate::command::participant::ParticipantGroupComponent;
    use crate::command::ChannelCommandProvider;
    let executor = CommandExecutor::empty(Arc::new(AllowAllPolicy::new()));
    let issuer = channel.user_actor(alice).expect("alice takes part");
    let context = CommandContext { issuer: &issuer, channel, executor: &executor };
    let nudge = CommandName::fixed("participant nudge");
    ParticipantGroupComponent.execute(&nudge, args, &context).await
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
    while let Ok(envelope) = following.try_recv() {
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
    channel.cancel(&context).await.expect("permitted");

    nudge(&channel, alice_of(&context), "helper").await.expect("/nudge runs");

    let finished = turn_finished(&mut events, agent).await;
    let ActorEvent::TurnFinished { messages, .. } = &finished.event else { unreachable!() };
    assert_eq!(
        messages.iter().map(|m| m.content.as_deref()).collect::<Vec<_>>(),
        [Some("Awake.")],
        "asked, though the channel was quiet"
    );
    assert!(scripted(&helper).woken(), "and woken");
    assert!(!channel.quiet.load(Ordering::SeqCst), "the channel is quiet no longer");
    running.abort();
}

#[tokio::test]
async fn an_agent_in_a_turn_is_not_nudged_into_a_second_one() {
    let Stuck { channel, context, agent, running, .. } = stuck_in_a_turn().await;
    let alice = alice_of(&context);

    let output = nudge(&channel, alice, "@helper").await.expect("answered, not failed");
    assert_eq!(output.text, "helper is responding already.");
    let error = channel
        .user_actor(alice)
        .unwrap()
        .request_response(ActorId::Agent(agent))
        .await
        .expect_err("refused");
    assert!(
        matches!(
            error.downcast_ref::<ChannelError>(),
            Some(ChannelError::AlreadyResponding { .. })
        ),
        "{error}"
    );
    running.abort();
}

#[tokio::test]
async fn only_someone_else_in_the_channel_is_nudged() {
    let Stuck { channel, context, running, .. } = stuck_in_a_turn().await;
    let alice = alice_of(&context);

    for args in ["", "@nobody", "alice"] {
        let error = nudge(&channel, alice, args).await.expect_err("names nobody else here");
        assert!(matches!(error, CommandError::InvalidArguments { .. }), "{args:?}: {error}");
    }
    let actor = channel.user_actor(alice).unwrap();
    let error = actor.request_response(ActorId::User(alice)).await.expect_err("herself");
    assert!(
        matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NudgesItself { .. })),
        "{error}"
    );
    let error =
        actor.request_response(ActorId::Agent(AgentId::new())).await.expect_err("a stranger");
    assert!(
        matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })),
        "{error}"
    );
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
/// the lists take nothing, `/participant remove`, `/agent delete` and `/channel describe` refuse
/// what names nobody, `/channel rename`, `summary` and `briefing` a text that does not fit.
#[tokio::test]
async fn the_commands_of_the_groups_refuse_what_they_cannot_use() {
    let world = World::new().await;
    let rename_too_long = format!("channel rename {}", "x".repeat(101));
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
        "channel list everyone",
        "channel describe nowhere",
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
