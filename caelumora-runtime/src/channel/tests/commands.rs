//! Commands and nudges in a running channel.

use super::*;

/// Decides by a rule over the action alone, to refuse one thing and
/// allow the rest.
#[derive(Debug)]
struct Policy(fn(ActionType<'_>) -> bool);

#[async_trait::async_trait]
impl SecurityManager for Policy {
    async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
        (self.0)(action)
    }
}

fn command_error(error: &anyhow::Error) -> &crate::command::CommandError {
    error.downcast_ref().unwrap_or_else(|| panic!("expected a CommandError, got {error:#}"))
}

#[tokio::test]
async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
    let alice = user("alice");
    // `/wakeup` is hidden from everybody.
    let policy = Policy(|action| !matches!(action, ActionType::ShowCommand { command, .. } if command.name == "wakeup"));
    let channel = channel_with(&[&alice], Arc::new(policy)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let reply = actor.execute_command("help", "").await.expect("/help runs");
    assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Private, "/help answers the issuer alone");
    let text = reply.output.expect("/help answers here");
    assert!(text.contains("/help"), "{text}");
    assert!(!text.contains("/wakeup"), "a hidden command is not listed: {text}");
    let names: Vec<String> = actor.visible_commands().await.into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["help"]);

    let error = actor.execute_command("wakeup", "").await.expect_err("hidden");
    assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "a hidden command looks unknown: {error}");
    let error = actor.execute_command("help", "wakeup").await.expect_err("hidden");
    assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "/help does not describe it either: {error}");

    let quiet = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await;
    assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
    assert!(channel.history().await.is_empty(), "commands never enter the history");
    running.abort();
}

#[tokio::test]
async fn wakeup_is_announced_to_the_channel_in_the_issuers_name() {
    let alice = user("alice");
    let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let mut events = channel.observe();
    let mut following = actor.watch().events;
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
    assert!(matches!(following.try_recv().map(|e| e.event), Ok(ActorEvent::CommandExecuted { .. })), "the issuer sees it from its own actor (issue 220)");
    assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Channel, "the answer goes to the channel");
    assert_eq!(reply.output.as_deref(), Some("No agent was asleep."), "and back to the issuer, whom the channel does not route it to");

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
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
    let alice = user("alice");
    let refuse_waking = Policy(|action| !matches!(action, ActionType::WakeChannel(_)));
    let channel = channel_with(&[&alice], Arc::new(refuse_waking)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup", "").await.expect_err("waking is refused");
    assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "{error}");
    assert!(crate::error::is_forbidden(&error), "{error}");

    let refuse_running = Policy(|action| !matches!(action, ActionType::ExecuteCommand { command, .. } if command.name == "wakeup"));
    let channel = channel_with(&[&alice], Arc::new(refuse_running)).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup", "").await.expect_err("running it is refused");
    assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "visible but not runnable: {error}");

    let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
    let actor = channel.user_actor(alice.id).unwrap();
    let error = actor.execute_command("wakeup", "@nobody").await.expect_err("no such agent");
    assert!(matches!(command_error(&error), crate::command::CommandError::InvalidArguments { .. }), "{error}");
    let error = actor.execute_command("", "").await.expect_err("empty");
    assert!(matches!(command_error(&error), crate::command::CommandError::Empty), "{error}");
    let error = actor.execute_command("nonsense", "").await.expect_err("unknown");
    assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "{error}");
}

