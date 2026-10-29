//! The user commands in a running channel - `/help`, `/wakeup`, `/nudge` -
//! and the nudges they ask for. The commands an agent carries out itself
//! are the agents' (`agent_commands`).

use super::*;
use crate::command::{CommandContext, CommandError, CommandResult};
use crate::error::is_forbidden;
use caelumora_model::agent::AgentId;
use caelumora_model::command::{CommandOutput, CommandVisibility};
use caelumora_model::user::testing::make_user;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::time;

#[tokio::test]
async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
    let alice = make_user("alice");
    // `/wakeup` is hidden from everybody.
    let policy = Policy(
        |action| !matches!(action, ActionType::ShowCommand { command, .. } if command.name == "wakeup"),
    );
    let channel = channel_with(&[&alice], Arc::new(policy)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });

    let reply = actor.execute_command("help", "").await.expect("/help runs");
    assert_eq!(reply.visibility, CommandVisibility::Private, "/help answers the issuer alone");
    let text = reply.output.expect("/help answers here");
    assert!(text.contains("/help"), "{text}");
    assert!(!text.contains("/wakeup"), "a hidden command is not listed: {text}");
    let names: Vec<String> = actor.visible_commands().await.into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["help"]);

    let error = actor.execute_command("wakeup", "").await.expect_err("hidden");
    assert!(
        matches!(command_error(&error), CommandError::Unknown(_)),
        "a hidden command looks unknown: {error}"
    );
    let error = actor.execute_command("help", "wakeup").await.expect_err("hidden");
    assert!(
        matches!(command_error(&error), CommandError::Unknown(_)),
        "/help does not describe it either: {error}"
    );

    let quiet = time::timeout(Duration::from_millis(200), events.recv()).await;
    assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
    assert!(channel.history().await.is_empty(), "commands never enter the history");
    running.abort();
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

    let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
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
            assert_eq!(command, "wakeup");
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
    let error = actor.execute_command("wakeup", "").await.expect_err("waking is refused");
    assert!(matches!(command_error(&error), CommandError::Forbidden { .. }), "{error}");
    assert!(is_forbidden(&error), "{error}");

    let refuse_running = Policy(
        |action| !matches!(action, ActionType::ExecuteCommand { command, .. } if command.name == "wakeup"),
    );
    let channel = channel_with(&[&alice], Arc::new(refuse_running)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup", "").await.expect_err("running it is refused");
    assert!(
        matches!(command_error(&error), CommandError::Forbidden { .. }),
        "visible but not runnable: {error}"
    );

    let channel = channel_with(&[&alice], Arc::new(AllowAllPolicy::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup", "@nobody").await.expect_err("no such agent");
    assert!(matches!(command_error(&error), CommandError::InvalidArguments { .. }), "{error}");
    let error = actor.execute_command("", "").await.expect_err("empty");
    assert!(matches!(command_error(&error), CommandError::Empty), "{error}");
    let error = actor.execute_command("nonsense", "").await.expect_err("unknown");
    assert!(matches!(command_error(&error), CommandError::Unknown(_)), "{error}");
}

#[tokio::test]
async fn wakeup_wakes_the_agent_named_and_refuses_an_ambiguous_name() {
    let (channel, alice, sleepers) = channel_with_sleepers(&["Sassy", "Dr. Who", "Dr-Who"]).await;
    let actor = channel.user_actor(alice).unwrap();

    let reply = actor.execute_command("wakeup", "@Sassy").await.expect("found regardless of case");
    assert_eq!(reply.output.as_deref(), Some("Woke Sassy."));
    assert!(sleepers[0].woken() && !sleepers[1].woken(), "it alone");
    let reply =
        actor.execute_command("wakeup", "Dr. Who").await.expect("by its name, spaces and all");
    assert_eq!(reply.output.as_deref(), Some("Woke Dr. Who."));

    let error = actor.execute_command("wakeup", "@dr-who").await.expect_err("ambiguous");
    assert!(
        matches!(command_error(&error), CommandError::InvalidArguments { reason, .. } if reason.contains("Dr-Who and Dr. Who")),
        "{error}"
    );
    assert!(!sleepers[2].woken(), "nobody is picked");
}

/// Runs `/nudge` with `args` in `channel` as alice.
async fn nudge(channel: &ChannelLoop, alice: UserId, args: &str) -> CommandResult<CommandOutput> {
    use crate::command::nudge_command::NudgeCommandProvider;
    use crate::command::ChannelCommandProvider;
    let executor = CommandExecutor::empty(Arc::new(AllowAllPolicy::new()));
    let issuer = channel.user_actor(alice).expect("alice takes part");
    let context = CommandContext { issuer: &issuer, channel, executor: &executor };
    NudgeCommandProvider.execute("nudge", args, &context).await
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
        (finished.security.caller_id, finished.security.initiator_id),
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
