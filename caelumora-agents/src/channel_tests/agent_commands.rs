//! The commands an agent carries out itself in a running channel (issue
//! 269): an agent's, which move with the agents' logic (issue 370).

use super::*;
use crate::tool::turn_end::TURN_END_KIND;
use caelumora_api::command::CommandError;
use caelumora_domain::channel::ThreadId;
use caelumora_domain::tool::testing::make_tool_call;
use caelumora_domain::transcript::TranscriptEntry;
use caelumora_kernel::command::{CommandContext, CommandReply};

/// A running channel with alice and an agent, `helper`, that carries
/// out `/mark`, idle after it answered alice's "hello" - or asleep, if
/// it said good-bye.
async fn with_a_marking_agent(asleep: bool) -> Stuck {
    with_a_marking_agent_under(Arc::new(AllowAllPolicy::new()), asleep).await
}

/// `with_a_marking_agent`, under `access_policy`.
async fn with_a_marking_agent_under(access_policy: Arc<dyn AccessPolicy>, asleep: bool) -> Stuck {
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::message::MessageRole;
    use caelumora_domain::tool::ToolCall;
    let stop = ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({}),
        ..make_tool_call("stop")
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
            ComponentDescriptor::new("stop", TURN_END_KIND),
        ],
    };
    let mut stuck =
        with_an_agent_under(access_policy, Arc::new(Scripted::new([hi])), components).await;
    answered(&mut stuck.events, stuck.agent).await;
    settled(&stuck.helper, if asleep { ActorState::Sleeping } else { ActorState::Idle }).await;
    stuck
}