#[tokio::test]
async fn wakeup_wakes_the_agent_named_and_refuses_an_ambiguous_name() {
    let (channel, alice, sleepers) = channel_with_sleepers(&["Sassy", "Dr. Who", "Dr-Who"]).await;
    let actor = channel.user_actor(alice).unwrap();

    let reply = actor.execute_command("wakeup", "@Sassy").await.expect("found regardless of case");
    assert_eq!(reply.output.as_deref(), Some("Woke Sassy."));
    assert!(sleepers[0].woken.load(Ordering::SeqCst) && !sleepers[1].woken.load(Ordering::SeqCst), "it alone");
    let reply = actor.execute_command("wakeup", "Dr. Who").await.expect("by its name, spaces and all");
    assert_eq!(reply.output.as_deref(), Some("Woke Dr. Who."));

    let error = actor.execute_command("wakeup", "@dr-who").await.expect_err("ambiguous");
    assert!(matches!(command_error(&error), crate::command::CommandError::InvalidArguments { reason, .. } if reason.contains("Dr-Who and Dr. Who")), "{error}");
    assert!(!sleepers[2].woken.load(Ordering::SeqCst), "nobody is picked");
}

/// A running channel with alice and an agent, `helper`, that carries
/// out `/mark`, idle after it answered alice's "hello" - or asleep, if
/// it said good-bye.
async fn with_a_marking_agent(asleep: bool) -> Stuck {
    with_a_marking_agent_under(Arc::new(DummySecurityManager::new()), asleep).await
}

/// `with_a_marking_agent`, under `security`.
async fn with_a_marking_agent_under(security: Arc<dyn SecurityManager>, asleep: bool) -> Stuck {
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::message::MessageRole;
    use caelumora_model::tool::{ToolCall, ToolVisibility};
    let stop = ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let hi = Message {
        role: MessageRole::Assistant,
        content: Some("Hi.".to_string()),
        tool_calls: if asleep { vec![stop] } else { Vec::new() },
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![
        ComponentDescriptor::new("marking", "test_marking"),
        ComponentDescriptor::new("stop", crate::tool::stop_tool::STOP_TOOL_KIND),
    ] };
    let mut stuck = with_an_agent_under(security, Arc::new(Scripted(std::sync::Mutex::new([hi].into()))), components).await;
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
    assert!(reply.output.is_none() && reply.span_id.is_some(), "answered later, on the stream: {reply:?}");
    let (started, states, answer) = command_events(&mut watch).await;
    let started = started.expect("she is told it began (issue 299)");
    let ActorEvent::CommandStarted { command, args, to, turn_id: begun } = started.event else { unreachable!() };
    assert_eq!((command.as_str(), args.as_str(), to), ("mark", "twice", ActorId::User(alice)));
    assert_eq!((started.context.sender_id, started.context.span_id), (ActorId::Agent(agent), reply.span_id.unwrap()), "by the agent, in the span of the request");
    assert_eq!(states, [ActorState::CarryingOutCommand], "shown carrying it out meanwhile");
    let ActorEvent::CommandExecuted { command, args, output, to, turn_id } = answer.event else { panic!("carried out, not {:?}", answer.event) };
    assert_eq!(turn_id, Some(begun), "the answer ends what began");
    assert_eq!((command.as_str(), args.as_str(), output.as_str(), to), ("mark", "twice", "Marked.", Some(ActorId::User(alice))), "without the handle, to her alone");
    assert_eq!(answer.context.sender_id, ActorId::Agent(agent));
    assert_eq!(answer.context.span_id, reply.span_id.unwrap(), "in the span of the request");
    assert_eq!((answer.security.caller_id, answer.security.initiator_id), (ActorId::Agent(agent), ActorId::User(alice)), "carried out by the agent, on her word");

    let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["marked twice"], "what it staged is applied");
    assert!(state.turn.is_none(), "the turn is over");
    let ends: Vec<String> = state.history.items().iter().filter_map(|item| match item {
        HistoryEntry::CommandExecuted { turn_id: t, command, .. } if Some(*t) == turn_id => Some(format!("/{command}")),
        HistoryEntry::TurnFinished { turn_id: t, .. } if Some(*t) == turn_id => Some("finished".to_string()),
        _ => None,
    }).collect();
    assert_eq!(ends, ["/mark", "finished"], "its history records the command before the turn's end");
    let recorded = channel.state.lock().await.history.items().into_iter()
        .any(|item| matches!(item, HistoryEntry::CommandExecuted { issuer, actor, ref command, .. } if issuer == ActorId::User(alice) && actor == ActorId::Agent(agent) && command == "mark"));
    assert!(recorded, "the channel's history records what was carried out, and on whose word");
    assert_eq!(told_states(&mut watch.events, agent, ActorState::Idle).await, [ActorState::Idle], "idle again after, and no answer begun");
    assert_eq!(helper.state(), ActorState::Idle);
    running.abort();
}

