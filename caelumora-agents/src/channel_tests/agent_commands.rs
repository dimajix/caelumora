//! The commands an agent carries out itself in a running channel (issue
//! 269): an agent's, which move with the agents' logic (issue 370).

use super::*;
use crate::tool::stop_tool::STOP_TOOL_KIND;
use caelumora_model::chain::Node;
use caelumora_model::transcript::TranscriptEntry;
use caelumora_runtime::command::{CommandContext, CommandError, CommandReply};
use std::sync::Mutex;

/// A running channel with alice and an agent, `helper`, that carries
/// out `/mark`, idle after it answered alice's "hello" - or asleep, if
/// it said good-bye.
async fn with_a_marking_agent(asleep: bool) -> Stuck {
    with_a_marking_agent_under(Arc::new(AllowAllPolicy::new()), asleep).await
}

/// `with_a_marking_agent`, under `access_policy`.
async fn with_a_marking_agent_under(access_policy: Arc<dyn AccessPolicy>, asleep: bool) -> Stuck {
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::message::MessageRole;
    use caelumora_model::tool::{ToolCall, ToolVisibility};
    let stop = ToolCall {
        id: Some("1".to_string()),
        function_name: "stop".to_string(),
        arguments: serde_json::json!({}),
        visibility: ToolVisibility::SHOWN,
        presentation: Default::default(),
    };
    let hi = Message {
        role: MessageRole::Assistant,
        content: Some("Hi.".to_string()),
        tool_calls: if asleep { vec![stop] } else { Vec::new() },
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig {
        component_nodes: vec![
            ComponentDescriptor::new("marking", "test_marking"),
            ComponentDescriptor::new("stop", STOP_TOOL_KIND),
        ],
    };
    let mut stuck =
        with_an_agent_under(access_policy, Arc::new(Scripted(Mutex::new([hi].into()))), components)
            .await;
    answered(&mut stuck.events, stuck.agent).await;
    settled(&stuck.helper, if asleep { ActorState::Sleeping } else { ActorState::Idle }).await;
    stuck
}

#[tokio::test]
async fn an_agent_carries_out_a_command_in_a_turn_of_its_own_and_answers_the_issuer_alone() {
    let Stuck { channel, context, agent, helper, running, .. } = with_a_marking_agent(false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();

    let reply = actor.execute_command("mark", "@helper twice").await.expect("sent to the agent");
    assert!(
        reply.output.is_none() && reply.span_id.is_some(),
        "answered later, on the stream: {reply:?}"
    );
    let (started, states, answer) = command_events(&mut watch).await;
    let started = started.expect("she is told it began (issue 299)");
    let ActorEvent::CommandStarted { command, args, to, turn_id: begun } = started.event else {
        unreachable!()
    };
    assert_eq!((command.as_str(), args.as_str(), to), ("mark", "twice", ActorId::User(alice)));
    assert_eq!(
        (started.context.sender_id, started.context.span_id),
        (ActorId::Agent(agent), reply.span_id.unwrap()),
        "by the agent, in the span of the request"
    );
    assert_eq!(states, [ActorState::CarryingOutCommand], "shown carrying it out meanwhile");
    let ActorEvent::CommandExecuted { command, args, output, to, turn_id } = answer.event else {
        panic!("carried out, not {:?}", answer.event)
    };
    assert_eq!(turn_id, Some(begun), "the answer ends what began");
    assert_eq!(
        (command.as_str(), args.as_str(), output.as_str(), to),
        ("mark", "twice", "Marked.", Some(ActorId::User(alice))),
        "without the handle, to her alone"
    );
    assert_eq!(answer.context.sender_id, ActorId::Agent(agent));
    assert_eq!(answer.context.span_id, reply.span_id.unwrap(), "in the span of the request");
    assert_eq!(
        (answer.security.caller_id, answer.security.initiator_id),
        (ActorId::Agent(agent), ActorId::User(alice)),
        "carried out by the agent, on her word"
    );

    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        ["marked twice"],
        "what it staged is applied"
    );
    assert!(state.turn.is_none(), "the turn is over");
    assert!(
        matches!(state.transcript.last().map(|node| &node.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if Some(*t) == turn_id),
        "its transcript records the turn's end"
    );
    let recorded = channel.history_entries().await.into_iter()
        .any(|item| matches!(item, HistoryEntry::CommandExecuted { issuer, actor, ref command, .. } if issuer == ActorId::User(alice) && actor == ActorId::Agent(agent) && command == "mark"));
    assert!(recorded, "the channel's history records what was carried out, and on whose word");
    assert_eq!(
        told_states(&mut watch, agent, ActorState::Idle).await,
        [ActorState::Idle],
        "idle again after, and no answer begun"
    );
    assert_eq!(helper.state(), ActorState::Idle);
    running.abort();
}

#[tokio::test]
async fn a_command_its_issuer_may_see_but_not_ask_for_is_refused_by_the_agent_alone() {
    let seen_not_asked = Policy(|action| !matches!(action, ActionType::ExecuteAgentCommand { .. }));
    let Stuck { channel, context, agent, running, .. } =
        with_a_marking_agent_under(Arc::new(seen_not_asked), false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();
    let before = channel.history_entries().await.len();

    // Her actor finds the agent by what she sees and asks nothing more:
    // the agent decides, once, as it carries it out (issue 304).
    let reply =
        actor.execute_command("mark", "@helper twice").await.expect("sent: the agent decides");
    assert!(reply.output.is_none() && reply.span_id.is_some(), "{reply:?}");
    let (started, _, answer) = command_events(&mut watch).await;
    assert!(started.is_none(), "never begun");
    let ActorEvent::CommandFailed { command, to, turn_id, .. } = answer.event else {
        panic!("refused, not {:?}", answer.event)
    };
    assert_eq!(
        (command.as_str(), to, turn_id),
        ("mark", ActorId::User(alice), None),
        "to her alone"
    );
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(
        state.notes.current().is_empty() && state.turn.is_none(),
        "nothing staged, no turn left"
    );
    let recorded = channel.history_entries().await.split_off(before);
    assert!(
        matches!(recorded.as_slice(), [HistoryEntry::CommandFailed { to, .. }] if *to == ActorId::User(alice)),
        "in the channel's history only that it was refused, to her (issue 417): {recorded:?}"
    );
    running.abort();
}

#[tokio::test]
async fn a_command_that_fails_is_told_to_the_issuer_and_leaves_nothing_behind() {
    let Stuck { channel, context, agent, running, .. } = with_a_marking_agent(false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();
    let before = channel.history_entries().await.len();

    actor.execute_command("mark", "@helper fail").await.expect("sent to the agent");
    let (started, _, answer) = command_events(&mut watch).await;
    let Some(ActorEvent::CommandStarted { turn_id: begun, .. }) = started.map(|e| e.event) else {
        panic!("it began")
    };
    let ActorEvent::CommandFailed { command, reason, to, turn_id } = answer.event else {
        panic!("failed, not {:?}", answer.event)
    };
    assert_eq!(
        (command.as_str(), to, turn_id),
        ("mark", ActorId::User(alice), Some(begun)),
        "ending what began"
    );
    assert!(reason.contains("it was told to"), "{reason}");

    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.notes.current().is_empty(), "nothing it staged is applied");
    assert!(state.turn.is_none());
    let ended = |node: &Node<TranscriptEntry>| matches!(node.entry, TranscriptEntry::TurnFinished { turn_id, .. } | TranscriptEntry::TurnGivenUp { turn_id, .. } if turn_id == begun);
    assert!(
        !state.transcript.all_entries().any(ended),
        "no turn's end in its transcript: nothing happened"
    );
    let recorded = channel.history_entries().await.split_off(before);
    assert!(
        matches!(recorded.as_slice(), [HistoryEntry::CommandFailed { turn_id: Some(turn_id), .. }] if *turn_id == begun),
        "in the channel's history only that it failed (issue 417): {recorded:?}"
    );
    for (reader, told) in [(ActorId::User(alice), 1), (ActorId::User(UserId::new()), 0)] {
        let seen = channel
            .visible_entries(reader)
            .await
            .iter()
            .filter(|e| matches!(e, HistoryEntry::CommandFailed { .. }))
            .count();
        assert_eq!(seen, told, "told to her alone: {reader}");
    }
    running.abort();
}

#[tokio::test]
async fn a_command_of_an_agent_names_its_agent() {
    let Stuck { channel, context, running, .. } = with_a_marking_agent(false).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let error = |result: Result<CommandReply>| result.expect_err("refused");

    let unnamed = error(actor.execute_command("mark", "twice").await);
    assert!(
        matches!(command_error(&unnamed), CommandError::InvalidArguments { reason, .. } if reason.contains("@helper") && reason.contains("@all")),
        "{unnamed}"
    );
    let named = actor
        .execute_command("mark", "@Helper twice")
        .await
        .expect("named regardless of case (issue 277)");
    assert!(named.span_id.is_some(), "it is asked: {named:?}");
    let nobody = error(actor.execute_command("mark", "@nobody twice").await);
    assert!(
        matches!(command_error(&nobody), CommandError::InvalidArguments { reason, .. } if reason.contains("@helper")),
        "{nobody}"
    );
    let unknown = error(actor.execute_command("nonsense", "@helper").await);
    assert!(
        matches!(command_error(&unknown), CommandError::Unknown(name) if name == "nonsense"),
        "{unknown}"
    );
    let all = actor.execute_command("mark", "@all").await.expect("every agent that carries it out");
    assert!(all.span_id.is_some() && all.output.is_none(), "all of them asked: {all:?}");
    let channel_wide = actor
        .execute_command("mark", "@channel")
        .await
        .expect("a group handle, as a mention (issue 401)");
    assert!(channel_wide.span_id.is_some(), "all of them asked: {channel_wide:?}");
    running.abort();
}

#[tokio::test]
async fn a_sleeping_agent_carries_out_a_command_and_sleeps_on() {
    let Stuck { channel, context, agent, helper, running, .. } = with_a_marking_agent(true).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let mut watch = actor.watch();

    actor.execute_command("mark", "@helper asleep").await.expect("sent to the agent");
    let (started, states, answer) = command_events(&mut watch).await;
    assert!(
        matches!(answer.event, ActorEvent::CommandExecuted { .. }),
        "carried out asleep: {:?}",
        answer.event
    );
    assert!(started.is_some(), "she is told it began, asleep or not");
    assert!(states.is_empty(), "shown asleep all along (issue 299): {states:?}");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.notes.current().len(), 1);
    assert_eq!(helper.state(), ActorState::Sleeping, "a command wakes nobody");
    running.abort();
}

#[tokio::test]
async fn help_lists_the_commands_of_the_agents_with_their_handles() {
    use caelumora_runtime::command::ChannelCommandProvider;
    let Stuck { channel, context, running, .. } = with_a_marking_agent(false).await;
    // This channel's session has no commands of its own: `/help` is run
    // as it would be among them.
    let executor = CommandExecutor::empty(Arc::new(AllowAllPolicy::new()));
    let issuer = channel.user_actor(alice_of(&context)).expect("alice takes part");
    let help = CommandContext { issuer: &issuer, channel: &channel, executor: &executor };

    let all = HelpCommandProvider.execute("help", "", &help).await.expect("/help runs").text;
    assert!(all.contains("`/mark @agent [text]` — Marks. (@helper)"), "{all}");
    let one = HelpCommandProvider.execute("help", "mark", &help).await.expect("/help runs").text;
    assert!(one.contains("Carried out by @helper"), "{one}");
    running.abort();
}