#[tokio::test]
async fn an_agent_carries_out_a_command_in_a_turn_of_its_own_and_answers_the_issuer_alone() {
    let Stuck { channel, context, agent, helper, running, .. } = with_a_marking_agent(false).await;
    let line = ThreadId::line(channel.id());
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();

    // By its short form (issue 560): what is told and recorded is its name.
    let reply = actor.execute_command(None, "mk  @helper twice").await.expect("sent to the agent");
    assert!(
        reply.output.is_none() && reply.span_id.is_some(),
        "answered later, on the stream: {reply:?}"
    );
    assert_eq!(reply.command, "mark", "answered by its name");
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
        (answer.context.sender_id, answer.context.initiator_id),
        (ActorId::Agent(agent), ActorId::User(alice)),
        "carried out by the agent, on her word"
    );

    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        ["marked twice"],
        "what it staged is applied"
    );
    assert!(state.turns().next().is_none(), "the turn is over");
    assert!(
        matches!(state.transcript.last(line).map(|node| &node.entry), Some(TranscriptEntry::TurnFinished { turn_id: t, .. }) if Some(*t) == turn_id),
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

/// A command its issuer may see but not ask for is rejected at the channel's
/// door (issue 516): her actor finds the agent by what she sees and asks
/// nothing more; the channel asks once, as it admits the request, and tells
/// her alone that it was not let in. The agent hears nothing of it.
#[tokio::test]
async fn a_command_its_issuer_may_see_but_not_ask_for_is_rejected_at_the_door() {
    use caelumora_domain::actor::Rejection;
    let seen_not_asked = Policy(|action| !matches!(action, ActionType::ExecuteAgentCommand { .. }));
    let Stuck { channel, context, agent, running, .. } =
        with_a_marking_agent_under(Arc::new(seen_not_asked), false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();
    let before = channel.history_entries().await.len();

    let reply =
        actor.execute_command(None, "mark @helper twice").await.expect("sent: the channel decides");
    assert!(reply.output.is_none() && reply.span_id.is_some(), "{reply:?}");
    let rejection = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = watch.recv().await.expect("the channel is running");
            if matches!(envelope.event, ActorEvent::EventRejected { .. }) {
                break envelope;
            }
        }
    })
    .await
    .expect("told that it was not let in");
    let ActorEvent::EventRejected { rejected, reason, .. } = rejection.event else {
        unreachable!()
    };
    assert_eq!(
        (rejected.as_str(), reason),
        ("CommandRequested", Rejection::Forbidden),
        "not hers to ask for"
    );
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(
        state.notes.current().is_empty() && state.turns().next().is_none(),
        "nothing reached the agent"
    );
    let recorded = channel.history_entries().await.split_off(before);
    assert!(
        matches!(recorded.as_slice(), [HistoryEntry::EventRejected { .. }]),
        "in the channel's history only that it was not let in, to her (issue 417): {recorded:?}"
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
    let ends_at = channel.stored_agent_channel_state(agent).await.unwrap().transcript.end();

    actor.execute_command(None, "mark @helper fail").await.expect("sent to the agent");
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
    assert!(state.turns().next().is_none());
    assert_eq!(
        state.transcript.end(),
        ends_at,
        "nothing recorded in its transcript, on whichever branch: nothing happened"
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

    let unnamed = error(actor.execute_command(None, "mark twice").await);
    assert!(
        matches!(as_command_error(&unnamed), CommandError::InvalidArguments { reason, .. } if reason.contains("@helper") && reason.contains("@all")),
        "{unnamed}"
    );
    let named = actor
        .execute_command(None, "mark @Helper twice")
        .await
        .expect("named regardless of case (issue 277)");
    assert!(named.span_id.is_some(), "it is asked: {named:?}");
    let nobody = error(actor.execute_command(None, "mark @nobody twice").await);
    assert!(
        matches!(as_command_error(&nobody), CommandError::InvalidArguments { reason, .. } if reason.contains("@helper")),
        "{nobody}"
    );
    let unknown = error(actor.execute_command(None, "nonsense @helper").await);
    assert!(
        matches!(as_command_error(&unknown), CommandError::Unknown(name) if name == "nonsense"),
        "{unknown}"
    );
    let all =
        actor.execute_command(None, "mark @all").await.expect("every agent that carries it out");
    assert!(all.span_id.is_some() && all.output.is_none(), "all of them asked: {all:?}");
    let channel_wide = actor
        .execute_command(None, "mark @channel")
        .await
        .expect("a group handle, as a mention (issue 401)");
    assert!(channel_wide.span_id.is_some(), "all of them asked: {channel_wide:?}");
    running.abort();
}

/// Issue 602: a command is issued in the thread its issuer names, of an agent that takes part
/// there: in a thread its agent takes no part in, it is carried out by nobody there; taken into
/// the thread, the agent is asked.
#[tokio::test]
async fn a_command_of_an_agent_is_issued_in_a_thread_it_takes_part_in() {
    use caelumora_domain::channel::ThreadOrigin;
    let Stuck { channel, context, agent, running, .. } = with_a_marking_agent(false).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let origin = ThreadOrigin { thread: ThreadId::line(channel.id()), message: MessageId::new() };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");

    let nobody = actor.execute_command(Some(thread), "mark @helper").await.expect_err("nobody");
    assert!(
        matches!(as_command_error(&nobody), CommandError::Unknown(name) if name == "mark"),
        "{nobody}"
    );
    let helper = ActorId::Agent(agent);
    channel.take_into_thread_by_procedure(&context, thread, helper).await.expect("taken in");
    let asked = actor.execute_command(Some(thread), "mark @helper").await.expect("asked there");
    assert!(asked.span_id.is_some(), "{asked:?}");
    running.abort();
}

#[tokio::test]
async fn a_sleeping_agent_carries_out_a_command_and_sleeps_on() {
    let Stuck { channel, context, agent, helper, running, .. } = with_a_marking_agent(true).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let mut watch = actor.watch();

    actor.execute_command(None, "mark @helper asleep").await.expect("sent to the agent");
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
    use caelumora_kernel::command::ChannelCommand;
    let Stuck { channel, context, running, .. } = with_a_marking_agent(false).await;
    // This channel's session has no commands of its own: `/help` is run
    // as it would be among them.
    let executor = CommandExecutor::empty(Arc::new(AllowAllPolicy::new()));
    let issuer = channel.user_actor(alice_of(&context)).expect("alice takes part");
    let thread = ThreadId::line(channel.id());
    let help = CommandContext { issuer: &issuer, channel: &channel, executor: &executor, thread };

    let all = HelpCommand::new().execute("", &help).await.expect("/help runs").text;
    assert!(all.contains("`/mark @agent [text]` (also `/mk`) — Marks. (@helper)"), "{all}");
    let one = HelpCommand::new().execute("mark", &help).await.expect("/help runs").text;
    assert!(one.contains("Carried out by @helper"), "{one}");
    let short = HelpCommand::new().execute("mk", &help).await.expect("/help runs").text;
    assert_eq!(short, one, "described by its short form as by its name");
    running.abort();
}