#[tokio::test]
async fn a_command_its_issuer_may_see_but_not_ask_for_is_refused_by_the_agent_alone() {
    let seen_not_asked = Policy(|action| !matches!(action, ActionType::ExecuteAgentCommand { .. }));
    let Stuck { channel, context, agent, running, .. } = with_a_marking_agent_under(Arc::new(seen_not_asked), false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();
    let before = channel.state.lock().await.history.items().len();

    // Her actor finds the agent by what she sees and asks nothing more:
    // the agent decides, once, as it carries it out (issue 304).
    let reply = actor.execute_command("mark", "@helper twice").await.expect("sent: the agent decides");
    assert!(reply.output.is_none() && reply.span_id.is_some(), "{reply:?}");
    let (started, _, answer) = command_events(&mut watch).await;
    assert!(started.is_none(), "never begun");
    let ActorEvent::CommandFailed { command, to, turn_id, .. } = answer.event else { panic!("refused, not {:?}", answer.event) };
    assert_eq!((command.as_str(), to, turn_id), ("mark", ActorId::User(alice), None), "to her alone");
    let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    assert!(state.notes.current().is_empty() && state.turn.is_none(), "nothing staged, no turn left");
    assert_eq!(channel.state.lock().await.history.items().len(), before, "nothing in the channel's history");
    running.abort();
}

#[tokio::test]
async fn a_command_that_fails_is_told_to_the_issuer_and_leaves_nothing_behind() {
    let Stuck { channel, context, agent, running, .. } = with_a_marking_agent(false).await;
    let alice = alice_of(&context);
    let actor = channel.user_actor(alice).unwrap();
    let mut watch = actor.watch();
    let before = channel.state.lock().await.history.items().len();

    actor.execute_command("mark", "@helper fail").await.expect("sent to the agent");
    let (started, _, answer) = command_events(&mut watch).await;
    let Some(ActorEvent::CommandStarted { turn_id: begun, .. }) = started.map(|e| e.event) else { panic!("it began") };
    let ActorEvent::CommandFailed { command, reason, to, turn_id } = answer.event else { panic!("failed, not {:?}", answer.event) };
    assert_eq!((command.as_str(), to, turn_id), ("mark", ActorId::User(alice), Some(begun)), "ending what began");
    assert!(reason.contains("it was told to"), "{reason}");

    let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    assert!(state.notes.current().is_empty(), "nothing it staged is applied");
    assert!(state.turn.is_none());
    assert!(!state.history.items().iter().any(|item| matches!(item, HistoryEntry::CommandExecuted { .. } | HistoryEntry::TurnGivenUp { .. })), "no command, no turn given up: nothing happened");
    assert_eq!(channel.state.lock().await.history.items().len(), before, "nor in the channel's");
    running.abort();
}

#[tokio::test]
async fn a_command_of_an_agent_names_its_agent() {
    let Stuck { channel, context, running, .. } = with_a_marking_agent(false).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let error = |result: Result<crate::command::CommandReply>| result.expect_err("refused");

    let unnamed = error(actor.execute_command("mark", "twice").await);
    assert!(matches!(command_error(&unnamed), crate::command::CommandError::InvalidArguments { reason, .. } if reason.contains("@helper") && reason.contains("@all")), "{unnamed}");
    let named = actor.execute_command("mark", "@Helper twice").await.expect("named regardless of case (issue 277)");
    assert!(named.span_id.is_some(), "it is asked: {named:?}");
    let nobody = error(actor.execute_command("mark", "@nobody twice").await);
    assert!(matches!(command_error(&nobody), crate::command::CommandError::InvalidArguments { reason, .. } if reason.contains("@helper")), "{nobody}");
    let unknown = error(actor.execute_command("nonsense", "@helper").await);
    assert!(matches!(command_error(&unknown), crate::command::CommandError::Unknown(name) if name == "nonsense"), "{unknown}");
    let all = actor.execute_command("mark", "@all").await.expect("every agent that carries it out");
    assert!(all.span_id.is_some() && all.output.is_none(), "all of them asked: {all:?}");
    running.abort();
}

#[tokio::test]
async fn a_sleeping_agent_carries_out_a_command_and_sleeps_on() {
    let Stuck { channel, context, agent, helper, running, .. } = with_a_marking_agent(true).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let mut watch = actor.watch();

    actor.execute_command("mark", "@helper asleep").await.expect("sent to the agent");
    let (started, states, answer) = command_events(&mut watch).await;
    assert!(matches!(answer.event, ActorEvent::CommandExecuted { .. }), "carried out asleep: {:?}", answer.event);
    assert!(started.is_some(), "she is told it began, asleep or not");
    assert!(states.is_empty(), "shown asleep all along (issue 299): {states:?}");
    let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    assert_eq!(state.notes.current().len(), 1);
    assert_eq!(helper.state(), ActorState::Sleeping, "a command wakes nobody");
    running.abort();
}

/// Runs `/nudge` with `args` in `channel` as alice.
async fn nudge(channel: &ChannelLoop, alice: UserId, args: &str) -> crate::command::CommandResult<caelumora_model::command::CommandOutput> {
    use crate::command::ChannelCommandProvider;
    use crate::command::nudge_command::NudgeCommandProvider;
    let executor = CommandExecutor::empty(Arc::new(DummySecurityManager::new()));
    let issuer = channel.user_actor(alice).expect("alice takes part");
    let context = crate::command::CommandContext { issuer: &issuer, channel, executor: &executor };
    NudgeCommandProvider.execute("nudge", args, &context).await
}

/// The next end of a turn of `agent` on `events`, with its envelope.
async fn turn_finished(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: caelumora_model::agent::AgentId) -> Envelope {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnFinished { .. }) {
                break envelope;
            }
        }
    }).await.expect("the agent answers")
}

#[tokio::test]
async fn a_nudged_agent_responds_in_the_name_of_who_nudged_it_and_the_history_keeps_nothing_of_the_nudge() {
    let model = Scripted(std::sync::Mutex::new([says("Hi."), says("Again.")].into()));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), Default::default()).await;
    let alice = alice_of(&context);
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let mut following = channel.user_actor(alice).unwrap().watch().events;

    let output = nudge(&channel, alice, "@helper").await.expect("/nudge runs");
    assert_eq!((output.text.as_str(), output.visibility), ("Asked helper to respond.", caelumora_model::command::CommandVisibility::Private), "told to the issuer alone");

    let finished = turn_finished(&mut events, agent).await;
    let ActorEvent::TurnFinished { messages, .. } = &finished.event else { unreachable!() };
    assert_eq!(messages.iter().map(|m| m.content.as_deref()).collect::<Vec<_>>(), [Some("Again.")], "it responds without being told anything new");
    assert_eq!((finished.security.caller_id, finished.security.initiator_id), (ActorId::Agent(agent), ActorId::User(alice)), "by the agent, on her word");
    let said: Vec<_> = channel.history().await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["hello", "Hi.", "Again."], "the nudge leaves nothing in the history");
    while let Ok(envelope) = following.try_recv() {
        assert!(!matches!(envelope.event, ActorEvent::NudgeRequested { .. } | ActorEvent::ResponseRequested), "nor is it shown: {}", envelope.event.kind());
    }
    running.abort();
}

#[tokio::test]
async fn a_nudge_wakes_the_agent_it_names_and_ends_a_quiet_channel() {
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::tool::{ToolCall, ToolVisibility};
    let stop = ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let asleep = Message { tool_calls: vec![stop], ..says("Good night.") };
    let model = Scripted(std::sync::Mutex::new([asleep, says("Awake.")].into()));
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("stop", crate::tool::stop_tool::STOP_TOOL_KIND)] };
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), components).await;
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Sleeping).await;
    channel.cancel(&context).await.expect("permitted");

    nudge(&channel, alice_of(&context), "helper").await.expect("/nudge runs");

    let finished = turn_finished(&mut events, agent).await;
    let ActorEvent::TurnFinished { messages, .. } = &finished.event else { unreachable!() };
    assert_eq!(messages.iter().map(|m| m.content.as_deref()).collect::<Vec<_>>(), [Some("Awake.")], "woken and asked, though the channel was quiet");
    assert!(!channel.quiet.load(Ordering::SeqCst), "the channel is quiet no longer");
    running.abort();
}

#[tokio::test]
async fn an_agent_in_a_turn_is_not_nudged_into_a_second_one() {
    let Stuck { channel, context, agent, running, .. } = stuck_in_a_model_call().await;
    let alice = alice_of(&context);

    let output = nudge(&channel, alice, "@helper").await.expect("answered, not failed");
    assert_eq!(output.text, "helper is responding already.");
    let error = channel.user_actor(alice).unwrap().request_response(ActorId::Agent(agent)).await.expect_err("refused");
    assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::AlreadyResponding { .. })), "{error}");
    running.abort();
}

#[tokio::test]
async fn only_someone_else_in_the_channel_is_nudged() {
    let Stuck { channel, context, running, .. } = stuck_in_a_model_call().await;
    let alice = alice_of(&context);

    for args in ["", "@nobody", "alice"] {
        let error = nudge(&channel, alice, args).await.expect_err("names nobody else here");
        assert!(matches!(error, crate::command::CommandError::InvalidArguments { .. }), "{args:?}: {error}");
    }
    let actor = channel.user_actor(alice).unwrap();
    let error = actor.request_response(ActorId::User(alice)).await.expect_err("herself");
    assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NudgesItself { .. })), "{error}");
    let error = actor.request_response(ActorId::Agent(caelumora_model::agent::AgentId::new())).await.expect_err("a stranger");
    assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })), "{error}");
    running.abort();
}

#[tokio::test]
async fn a_nudged_user_is_shown_who_asks_them_to_respond() {
    let (alice, bob) = (user("alice"), user("bob"));
    let channel = channel_with(&[&alice, &bob], Arc::new(DummySecurityManager::new())).await;
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
    let mut his = channel.user_actor(bob.id).unwrap().watch().events;

    let output = nudge(&channel, alice.id, "Bob").await.expect("/nudge runs");
    assert_eq!(output.text, "Asked bob to respond.");

    let asked = tokio::time::timeout(Duration::from_secs(5), his.recv()).await.expect("he is asked").unwrap();
    assert!(matches!(asked.event, ActorEvent::ResponseRequested), "{}", asked.event.kind());
    assert_eq!(asked.context.sender_id, ActorId::User(alice.id), "by her");
    assert!(channel.history().await.is_empty(), "the history keeps nothing of it");
    running.abort();
}

#[tokio::test]
async fn help_lists_the_commands_of_the_agents_with_their_handles() {
    use crate::command::ChannelCommandProvider;
    let Stuck { channel, context, running, .. } = with_a_marking_agent(false).await;
    // This channel's session has no commands of its own: `/help` is run
    // as it would be among them.
    let executor = CommandExecutor::empty(Arc::new(DummySecurityManager::new()));
    let issuer = channel.user_actor(alice_of(&context)).expect("alice takes part");
    let help = crate::command::CommandContext { issuer: &issuer, channel: &channel, executor: &executor };

    let all = HelpCommandProvider.execute("help", "", &help).await.expect("/help runs").text;
    assert!(all.contains("`/mark @agent [text]` — Marks. (@helper)"), "{all}");
    let one = HelpCommandProvider.execute("help", "mark", &help).await.expect("/help runs").text;
    assert!(one.contains("Carried out by @helper"), "{one}");
    running.abort();
}
