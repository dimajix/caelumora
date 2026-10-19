//! The agent actor in a running channel: its turns, plans, retries, approvals, tool calls, and how
//! it is cancelled, removed or lowered.

use super::*;
use anyhow::Result;
use caelumora_model::agent::{AgentChannelState, AgentId, HearsDuringTurn, HearsFrom, RetryRefused};
use caelumora_model::approval::{ApprovalRequest, ApprovalResponse};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::ModelProfile;
use caelumora_model::history::HistoryEntry;
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::message::{MessageEvent, MessageRole};
use caelumora_model::security::AllowAllPolicy;
use caelumora_model::tool::{Tool, ToolResultStatus};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::work::TurnId;
use crate::components::create_plan_tool::CREATE_PLAN_TOOL_KIND;
use crate::llm::{LLMClient, LLMError, LLMResult};
use crate::message::testing::given_up_turns;
use crate::tool::calculator_tool::CALCULATOR_TOOL_KIND;
use crate::tool::sleep_tool::SLEEP_TOOL_KIND;
use crate::tool::stop_tool::STOP_TOOL_KIND;
use crate::turn::MAX_MODEL_CALLS;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

#[tokio::test]
async fn an_agent_takes_a_change_in_after_its_turn_and_is_given_what_stands_from_then_on() {
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the agent asks its model about hello");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let hello = channel.whole_history().await[0].id;
    let until = |what: &'static str, done: Box<dyn Fn(&AgentChannelState) -> bool + Send>| {
        let channel = Arc::clone(&channel);
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(state) = channel.stored_agent_channel_state(agent).await {
                        if done(&state) { break }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap_or_else(|_| panic!("{what}"));
        }
    };

    // Modified while the turn about it runs: the turn ends as it began.
    alice.modify_message(hello, "good evening".to_string(), None).await.expect("hers");
    tokio::time::timeout(Duration::from_secs(5), async {
        while channel.message_of(hello).await.and_then(|m| m.content).as_deref() != Some("good evening") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the channel takes it in");
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["first"]);

    alice.send_message("and now?".to_string(), None).await.unwrap();
    model.gate.add_permits(1);
    assert_eq!(said(answered(&mut events, agent).await), ["second"]);
    let seen = model.seen.lock().unwrap().clone();
    let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(heard(&seen[0], "hello"), "the turn under way saw what was said then");
    assert!(heard(&seen[1], "good evening") && !heard(&seen[1], "hello"), "the next is given what stands");
    until("the agent's transcript holds the modification", Box::new(move |state| state.transcript.messages().iter().any(|m| m.id == hello && m.modified.is_some()))).await;

    let question = channel.whole_history().await.iter().find(|m| m.content.as_deref() == Some("and now?")).expect("said").id;
    alice.revoke_message(question).await.expect("hers to take back");
    until("the agent takes it back", Box::new(move |state| {
        !state.transcript.messages().iter().any(|m| m.id == question)
    })).await;
    assert!(channel.message_of(question).await.is_some_and(|m| m.revoked.is_some()), "so does the channel");

    // Another's message released (issue 239), as the policy lets alice
    // here: it reaches the agent that said it, which is not its sender.
    let first = channel.whole_history().await.iter().find(|m| m.content.as_deref() == Some("first")).expect("answered").id;
    alice.reclassify_message(first, Classification::PUBLIC).await.expect("hers to release");
    until("the agent takes the release in", Box::new(move |state| {
        state.transcript.messages().iter().any(|m| m.id == first && m.classification == Classification::PUBLIC)
    })).await;
    running.abort();
}

#[tokio::test]
async fn an_agent_asked_to_answer_again_takes_its_answer_back_and_answers_without_it() {
    let model = Arc::new(Gated::saying(&["first", "second"], 1));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();

    alice.request_retry(first.id).await.expect("asked");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Message(first.id)));
    assert!(matches!(shown_by(&mut events, agent, "MessageRevoked").await, ActorEvent::MessageRevoked { message, .. } if message == first.id));
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "answered again");

    let seen = model.seen.lock().unwrap().clone();
    let given = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(given(&seen[1], "hello") && !given(&seen[1], "first"), "given what was said before it, not the answer replaced: {:?}", seen[1]);
    assert!(channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_some()), "the channel shows it taken back");
    let history: Vec<_> = channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(history, ["hello", "second"], "in its place, the new answer at the end");
    running.abort();
}

#[tokio::test]
async fn an_agent_does_not_answer_again_once_more_was_said() {
    let model = Arc::new(Gated::saying(&["first", "second", "third"], 2));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("and now?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;

    alice.request_retry(first.id).await.expect("asked");
    let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
    assert_eq!(target, RetryTarget::Message(first.id));
    assert_eq!(reason, RetryRefused::NotLast);
    assert!(channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_none()), "nothing taken back");
    assert_eq!(model.seen.lock().unwrap().len(), 2, "and no model asked again");
    running.abort();
}

/// A gated model, its agent in a channel, and the agent's first turn
/// asking the model about "hello", its answers held back.
async fn asking_a_gated_model(answers: &[&str]) -> (Stuck, Arc<Gated>) {
    let model = Arc::new(Gated::saying(answers, 0));
    let stuck = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    asked(&model, 1).await;
    (stuck, model)
}

/// Until `model` was asked `calls` times.
async fn asked(model: &Gated, calls: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().len() < calls {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or_else(|_| panic!("the model is asked {calls} times"));
}

#[tokio::test]
async fn an_answer_under_way_asked_for_again_is_given_up_and_answered_anew() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) = asking_a_gated_model(&["first", "second"]).await;
    let ActorEvent::ResponseStarted { turn_id: turn } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
    assert_eq!(channel.turn_of(ActorId::Agent(agent)), Some(turn), "the turn its start announced");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let error = |result: Result<Uuid>| result.expect_err("refused").downcast::<ChannelError>().expect("a channel's error");
    assert!(matches!(error(alice.request_retry_of_turn(ActorId::Agent(agent), TurnId::new()).await), ChannelError::TurnNotRetryable { .. }), "not another turn");

    alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
    let ActorEvent::ResponseStarted { turn_id: next } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
    assert_ne!(next, turn, "a turn of its own answers");
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "the answer under way never came");

    let seen = model.seen.lock().unwrap().clone();
    assert!(seen[1].iter().any(|c| c.contains("hello")), "asked about what the given-up turn was asked: {:?}", seen[1]);
    let history: Vec<_> = channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(history, ["hello", "second"], "nothing of the given-up turn reached the channel");
    assert!(matches!(error(alice.request_retry_of_turn(ActorId::Agent(agent), turn).await), ChannelError::TurnNotRetryable { .. }), "and it is over");
    running.abort();
}

#[tokio::test]
async fn what_was_said_during_a_turn_asked_for_again_is_answered_in_the_one_that_follows() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) = asking_a_gated_model(&["first", "second", "third"]).await;
    let ActorEvent::ResponseStarted { turn_id: turn } = shown_by(&mut events, agent, "ResponseStarted").await else { unreachable!() };
    let alice_id = alice_of(&context);
    let alice = channel.user_actor(alice_id).unwrap();
    alice.send_message("and this too".to_string(), None).await.expect("taken during a turn");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice_id) && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                break;
            }
        }
    }).await.expect("the message is said in the channel");

    alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
    model.gate.add_permits(3);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["second"]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one turn answers both, not one each: {seen:?}");
    assert!(seen[1].iter().any(|c| c.contains("hello")) && seen[1].iter().any(|c| c.contains("and this too")), "{:?}", seen[1]);
    running.abort();
}

#[tokio::test]
async fn an_older_answer_asked_for_again_leaves_the_answer_under_way_alone() {
    let (Stuck { channel, context, agent, helper, mut events, running }, model) = asking_a_gated_model(&["first", "second", "third"]).await;
    model.gate.add_permits(1);
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("and now?".to_string(), None).await.unwrap();
    asked(&model, 2).await;

    // Asked while the next answer is under way: it waits for it, and
    // so does what is said meanwhile.
    alice.request_retry(first.id).await.expect("asked");
    alice.send_message("one more".to_string(), None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(helper.state(), ActorState::Planning, "the answer under way goes on");
    model.gate.add_permits(2);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "it ends as it began");

    // Judged against what stands, beside the request to answer "one
    // more": not its last answer any more.
    let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
    assert_eq!((target, reason), (RetryTarget::Message(first.id), RetryRefused::NotLast));
    assert_eq!(said(answered(&mut events, agent).await), ["third"], "and the request is answered");
    assert!(channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_none()), "nothing taken back");
    running.abort();
}

/// A model that answers each call with the next of its answers only once
/// `gate` lets it, and remembers the contents each call was given.
#[derive(Debug)]
struct Gated {
    answers: Mutex<VecDeque<Message>>,
    gate: Arc<Semaphore>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl Gated {
    /// A gated model that gives `answers`, the first `permits` of them at once.
    fn new(answers: impl IntoIterator<Item = Message>, permits: usize) -> Self {
        Self { answers: Mutex::new(answers.into_iter().collect()), gate: Arc::new(Semaphore::new(permits)), seen: Default::default() }
    }

    /// A gated model that says `answers`, the first `permits` of them at once.
    fn saying(answers: &[&str], permits: usize) -> Self {
        Self::new(answers.iter().map(|answer| model_says(answer)), permits)
    }
}

#[async_trait::async_trait]
impl LLMClient for Gated {
    async fn chat_completion(&self, messages: &[Message], _: &[Tool])
        -> BoxStream<'static, LLMResult<MessageEvent>>
    {
        self.seen.lock().unwrap().push(messages.iter().filter_map(|m| m.content.clone()).collect());
        let Some(answer) = self.answers.lock().unwrap().pop_front() else { return Box::pin(futures::stream::pending()) };
        let gate = Arc::clone(&self.gate);
        Box::pin(futures::stream::once(async move {
            gate.acquire().await.unwrap().forget();
            Ok(MessageEvent::Complete(answer))
        }))
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

#[tokio::test]
async fn what_a_user_says_during_a_turn_is_answered_after_it() {
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the agent asks its model about hello");

    // Taken while the turn waits for its model: nothing holds the user back.
    let alice = alice_of(&context);
    channel.user_actor(alice).unwrap().send_message("and this too".to_string(), None).await.expect("taken during a turn");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice) && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                break;
            }
        }
    }).await.expect("the message is said in the channel");
    model.gate.add_permits(2);

    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["first"], "the running turn ends as it began");
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "and a turn of its own answers the message");
    let seen = model.seen.lock().unwrap().clone();
    let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(heard(&seen[0], "hello") && !heard(&seen[0], "and this too"), "the running turn does not see it");
    assert!(heard(&seen[1], "and this too"), "the next one does");
    running.abort();
}

/// Issue 223: an agent whose turn outlasts more than its inbox holds - two
/// hundred messages, a modification of one of them, a command and a message
/// after it - hears all of it once its turn is over, in order and as it
/// stands, and carries the command out where it came.
#[tokio::test]
async fn an_agent_whose_turn_outlasts_its_inbox_hears_all_of_it_afterwards() {
    use caelumora_model::component::ComponentDescriptor;
    const SAID: usize = 200;
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("marking", "test_marking")] };
    // What the channel takes in is not watched: more than a watcher holds.
    let Stuck { channel, context, agent, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    for n in 0..SAID {
        alice.send_message(format!("message {n}"), None).await.expect("taken during a turn");
    }
    // Read from the store: the channel holds a window of its history alone.
    let last = format!("message {}", SAID - 1);
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let said = channel.visible_messages(ActorId::User(alice_of(&context))).await;
            if said.iter().any(|m| m.content.as_deref() == Some(last.as_str())) {
                break said.into_iter().find(|m| m.content.as_deref() == Some("message 0")).expect("said first");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("all of it is said in the channel");
    alice.modify_message(first.id, "message 0, corrected".to_string(), None).await.expect("hers to modify");
    alice.execute_command("mark", "@helper during").await.expect("sent to the agent");
    alice.send_message("after the command".to_string(), None).await.expect("said");
    tokio::time::timeout(Duration::from_secs(10), async {
        while !channel.visible_messages(ActorId::User(alice_of(&context))).await.iter().any(|m| m.content.as_deref() == Some("after the command")) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("all of it is in the channel before the turn ends");
    model.gate.add_permits(2);

    let answers = |state: &AgentChannelState| state.transcript.messages().into_iter().filter(|m| m.role == MessageRole::Assistant).filter_map(|m| m.content).collect::<Vec<_>>();
    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn.is_none() && answers(&state).len() == 2 {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the agent answers twice");
    assert_eq!(answers(&state), ["first", "second"], "the turn ends as it began, and one turn answers all of it");
    let heard: Vec<String> = state.transcript.messages().into_iter().filter(|m| m.role == MessageRole::User).filter_map(|m| m.content).collect();
    let mut expected: Vec<String> = vec!["hello".to_string(), "message 0, corrected".to_string()];
    expected.extend((1..SAID).map(|n| format!("message {n}")));
    expected.push("after the command".to_string());
    assert_eq!(heard, expected, "all of it, once, in order, as it stands");
    assert_eq!(state.notes.current().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["marked during"], "the command carried out");
    let seen = model.seen.lock().unwrap().clone();
    assert!(seen[1].iter().any(|c| c.contains("after the command")), "the next turn is given all of it");
    running.abort();
}

/// Issue 385: a channel that stops while an agent is in its turn, and runs
/// again: what was delivered to the agent and not taken in - a message, a
/// request to answer it, a change of what was said - is delivered again from
/// the history, and taken in once; what it took in and stored is not
/// delivered again.
#[tokio::test]
async fn what_an_agent_was_delivered_and_did_not_store_before_a_restart_is_delivered_again_once() {
    let (Stuck { channel, context, agent, running, .. }, model) = asking_a_gated_model(&["never given", "Answered.", "Again."]).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let hello = channel.whole_history().await.into_iter().find(|m| m.content.as_deref() == Some("hello")).expect("said");
    alice.send_message("and this too".to_string(), None).await.expect("taken during a turn");
    alice.modify_message(hello.id, "hello, corrected".to_string(), None).await.expect("hers to modify");
    tokio::time::timeout(Duration::from_secs(5), async {
        while channel.message_of(hello.id).await.and_then(|m| m.content).as_deref() != Some("hello, corrected") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the change is recorded");

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.transcript.messages().iter().all(|m| m.content.as_deref() != Some("and this too")), "not taken in before the stop");

    let (_restarted, mut events, running) = run_again(&channel).await;
    asked(&model, 2).await;
    model.gate.add_permits(2);
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["Answered."], "the turn resumed");
    assert_eq!(said(answered(&mut events, agent).await), ["Again."], "and what it was delivered asks it again");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let heard: Vec<String> = state.transcript.messages().into_iter().filter(|m| m.role == MessageRole::User).filter_map(|m| m.content).collect();
    assert_eq!(heard, ["hello, corrected", "and this too"], "each once, as it stands");
    running.abort();
}

/// A gated model whose first answer calls the calculator and whose next say "done" and "again",
/// its agent hearing between the steps of a turn as `hears` says (issue 431): the agent asked
/// about "hello", the call held back, and alice saying "and this too" meanwhile.
async fn told_while_calculating(hears: HearsDuringTurn) -> (Stuck, Arc<Gated>) {
    let (calculate, components) = calculator();
    let model = Arc::new(Gated::new([calculate, model_says("done"), model_says("again")], 0));
    let mut stuck = with_an_agent_hearing(hears, Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let alice = alice_of(&stuck.context);
    stuck.channel.user_actor(alice).unwrap().send_message("and this too".to_string(), None).await.expect("taken during a turn");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = stuck.events.recv().await.expect("the channel is running");
            if let ActorEvent::CompleteMessageGenerated(message) = envelope.event {
                if message.content.as_deref() == Some("and this too") { break }
            }
        }
    }).await.expect("the message is said in the channel");
    (stuck, model)
}

/// Where in `call` - what a model was given - a content with `text` is.
fn given_at(call: &[String], text: &str) -> Option<usize> {
    call.iter().position(|content| content.contains(text))
}

#[tokio::test]
async fn what_a_user_says_between_the_steps_of_a_turn_is_given_to_its_next_step_and_answered_by_it() {
    let (Stuck { agent, mut events, running, .. }, model) = told_while_calculating(HearsDuringTurn { from: HearsFrom::Users, ..Default::default() }).await;
    model.gate.add_permits(3);

    let answer = answered(&mut events, agent).await;
    assert_eq!(answer.last().and_then(|m| m.content.as_deref()), Some("done"), "the turn ends after its tool step");
    let seen = model.seen.lock().unwrap().clone();
    assert!(given_at(&seen[0], "and this too").is_none(), "the first step was asked before it was said");
    let heard = given_at(&seen[1], "and this too").expect("the next step is given it");
    let told = given_at(&seen[1], "while you were working").expect("and told that it was said meanwhile");
    assert!(heard < told, "after what was heard: {:?}", seen[1]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(model.seen.lock().unwrap().len(), 2, "the turn answered its request: no other follows");
    running.abort();
}

#[tokio::test]
async fn an_agent_that_hears_nobody_during_a_turn_answers_what_was_said_after_it() {
    let (Stuck { agent, mut events, running, .. }, model) = told_while_calculating(HearsDuringTurn::default()).await;
    model.gate.add_permits(3);

    let said = |messages: Vec<Message>| messages.last().and_then(|m| m.content.clone());
    assert_eq!(said(answered(&mut events, agent).await).as_deref(), Some("done"));
    assert_eq!(said(answered(&mut events, agent).await).as_deref(), Some("again"), "a turn of its own answers it");
    let seen = model.seen.lock().unwrap().clone();
    assert!(given_at(&seen[1], "and this too").is_none(), "the running turn does not hear it");
    assert!(given_at(&seen[2], "and this too").is_some() && given_at(&seen[2], "while you were working").is_none(), "the next turn does, as it hears between turns");
    running.abort();
}

#[tokio::test]
async fn an_agent_takes_note_of_who_left_without_responding() {
    let model = Arc::new(Gated::saying(&["first", "second"], 1));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    answered(&mut events, agent).await;

    channel.remove_participant(&context, context.initiator_id).await.expect("she may leave her channel");

    // In its transcript, where the next turn's model is told of it, and in
    // the channel's history as the agent saw it (issues 408, 417); it asks
    // nobody to respond.
    let state = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.transcript.entries().any(|node| matches!(node.entry, TranscriptEntry::ParticipantLeft { .. })) { break state }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the agent takes note of it");
    assert!(state.transcript.entries().any(|node| matches!(node.entry, TranscriptEntry::ParticipantLeft { participant, by, .. } if participant == context.initiator_id && by == context.initiator_id)));
    assert!(matches!(channel.visible_entries(ActorId::Agent(agent)).await.last(), Some(HistoryEntry::ParticipantLeft { participant, by, .. }) if *participant == context.initiator_id && *by == context.initiator_id));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(model.seen.lock().unwrap().len(), 1, "no turn begins for it");
    running.abort();
}

#[tokio::test]
async fn an_agent_asked_several_times_during_a_turn_answers_once_after_it() {
    let model = Arc::new(Gated::saying(&["first", "second", "third"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the agent asks its model about hello");

    // Three messages, each asking the agent, while its turn waits.
    let alice = alice_of(&context);
    let queued = ["one", "two", "three"];
    for text in queued {
        channel.user_actor(alice).unwrap().send_message(text.to_string(), None).await.expect("taken during a turn");
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut said = 0;
        while said < queued.len() {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice) && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                said += 1;
            }
        }
    }).await.expect("the messages are said in the channel");
    model.gate.add_permits(3);

    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["first"], "the running turn ends as it began");
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "one turn answers what was queued");
    let seen = model.seen.lock().unwrap().clone();
    assert!(queued.iter().all(|text| seen[1].iter().any(|c| c.contains(text))), "having heard all of it: {:?}", seen[1]);

    // Nothing is left to answer: told something new, the agent answers
    // it with the next call of its model, not with a stale one.
    channel.user_actor(alice).unwrap().send_message("four".to_string(), None).await.expect("sent");
    assert_eq!(said(answered(&mut events, agent).await), ["third"]);
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "one call per turn, one turn per batch");
    assert!(seen[2].iter().any(|c| c.contains("four")), "the third call answers the new message");
    running.abort();
}

#[tokio::test]
async fn a_sleeping_agent_woken_by_a_mention_answers_it() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let answer = |text: &str, tool_calls| Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        tool_calls,
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let stop = vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }];
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "stop".to_string(),
        kind: STOP_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let model = Scripted(Mutex::new([answer("Good bye.", stop), answer("I am back.", Vec::new())].into()));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), components).await;
    settled(&helper, ActorState::Sleeping).await;
    let alice = alice_of(&context);

    // The message and the request reach it together, and it was woken
    // before either: it answers (issue 222).
    channel.user_actor(alice).unwrap().send_message("@helper, are you there?".to_string(), None).await.expect("sent");
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    loop {
        if said(answered(&mut events, agent).await) == ["I am back."] {
            break;
        }
    }
    running.abort();
}

#[tokio::test]
async fn an_agent_that_calls_stop_ends_its_turn_with_the_call_and_sleeps() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // One answer only: a second call of the model would never return.
    let bye = Message {
        role: MessageRole::Assistant,
        content: Some("Good bye.".to_string()),
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "stop".to_string(),
        kind: STOP_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, agent, helper, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new([bye].into()))), components).await;

    // The turn ends with the call - the model is not asked again - and
    // the agent sleeps, as the tool asked; no turn is left behind.
    settled(&helper, ActorState::Sleeping).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turn.is_none(), "the turn was taken over");
    assert!(channel.visible_messages(ActorId::Agent(agent)).await.iter().any(|m| m.content.as_deref() == Some("Good bye.")), "and its answer recorded in the channel's history");
    running.abort();
}

/// An agent tells the users where it stands as it starts and moves
/// (issue 243): through its turn into its sleep - and woken by `/wakeup`,
/// which sends it no message, it tells them so too.
#[tokio::test]
async fn an_agent_tells_the_users_where_it_stands_and_that_it_was_woken() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let bye = Message {
        role: MessageRole::Assistant,
        content: Some("Good bye.".to_string()),
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "stop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "stop".to_string(),
        kind: STOP_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let (channel, context, agent, alice) = agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted(Mutex::new([bye].into()))), components).await;
    // What reaches alice, watched since before it ran.
    let mut shown = channel.user_actor(alice).unwrap().watch();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
    assert_eq!(told_states(&mut shown, agent, ActorState::Idle).await, [ActorState::Idle], "where it starts, before it is asked");
    channel.user_actor(alice).unwrap().send_message("hello".to_string(), None).await.unwrap();
    let helper = channel.actor_of(ActorId::Agent(agent)).unwrap();
    assert_eq!(told_states(&mut shown, agent, ActorState::Sleeping).await, [ActorState::Planning, ActorState::ExecutingTools, ActorState::Reflecting, ActorState::Sleeping], "each step it moves to, once");

    let woken = channel.wakeup(&context).await.expect("alice may wake it");
    assert_eq!(woken, ["helper"]);
    assert_eq!(told_states(&mut shown, agent, ActorState::Idle).await, [ActorState::Idle], "woken without a message, it tells so itself");
    assert_eq!(helper.state(), ActorState::Idle);
    running.abort();
}

#[tokio::test]
async fn a_plan_whose_steps_need_more_calls_than_a_turn_runs_to_its_end() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // Two steps, each calling a tool more than half as often as a turn may
    // call its model (issue 412).
    let said = |content: &str| Message { role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("plan".to_string()),
            function_name: "create_plan".to_string(),
            arguments: serde_json::json!({ "summary": "two steps", "steps": [{ "description": "first" }, { "description": "second" }] }),
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let (calculate, calculator) = calculator();
    let calls = MAX_MODEL_CALLS / 2 + 1;
    let step = |done: &str| (0..calls).map(|_| Message { id: MessageId::new(), ..calculate.clone() }).chain([said(done)]).collect::<Vec<_>>();
    let script: VecDeque<Message> = [plan].into_iter().chain(step("first done")).chain(step("second done")).chain([said("all done")]).collect();
    assert!(script.len() > MAX_MODEL_CALLS as usize, "more calls than a turn without a plan may make");
    let components = ComponentConfig { component_nodes: [ComponentDescriptor {
        id: "plan".to_string(),
        kind: CREATE_PLAN_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }].into_iter().chain(calculator.component_nodes).collect() };
    let Stuck { agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new(script))), components).await;

    let final_messages = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::TurnGivenUp { reason, .. } => panic!("given up: {reason:?}"),
                _ => {}
            }
        }
    }).await.expect("the turn ends");
    let contents: Vec<_> = final_messages.iter().filter_map(|m| m.content.as_deref()).collect();
    assert_eq!(contents, ["all done"], "the plan came to its final answer");
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn a_plan_runs_its_steps_as_intermediate_messages_and_ends_with_the_final_answer() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let said = |content: &str| Message { role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            function_name: "create_plan".to_string(),
            arguments: serde_json::json!({ "summary": "two steps", "steps": [{ "description": "first" }, { "description": "second" }] }),
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let script = [plan, said("first done"), said("second done"), said("all done")];
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "plan".to_string(),
        kind: CREATE_PLAN_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new(script.into()))), components).await;

    // The plan needs no approval: it runs at once.
    let mut seen = Vec::new();
    let final_messages = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            assert!(!matches!(envelope.event, ActorEvent::ApprovalRequested(_)), "nobody is asked");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::IntermediateMessageGenerated(m) => seen.push(format!("intermediate:{}", m.content.unwrap_or_default())),
                // Not the plan's call nor the tool's result: they say nothing.
                ActorEvent::CompleteMessageGenerated(m) => seen.extend(m.content.map(|c| format!("complete:{c}"))),
                ActorEvent::PlanStepStarted { step_index, .. } => seen.push(format!("started:{step_index}")),
                ActorEvent::PlanStepFinished { step_index, .. } => seen.push(format!("finished:{step_index}")),
                _ => {}
            }
        }
    }).await.expect("the turn ends");
    assert_eq!(seen, [
        "started:0", "intermediate:first done", "finished:0",
        "started:1", "intermediate:second done", "finished:1",
        "complete:all done",
    ]);
    let contents: Vec<_> = final_messages.iter().filter_map(|m| m.content.as_deref()).collect();
    assert_eq!(contents, ["all done"], "the steps are not part of the answer");
    settled(&helper, ActorState::Idle).await;

    // The channel keeps every message as it was said, the steps too,
    // each once, and the turn's end after them (issue 258).
    let history = channel.whole_history().await;
    let kept: Vec<_> = history.iter().filter(|m| m.sender_id == ActorId::Agent(agent)).map(|m| (m.internal, m.content.clone().unwrap_or_default())).collect();
    assert_eq!(kept, [
        (false, String::new()), (false, String::new()),
        (true, "first done".to_string()), (true, "second done".to_string()),
        (false, "all done".to_string()),
    ], "the plan's call and its result, the steps, the answer");
    // Each step begins before what it says (issue 263).
    let items: Vec<String> = channel.history_entries().await.into_iter().filter_map(|item| match item {
        HistoryEntry::Said { message, .. } if message.sender_id == ActorId::Agent(agent) && message.internal => message.content,
        HistoryEntry::PlanStepStarted { actor, step_index, total_steps, description, .. } if actor == ActorId::Agent(agent) => Some(format!("step {}/{total_steps}: {description}", step_index + 1)),
        _ => None,
    }).collect();
    assert_eq!(items, ["step 1/2: first", "first done", "step 2/2: second", "second done"]);
    let ended = channel.last_history_entry().await;
    assert!(matches!(ended, Some(HistoryEntry::TurnFinished { turn_id, .. }) if Some(turn_id) == history.last().and_then(|m| m.turn_id)), "{ended:?}");
    running.abort();
}

#[tokio::test]
async fn every_message_of_a_turn_carries_the_turn_its_start_announced() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // A plan: a call, its result, the steps' internal answers and the
    // final one - every kind of message a turn produces.
    let said = |content: &str| Message { role: MessageRole::Assistant, content: Some(content.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            function_name: "create_plan".to_string(),
            arguments: serde_json::json!({ "summary": "one step", "steps": [{ "description": "first" }] }),
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let script = [plan, said("first done"), said("all done")];
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "plan".to_string(),
        kind: CREATE_PLAN_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new(script.into()))), components).await;

    let mut started = None;
    let mut shown = Vec::new();
    let mut said = Vec::new();
    let final_messages = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            said.push(envelope.context.event_id);
            match envelope.event {
                ActorEvent::ResponseStarted { turn_id } => started = Some(turn_id),
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) => shown.push(m.turn_id),
                _ => {}
            }
        }
    }).await.expect("the turn ends");
    settled(&helper, ActorState::Idle).await;

    let started = started.expect("the turn announced itself");
    let ids: HashSet<_> = said.iter().collect();
    assert_eq!(ids.len(), said.len(), "every event of the turn is one of its own (issue 352)");
    assert_eq!(shown.len(), 4, "the call, its result, the step's answer and the final answer: {shown:?}");
    assert!(shown.iter().all(|turn| *turn == Some(started)), "each shown as the turn's: {shown:?}");
    assert!(final_messages.iter().all(|m| m.turn_id == Some(started)), "and published so");
    let answer = final_messages.iter().find(|m| m.content.as_deref() == Some("all done")).expect("the final answer");
    assert_eq!(channel.message_of(answer.id).await.and_then(|m| m.turn_id), Some(started), "the channel's history keeps it");

    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let (own, heard): (Vec<_>, Vec<_>) = stored.transcript.messages().into_iter().partition(|m| m.sender_id == ActorId::Agent(agent));
    assert!(own.len() >= 4 && own.iter().all(|m| m.turn_id == Some(started)), "its transcript keeps every message of the turn as the turn's");
    assert!(heard.iter().all(|m| m.turn_id.is_none()), "what it heard is no turn's of its own");
    running.abort();
}

#[tokio::test]
async fn a_turn_is_stored_with_the_step_that_comes_next() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // The model calls a tool that needs no approval, and then never
    // answers again: the agent waits in `Planning` with the result.
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "calculator".to_string(),
        kind: CALCULATOR_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, agent, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new([calculate].into()))), components).await;

    // What a crash now would leave: `Planning`, with the tool's result
    // in the turn - so that a resume asks the model about the result
    // instead of asking again for the call or running the tool twice.
    let stored = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            let has_result = state.turn.as_ref().is_some_and(|t| state.transcript.messages().iter().any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty()));
            if has_result && state.processing_state == ActorState::Planning { break state }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("stored in Planning, with the tool's result");
    assert!(stored.turn.is_some_and(|t| t.tool_calls.is_empty()), "the calls of the step are done");
    running.abort();
}

/// A model that fails where its script says `None`, and answers where
/// it says `Some`; it waits for ever once the script is done.
#[derive(Debug)]
struct FailsWhere(Mutex<VecDeque<Option<Message>>>);

#[async_trait::async_trait]
impl LLMClient for FailsWhere {
    async fn chat_completion(&self, _: &[Message], _: &[Tool])
        -> BoxStream<'static, LLMResult<MessageEvent>>
    {
        match self.0.lock().unwrap().pop_front() {
            Some(Some(answer)) => Box::pin(futures::stream::iter([Ok(MessageEvent::Complete(answer))])),
            Some(None) => Box::pin(futures::stream::iter([Err(LLMError::Backend {
                endpoint: "test".to_string(), model: "test".to_string(), message: "gone".to_string(),
            })])),
            None => Box::pin(futures::stream::pending()),
        }
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// The turn the agent `agent` gives up next, by its `TurnGivenUp`.
async fn given_up_turn(events: &mut Receiver<Envelope>, agent: AgentId) -> TurnId {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            if let ActorEvent::TurnGivenUp { turn_id, .. } = envelope.event { break turn_id }
        }
    }).await.expect("the turn is given up")
}

#[tokio::test]
async fn a_turn_that_failed_is_answered_anew_on_request_and_stays() {
    let answer = Message { role: MessageRole::Assistant, content: Some("second try".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::new(FailsWhere(Mutex::new([None, Some(answer)].into()))), Default::default()).await;
    let failed = given_up_turn(&mut events, agent).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.request_retry_of_turn(ActorId::Agent(agent), failed).await.expect("its last turn, given up as failed (issue 259)");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(failed)));
    let said: Vec<_> = answered(&mut events, agent).await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(said, ["second try"], "answered anew after it");

    let ended: Vec<_> = channel.history_entries().await.into_iter().filter_map(|item| match item {
        HistoryEntry::TurnGivenUp { turn_id, .. } => Some(turn_id),
        _ => None,
    }).collect();
    assert_eq!(ended, [failed], "the turn given up stays, with its end");
    running.abort();
}

#[tokio::test]
async fn a_turn_given_up_is_not_asked_for_again_once_something_followed_it() {
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::new(FailsWhere(Mutex::new([None].into()))), Default::default()).await;
    let failed = given_up_turn(&mut events, agent).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("are you there?".to_string(), None).await.unwrap();
    // The message is in the channel's history once it was routed.
    tokio::time::timeout(Duration::from_secs(5), async {
        while channel.whole_history().await.len() < 2 { tokio::time::sleep(Duration::from_millis(10)).await }
    }).await.expect("the message is said");

    let refused = alice.request_retry_of_turn(ActorId::Agent(agent), failed).await;
    assert!(matches!(refused.expect_err("refused").downcast_ref::<ChannelError>(), Some(ChannelError::TurnNotRetryable { .. })), "the conversation moved on");
    running.abort();
}

#[tokio::test]
async fn a_turn_that_reaches_the_limit_of_model_calls_is_given_up_and_may_be_answered_anew() {
    // The model calls a tool at every call, as one that does not come to an
    // end; asked anew, it answers.
    let (calculate, components) = calculator();
    let mut answers: VecDeque<Message> = (0..MAX_MODEL_CALLS).map(|_| Message { id: MessageId::new(), ..calculate.clone() }).collect();
    answers.push_back(Message { role: MessageRole::Assistant, content: Some("going on".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) });
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(Mutex::new(answers))), components).await;

    // The users are told why the turn ended (issue 411).
    let (turn, reason) = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            if let ActorEvent::TurnGivenUp { turn_id, reason } = envelope.event { break (turn_id, reason) }
        }
    }).await.expect("the turn is given up");
    assert_eq!(reason, GivenUpReason::CallLimit { calls: MAX_MODEL_CALLS });
    settled(&helper, ActorState::Idle).await;

    // What its tools did stays, its end follows, and its model is told why.
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let turns = given_up_turns(&stored);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].1, GivenUpReason::CallLimit { calls: MAX_MODEL_CALLS }, "kept in the history");
    let results = turns[0].2.iter().filter(|m| m.role == MessageRole::Tool).count();
    assert_eq!(results, MAX_MODEL_CALLS as usize, "a result for every call");
    let told = given_next(&stored, agent).last().and_then(|m| m.content.clone()).unwrap_or_default();
    assert!(told.contains("because it asked the model as often as one answer, or one step of a plan, may"), "{told}");

    // It may be asked to answer anew.
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("its last turn, given up at the limit");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
    let said: Vec<_> = answered(&mut events, agent).await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(said, ["going on"], "answered anew after it");
    running.abort();
}

/// A model that gives the answers it was given, one per call, and then
/// fails.
#[derive(Debug)]
struct FailsAfter(Mutex<VecDeque<Message>>);

#[async_trait::async_trait]
impl LLMClient for FailsAfter {
    async fn chat_completion(&self, _: &[Message], _: &[Tool])
        -> BoxStream<'static, LLMResult<MessageEvent>>
    {
        match self.0.lock().unwrap().pop_front() {
            Some(answer) => Box::pin(futures::stream::iter([Ok(MessageEvent::Complete(answer))])),
            None => Box::pin(futures::stream::iter([Err(LLMError::Backend {
                endpoint: "test".to_string(), model: "test".to_string(), message: "gone".to_string(),
            })])),
        }
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

#[tokio::test]
async fn a_turn_that_fails_after_a_tool_ran_keeps_what_it_said_and_the_agent_is_told() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // The model calls a tool that needs no approval, and fails when
    // asked about its result.
    let calculate = Message {
        id: MessageId::new(),
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "calculator".to_string(),
        kind: CALCULATOR_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, context: _, agent, helper, mut events, running } =
        with_an_agent(Arc::new(FailsAfter(Mutex::new([calculate.clone()].into()))), components).await;

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnGivenUp { .. }) { break }
        }
    }).await.expect("the turn fails");
    settled(&helper, ActorState::Idle).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();

    // The turn's messages stay where they were said, followed by its end
    // (issue 258); what the model saw for them can still be told.
    let seen = channel.agent_context(agent).await;
    let turns = given_up_turns(&stored);
    assert_eq!(turns.len(), 1);
    assert!(matches!(turns[0].1, GivenUpReason::Failed { .. }));
    let roles: Vec<_> = turns[0].2.iter().map(|m| m.role).collect();
    assert_eq!(roles, [MessageRole::Assistant, MessageRole::Tool], "the call and its result");
    assert!(seen.context_of(calculate.id).await.is_ok(), "the call's context is resolved");
    assert!(stored.transcript.messages().iter().any(|m| m.id == calculate.id), "in the transcript");
    assert!(channel.visible_messages(ActorId::Agent(agent)).await.iter().any(|m| m.id == calculate.id), "and in the channel's history, as the agent said it");

    // Its model is given the call in full, and told why the turn ended.
    let given = given_next(&stored, agent);
    assert!(given.iter().any(|m| m.tool_calls.iter().any(|c| c.arguments == serde_json::json!({ "expression": "1+1" }))), "{given:?}");
    let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
    assert!(told.contains("because it failed"), "{told}");
    running.abort();
}

#[tokio::test]
async fn an_agent_removed_during_a_turn_gives_it_up() {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // The model calls a tool, and never answers again.
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "calculator".to_string(),
        kind: CALCULATOR_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new([calculate].into()))), components).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn.as_ref().is_some_and(|t| state.transcript.messages().iter().any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty())) { break }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the tool ran");

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    let turns = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turns = given_up_turns(&channel.stored_agent_channel_state(agent).await.unwrap());
            if !turns.is_empty() { break turns }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the turn is given up");
    assert_eq!(turns[0].1, GivenUpReason::Left);
    running.abort();
}

/// What the model of the agent whose state is `stored` is given next:
/// its transcript as it stands, resolved as a turn's context is.
fn given_next(stored: &AgentChannelState, agent: AgentId) -> Vec<Message> {
    use caelumora_model::context::{ContextItem, ContextRef, ContextResolver};
    let reference = ContextRef { transcript: stored.transcript.digest(), ..ContextRef::default() };
    ContextResolver::of(stored).resolve(&reference, TurnId::new(), ActorId::Agent(agent)).expect("resolved").items.into_iter()
        .filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message),
            ContextItem::Prompt { .. } => None,
        })
        .collect()
}

fn calculator() -> (Message, ComponentConfig) {
    use caelumora_model::tool::{ToolCall, ToolVisibility};
    let calculate = Message {
        id: MessageId::new(),
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "calculate".to_string(), arguments: serde_json::json!({ "expression": "1+1" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "calculator".to_string(),
        kind: CALCULATOR_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    (calculate, components)
}

#[tokio::test]
async fn lowering_the_channel_gives_up_a_running_turn() {
    use caelumora_model::confidentiality::Confidentiality;

    // The model calls a tool, and then is asked about its result for
    // ever.
    let (calculate, components) = calculator();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent_under(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted(Mutex::new([calculate.clone()].into()))), components).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn.as_ref().is_some_and(|t| state.transcript.messages().iter().any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty())) { break }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the tool ran");

    let public = Classification::general(Confidentiality::Public);
    channel.reclassify(&context, public.clone()).await.expect("permitted");

    // The model call is interrupted, and the turn given up rather than
    // taken over: the users are told, nothing of it is in the history.
    let told = turn_given_up(&mut events, agent).await;
    assert!(told.contains("lowered"), "{told}");
    settled(&helper, ActorState::Idle).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let turns = given_up_turns(&stored);
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].1, GivenUpReason::Lowered { channel: public.clone() });
    // What it said before the lowering was held by the channel: it stays,
    // fitted into the channel as it is now (issue 258).
    let said = channel.whole_history().await.into_iter().find(|m| m.id == calculate.id).expect("said before the lowering");
    assert_eq!(said.classification, public);

    // The agent keeps what the turn said, but its model is given which
    // tool ran and not what it saw (issues 141, 258).
    assert!(stored.transcript.messages().iter().any(|m| m.id == calculate.id), "the record keeps it");
    let given = given_next(&stored, agent);
    let call = given.iter().find(|m| m.id == calculate.id).expect("the call is given");
    assert_eq!((call.tool_calls[0].function_name.as_str(), &call.tool_calls[0].arguments), ("calculate", &Value::Null));
    assert_eq!(call.classification, public);
    assert!(given.iter().flat_map(|m| &m.tool_results).all(|r| r.content.is_empty()), "nothing of what it returned");
    let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
    assert!(told.contains("lowered") && told.contains("withheld"), "{told}");
    running.abort();
}

#[tokio::test]
async fn lowering_the_channel_ends_a_turn_waiting_for_approval_at_once() {
    use caelumora_model::confidentiality::Confidentiality;

    // Nobody approves the call.
    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent_under(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted(Mutex::new([call].into()))), components).await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel.reclassify(&context, Classification::general(Confidentiality::Public)).await.expect("permitted");

    turn_given_up(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn raising_the_channel_lets_a_running_turn_go_on() {
    use caelumora_model::confidentiality::Confidentiality;
    let stuck = with_an_agent_under(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), Default::default()).await;
    settled(&stuck.helper, ActorState::Planning).await;

    stuck.channel.reclassify(&stuck.context, Classification::general(Confidentiality::Confidential)).await.expect("permitted");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(stuck.helper.state(), ActorState::Planning, "the model call goes on");
    stuck.running.abort();
}

#[tokio::test]
async fn a_cancel_ends_a_turn_waiting_for_approval_at_once() {

    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(Mutex::new([call].into()))), components).await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel.cancel(&context).await.expect("permitted");

    // Without anyone deciding on the call.
    settled(&helper, ActorState::Idle).await;

    // The request is withdrawn, and the approver tells the users
    // (issue 148).
    let (requested, withdrawn) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut requested = None;
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            match envelope.event {
                ActorEvent::ApprovalRequested(request) if matches!(envelope.context.sender_id, ActorId::Approver(_)) => requested = Some(request.id),
                ActorEvent::ApprovalRequestWithdrawn { id } if matches!(envelope.context.sender_id, ActorId::Approver(_)) => break (requested, id),
                _ => {}
            }
        }
    }).await.expect("the users learn that the request was withdrawn");
    assert_eq!(requested, Some(withdrawn));
    running.abort();

    // The call is answered as not executed (issue 148).
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let last = stored.transcript.messages().pop().expect("the turn was taken over");
    let answered: Vec<_> = last.tool_results.iter().map(|r| r.status).collect();
    assert_eq!(answered, [ToolResultStatus::NotExecuted]);
}

/// A call of `sleep` that needs approval, and the component that gives
/// it, for an agent to wait for its approval.
fn a_call_needing_approval() -> (Message, ComponentConfig) {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({ "seconds": 0 }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "sleep".to_string(),
        kind: SLEEP_TOOL_KIND.to_string(),
        params: serde_json::json!({ "requires_approval": true }),
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    (call, components)
}

#[tokio::test]
async fn a_rule_of_the_channel_asks_for_a_call_its_tool_would_run() {
    use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};

    let (call, mut components) = a_call_needing_approval();
    components.component_nodes[0].params = serde_json::json!({ "requires_approval": false });
    let built = agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted(Mutex::new([call].into()))), components).await;
    let ask = ApprovalRule::new(ToolMatcher { tool: "sleep".to_string(), arguments: Default::default() }, RuleDecision::Ask, Reach::Everywhere, None).unwrap();
    built.0.update_approval_rules(&SecurityContext::system(), vec![ask]).await.expect("the system may");

    let Stuck { helper, mut events, running, .. } = start(built).await;
    let request = approval_requested(&mut events).await;
    assert!(request.explicit, "for a decision of a person's own");
    settled(&helper, ActorState::WaitingForUser).await;
    running.abort();
}

/// The approval the approver asks the users for, from `events`.
async fn approval_requested(events: &mut broadcast::Receiver<Envelope>) -> ApprovalRequest {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if let ActorEvent::ApprovalRequested(request) = envelope.event {
                if matches!(envelope.context.sender_id, ActorId::Approver(_)) {
                    break request;
                }
            }
        }
    }).await.expect("the users are asked")
}

#[tokio::test]
async fn a_turn_waiting_for_approval_can_be_decided_after_a_restart() {

    let (call, components) = a_call_needing_approval();
    let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted(Mutex::new([call, done].into()))), components).await;
    settled(&helper, ActorState::WaitingForUser).await;
    let asked = approval_requested(&mut events).await;

    // The turn is the record of what it waits for (issue 172).
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let pending: Vec<_> = stored.turn.expect("a turn under way").pending_approvals().into_iter().map(|r| r.id).collect();
    assert_eq!(pending, [asked.id]);

    // A restart: the channel and its actors are gone, the approver's
    // memory with them; the channel is built again from the stores.
    running.abort();
    let _ = running.await;
    let document = Channel::clone(&channel.channel());
    let restarted = ChannelLoop::new(channel.restarted_runtime()).await.unwrap();
    let mut events = restarted.observe();
    let running = tokio::spawn({ let restarted = Arc::clone(&restarted); async move { restarted.run().await } });
    let helper = restarted.actor_of(ActorId::Agent(agent)).unwrap();

    // The agent asks again, with the same id, and the users can decide.
    let asked_again = approval_requested(&mut events).await;
    assert_eq!(asked_again.id, asked.id);
    let alice = document.participants.iter().find_map(|p| match p { Participant::User(id) => Some(*id), _ => None }).expect("alice");
    let response = ApprovalResponse::new(asked.id, agent);
    restarted.user_actor(alice).unwrap().send_approval(response, true).await.unwrap();
    let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Done."], "the call runs and the turn goes on");
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

/// An agent whose model calls the `sleep` tool, which sleeps longer
/// than any test waits, and would then say what it is never asked for.
async fn with_an_agent_in_a_long_tool() -> (Stuck, Arc<Scripted>) {
    with_an_agent_in_long_tools(&["1"]).await
}

/// An agent whose model calls a tool that sleeps a minute, once for
/// each of `calls`, by their ids, and then answers "never given".
async fn with_an_agent_in_long_tools(calls: &[&str]) -> (Stuck, Arc<Scripted>) {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: calls.iter()
            .map(|id| ToolCall { id: Some(id.to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() })
            .collect(),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "sleep".to_string(),
        kind: SLEEP_TOOL_KIND.to_string(),
        params: serde_json::json!({ "seconds": 60 }),
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let unasked = Message { role: MessageRole::Assistant, content: Some("never given".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let model = Arc::new(Scripted(Mutex::new([call, unasked].into())));
    let stuck = with_an_agent(Arc::<Scripted>::clone(&model), components).await;
    settled(&stuck.helper, ActorState::ExecutingTools).await;
    (stuck, model)
}

#[tokio::test]
async fn a_turn_asked_for_again_while_it_waits_for_an_approval_withdraws_the_request() {

    let (call, components) = a_call_needing_approval();
    let instead = Message { role: MessageRole::Assistant, content: Some("no wait needed".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(Mutex::new([call, instead].into()))), components).await;
    let ActorEvent::ApprovalRequested(request) = shown_by(&mut events, agent, "ApprovalRequested").await else { unreachable!() };
    settled(&helper, ActorState::WaitingForUser).await;

    let turn = channel.turn_of(ActorId::Agent(agent)).expect("waiting is under way");
    channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
    assert!(matches!(shown_by(&mut events, agent, "ApprovalRequestWithdrawn").await, ActorEvent::ApprovalRequestWithdrawn { id } if id == request.id),
        "nobody is asked any longer");
    assert!(matches!(shown_by(&mut events, agent, "RetryAccepted").await, ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)));
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["no wait needed"]);
    running.abort();
}

#[tokio::test]
async fn a_turn_asked_for_again_while_a_tool_runs_tells_the_next_one_of_the_call() {
    let (Stuck { channel, context, agent, mut events, running, .. }, _) = with_an_agent_in_a_long_tool().await;
    let turn = channel.turn_of(ActorId::Agent(agent)).expect("under way");
    channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");

    assert_eq!(turn_given_up(&mut events, agent).await, "your answer was asked for again", "the users learn why the turn ended (issue 258)");
    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["never given"], "the tool is interrupted, and the next turn answers");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let given_up = given_up_turns(&state);
    assert_eq!(given_up.len(), 1, "the turn is kept as given up");
    assert_eq!(given_up[0].1, GivenUpReason::Retried);
    assert_eq!(given_up[0].0, turn);
    assert!(given_up[0].2.iter().any(|m| m.tool_calls.iter().any(|c| c.function_name == "sleep")), "the call that ran stays");
    let told: Vec<_> = given_next(&state, agent).into_iter().filter_map(|m| m.content).filter(|c| c.contains("given up")).collect();
    assert!(told.iter().any(|c| c.contains("asked for again")), "the model is told why the turn was given up: {told:?}");
    let history: Vec<_> = channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(history, ["hello", "never given"], "the channel's history takes a turn's answer alone");
    running.abort();
}

#[tokio::test]
async fn a_cancel_while_a_tool_runs_interrupts_the_tool_and_ends_the_turn() {

    let (Stuck { channel, context, agent, helper, mut events, running }, model) = with_an_agent_in_a_long_tool().await;

    channel.cancel(&context).await.expect("permitted");

    // The turn ends at once, not when the tool would have (issue 161),
    // and the model is not asked again.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event { break messages }
            }
        }
    }).await.expect("the cancelled turn ends without waiting for the tool");
    let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.len(), 1, "{ended:?}");
    assert_eq!(results[0].status, ToolResultStatus::Interrupted);
    assert!(results[0].started_at.is_some(), "it ran");
    settled(&helper, ActorState::Idle).await;
    assert_eq!(model.0.lock().unwrap().len(), 1, "the second answer was never asked for");
    running.abort();
}

#[tokio::test]
async fn a_tool_call_interrupted_alone_is_answered_so_and_the_turn_goes_on() {

    let (Stuck { channel, context, agent, mut events, running, .. }, model) = with_an_agent_in_long_tools(&["1", "2"]).await;
    let turn = channel.turn_of(ActorId::Agent(agent)).expect("under way");
    let not_running = |refused: Result<()>| matches!(refused.expect_err("refused").downcast_ref::<ChannelError>(), Some(ChannelError::ToolCallNotRunning { .. }));
    assert!(not_running(channel.interrupt_tool_call(&context, ActorId::Agent(agent), TurnId::new(), "1").await), "not of another turn");
    assert!(not_running(channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "3").await), "no such call");

    // The second call, still to run, first; then the one that runs.
    channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "2").await.expect("it waits to run");
    channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "1").await.expect("it runs");
    assert!(not_running(channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "1").await), "once");

    let finished = answered(&mut events, agent).await;
    let results: Vec<_> = finished.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.iter().map(|r| r.status).collect::<Vec<_>>(), [ToolResultStatus::Interrupted, ToolResultStatus::NotExecuted], "{finished:?}");
    assert!(results[0].started_at.is_some() && results[1].started_at.is_none(), "the first ran, the second never did");
    let alice = channel.actor_of(context.initiator_id).expect("she takes part").name().to_string();
    assert!(results.iter().all(|r| r.content.contains(&format!("{alice} interrupted this tool call"))), "the model is told who: {results:?}");
    let said: Vec<_> = finished.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["never given"], "the turn goes on with the results");
    assert_eq!(model.0.lock().unwrap().len(), 0, "the model was asked again");
    running.abort();
}

#[tokio::test]
async fn an_agent_removed_while_a_tool_runs_gives_up_its_turn_and_keeps_the_call() {

    let (Stuck { channel, context, agent, running, .. }, _) = with_an_agent_in_a_long_tool().await;

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // Within the grace of a stop (issue 161): the turn is given up rather
    // than the actor aborted with the turn left as it was before the tool.
    let stored = tokio::time::timeout(STOP_GRACE, async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn.is_none() && !given_up_turns(&state).is_empty() { break state }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the turn is given up before the grace runs out");
    assert_eq!(given_up_turns(&stored)[0].1, GivenUpReason::Left);
    let interrupted = stored.transcript.messages().iter().flat_map(|m| m.tool_results.clone())
        .find(|r| r.tool_name == "sleep").map(|r| r.content).unwrap_or_default();
    assert!(interrupted.contains("interrupted"), "the call that ran stays, with how it ended: {interrupted}");

    // The agent tells the channel of its turn's end as it leaves, and
    // the channel records it after the call it said (issues 258, 262), and
    // that it left after that (issue 408).
    let entries = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let entries = channel.history_entries().await;
            if matches!(entries.last(), Some(HistoryEntry::ParticipantLeft { .. })) { break entries }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the channel records that it left");
    let ended = match &entries[entries.len() - 2] {
        HistoryEntry::TurnGivenUp { actor, reason, .. } => Some((*actor, reason.clone())),
        _ => None,
    };
    assert_eq!(ended, Some((ActorId::Agent(agent), GivenUpReason::Left)), "the turn's end comes before: {entries:?}");
    assert!(matches!(entries.last(), Some(HistoryEntry::ParticipantLeft { participant, by, .. }) if *participant == ActorId::Agent(agent) && *by == context.caller_id), "in the name of who removed it");
    assert!(channel.whole_history().await.iter().any(|m| m.tool_calls.iter().any(|c| c.function_name == "sleep")), "what it said stays");
    running.abort();
}

#[tokio::test]
async fn an_agent_stopped_in_a_model_call_keeps_its_turn_and_asks_again_when_its_channel_runs_again() {
    let (Stuck { channel, agent, running, .. }, model) = asking_a_gated_model(&["never given", "Answered."]).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(stored.processing_state, ActorState::Planning, "stored at the step it stood at");
    assert!(stored.turn.is_some(), "the turn is kept");
    assert!(given_up_turns(&stored).is_empty(), "not given up");

    let (_restarted, mut events, running) = run_again(&channel).await;
    asked(&model, 2).await;
    model.gate.add_permits(1);
    let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Answered."], "the turn resumed with the call it stood at");
    running.abort();
}

/// The call under way is interrupted and recorded so; the one still to
/// run is not run when the turn resumes, but told the model as not
/// executed, as after a restart (issue 75) - and the turn goes on.
#[tokio::test]
async fn an_agent_stopped_while_a_tool_runs_records_the_call_as_stopped_and_goes_on_when_its_channel_runs_again() {
    let (Stuck { channel, agent, running, .. }, model) = with_an_agent_in_long_tools(&["1", "2"]).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(stored.processing_state, ActorState::ExecutingTools, "the second call has no result yet");
    assert!(given_up_turns(&stored).is_empty(), "not given up");

    let (_restarted, mut events, running) = run_again(&channel).await;
    let finished = answered(&mut events, agent).await;
    let results: Vec<_> = finished.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.iter().map(|r| r.status).collect::<Vec<_>>(), [ToolResultStatus::Interrupted, ToolResultStatus::NotExecuted], "{finished:?}");
    assert!(results[0].content.contains("because the session was stopped"), "the model is told why: {}", results[0].content);
    assert!(results[0].started_at.is_some() && results[1].started_at.is_none(), "the first ran, the second never did");
    let said: Vec<_> = finished.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["never given"], "the turn goes on with the results");
    assert_eq!(model.0.lock().unwrap().len(), 0, "the model was asked again");
    running.abort();
}

#[tokio::test]
async fn an_idle_agent_stopped_ends_at_once_where_it_stands() {
    let model = Arc::new(Scripted(Mutex::new([model_says("Hello.")].into())));
    let Stuck { channel, agent, helper, mut events, running, .. } = with_an_agent(model, Default::default()).await;
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(stored.processing_state, ActorState::Idle);
    assert!(stored.turn.is_none() && given_up_turns(&stored).is_empty(), "nothing under way, nothing given up");
}

#[tokio::test]
async fn an_agent_whose_channel_is_deleted_while_a_tool_runs_gives_its_turn_up() {
    let (Stuck { channel, agent, running, .. }, _) = with_an_agent_in_a_long_tool().await;

    let leaving = std::time::Instant::now();
    channel.leave_all_actors().await;
    assert!(leaving.elapsed() < STOP_GRACE, "the actors ended by themselves");
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turn.is_none(), "the turn is not kept");
    assert_eq!(given_up_turns(&stored).iter().map(|(_, reason, _)| reason.clone()).collect::<Vec<_>>(), [GivenUpReason::Left]);
    tokio::time::timeout(Duration::from_secs(1), running).await.expect("and the channel ended with them").ok();
}

#[tokio::test]
async fn an_agent_removed_while_waiting_for_approval_gives_up_its_turn() {

    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(Mutex::new([call].into()))), components).await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // It stops waiting and gives the turn up, instead of being aborted
    // with the turn left in its state.
    let stored = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn.is_none() && !given_up_turns(&state).is_empty() { break state }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the turn is given up");
    assert_eq!(given_up_turns(&stored)[0].1, GivenUpReason::Left);
    // The request it waited for is withdrawn, as for any turn given up
    // (issue 262).
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::ApprovalRequestWithdrawn { .. }) { break }
        }
    }).await.expect("the approval request is withdrawn");
    running.abort();
}

#[tokio::test]
async fn a_call_with_invalid_arguments_is_answered_without_asking_anyone() {
    use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    // A rule asks for every `create_plan` (issue 13); this call's
    // arguments are no plan.
    let not_a_plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "create_plan".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let answer = Message { role: MessageRole::Assistant, content: Some("Let me answer directly.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "plan".to_string(),
        kind: CREATE_PLAN_TOOL_KIND.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let built = agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted(Mutex::new([not_a_plan, answer].into()))), components).await;
    let ask = ApprovalRule::new(ToolMatcher { tool: "create_plan".to_string(), arguments: Default::default() }, RuleDecision::Ask, Reach::Everywhere, None).unwrap();
    built.0.update_approval_rules(&SecurityContext::system(), vec![ask]).await.expect("the system may");
    let Stuck { agent, helper, mut events, running, .. } = start(built).await;

    // The turn ends without anyone approving anything: the call was
    // answered as invalid, and the model tried again.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event { break messages }
            }
        }
    }).await.expect("the turn ends without an approval");
    let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.len(), 1, "{ended:?}");
    assert_eq!(results[0].status, ToolResultStatus::InvalidArguments, "the model is asked to correct the call");
    assert_eq!(ended.last().and_then(|m| m.content.as_deref()), Some("Let me answer directly."));
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn a_cancel_interrupts_a_model_call_that_is_in_flight() {
    let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;
    let agent_id = ActorId::Agent(agent);

    channel.cancel(&context).await.expect("permitted");

    // The turn ends at once - with an empty answer, so that observers
    // stop waiting - instead of when the model would have answered.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == agent_id && matches!(envelope.event, ActorEvent::TurnFinished { .. }) {
                break envelope;
            }
        }
    }).await.expect("the cancelled turn ends without the model");
    assert!(matches!(ended.event, ActorEvent::TurnFinished { ref messages, .. } if messages.is_empty()));
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn a_user_learns_that_a_turn_cancelled_before_it_said_anything_ended() {
    let Stuck { channel, context, agent, running, .. } = stuck_in_a_model_call().await;
    let mut followed = channel.user_actor(alice_of(&context)).unwrap().watch();

    channel.cancel(&context).await.expect("permitted");

    // Nothing to hear of it - but the end, for whoever shows the answer
    // as under way (issue 224).
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = followed.recv().await.expect("the user's actor runs");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event { break messages }
            }
        }
    }).await.expect("the user learns that the answer ended");
    assert!(ended.is_empty(), "with nothing said: {ended:?}");
    running.abort();
}

#[tokio::test]
async fn a_removed_agent_gives_its_turn_up_says_so_and_leaves_an_idle_state_behind() {

    let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // The turn ends without an answer, and the agent says so itself
    // (issue 262); the state in the store is the one a later actor of
    // the agent starts from: idle, with no turn to resume.
    settled(&helper, ActorState::Idle).await;
    let stored = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.processing_state == ActorState::Idle { break state }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the idle state is stored");
    assert!(stored.turn.is_none(), "the turn was abandoned, not stored");
    assert!(stored.transcript.messages().iter().all(|m| m.sender_id != ActorId::Agent(agent)), "nothing of it was recorded");
    let farewell = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::TurnGivenUp { reason, .. } => break reason,
                ActorEvent::TurnFinished { .. } => panic!("a stopped agent announces no answer"),
                _ => {}
            }
        }
    }).await.expect("the agent tells the channel it gave its turn up");
    assert_eq!(farewell, GivenUpReason::Left);
    // The channel is still running: alice is still in it.
    assert!(!running.is_finished());
    running.abort();
}

#[tokio::test]
async fn an_agent_that_starts_hears_another_agents_answer_as_its_words_without_the_tool_traffic() {
    use caelumora_model::message::MessageId;
    use caelumora_model::tool::{ToolCall, ToolCallResult, ToolVisibility};
    let (channel, _, agent, _) = agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), Default::default()).await;
    let other = ActorId::Agent(AgentId::new());
    let call = ToolCall { id: Some("1".to_string()), function_name: "noop".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let result = ToolCallResult::of(&call, Default::default(), "done".to_string(), Classification::CHANNEL_DEFAULT);
    // Another agent's answer, as the channel's history keeps it.
    let answer = Message { id: MessageId::new(), sender_id: other, sender_name: "hermes".to_string(), role: MessageRole::Assistant, content: Some("Let me check.".to_string()), tool_calls: vec![call], ..Message::new(Classification::CHANNEL_DEFAULT) };
    let tool = Message { id: MessageId::new(), sender_id: other, role: MessageRole::Tool, content: Some("done".to_string()), tool_results: vec![result], ..Message::new(Classification::CHANNEL_DEFAULT) };

    channel.record_said(vec![answer.clone(), tool.clone()]).await;
    let state = channel.run_until(agent, |state| state.last_message == Some(answer.id)).await;

    let heard = state.transcript.messages();
    assert_eq!(heard.len(), 1, "nothing of the tool message: {heard:?}");
    assert_eq!((heard[0].id, heard[0].role, heard[0].content.as_deref()), (answer.id, MessageRole::User, Some("Let me check.")), "the other's words, as another's");
    assert!(heard[0].tool_calls.is_empty(), "not the other's call, as if its own");
    let kept: Vec<_> = channel.visible_messages(ActorId::Agent(agent)).await.iter().map(|m| (m.id, m.role)).collect();
    assert_eq!(kept, [(answer.id, MessageRole::Assistant)], "the channel's history gives it what it heard as it was said; the tool traffic, which it would not have heard live, is not its (issues 258, 417)");
}

#[tokio::test]
async fn an_agent_that_starts_takes_in_what_it_missed_as_what_it_hears() {
    use caelumora_model::message::MessageId;
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("filter", "test_no_secrets")] };
    let (channel, context, agent, _) = agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), components).await;
    let said = |text: &str| Message {
        id: MessageId::new(), role: MessageRole::User, sender_id: context.initiator_id, content: Some(text.to_string()),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let secret = said("the secret is 42");

    channel.record_said(vec![said("hello"), secret.clone()]).await;
    let state = channel.run_until(agent, |state| state.last_message == Some(secret.id)).await;

    let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
    assert_eq!(contents(channel.visible_messages(ActorId::Agent(agent)).await), ["hello", "the secret is 42"], "the channel's history holds all it missed, for it");
    assert_eq!(contents(state.transcript.messages()), ["hello"], "its transcript what passed its processors");
}

#[tokio::test]
async fn all_an_agent_was_offered_is_in_its_channels_history_and_what_passes_its_processors_in_its_transcript() {
    let hi = Message { role: MessageRole::Assistant, content: Some("Hi.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("filter", "test_no_secrets")] };
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(Scripted(Mutex::new([hi].into()))), components).await;
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;

    // Heard between turns; the turn it starts waits for a model that
    // does not answer any more.
    channel.user_actor(alice_of(&context)).unwrap().send_message("the secret is 42".to_string(), None).await.unwrap();
    settled(&helper, ActorState::Planning).await;

    let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(contents(channel.visible_messages(ActorId::Agent(agent)).await).contains(&"the secret is 42".to_string()), "the channel's history holds all it was offered, for it");
    assert!(!contents(state.transcript.messages()).contains(&"the secret is 42".to_string()), "its transcript only what passed");
    assert!(contents(state.transcript.messages()).contains(&"hello".to_string()));
    running.abort();
}

/// A channel whose agent answered alice's "hello" with "Hi." and then
/// heard "take a nap", which it answers by calling the sleep tool, its
/// context full once that call is in - 910 of 1000 -, and then by
/// `afterwards`, one answer a call.
async fn with_a_full_context(afterwards: Vec<Message>) -> Stuck {
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let nap = ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: serde_json::json!({ "seconds": 0 }), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let mut script = vec![measured(100, model_says("Hi.")), measured(900, Message { tool_calls: vec![nap], ..model_says("") })];
    script.extend(afterwards);
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "sleep".to_string(),
        kind: SLEEP_TOOL_KIND.to_string(),
        params: serde_json::json!({ "requires_approval": false }),
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    let mut stuck = with_an_agent(Arc::new(Scripted(Mutex::new(script.into()))), components).await;
    answered(&mut stuck.events, stuck.agent).await;
    settled(&stuck.helper, ActorState::Idle).await;
    stuck.channel.user_actor(alice_of(&stuck.context)).unwrap().send_message("take a nap".to_string(), None).await.unwrap();
    stuck
}

/// `message` as a call of a model whose context holds 1000 tokens
/// answered it, given `prompt` tokens.
fn measured(prompt: u64, message: Message) -> Message {
    Message { prompt_tokens: Some(prompt), completion_tokens: Some(10), context_size: Some(1000), ..message }
}

/// An answer of the model, `text`.
fn model_says(text: &str) -> Message {
    Message { id: MessageId::new(), role: MessageRole::Assistant, content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) }
}

/// What the agent `agent` told of its activities - its state `Busy`, an
/// activity begun, one ended with what it said - until its turn ended,
/// and how it ended: its answer, or why it was given up.
async fn activities_until_the_turn_ends(events: &mut broadcast::Receiver<Envelope>, agent: AgentId) -> (Vec<String>, Result<Vec<Message>, String>) {
    let mut told = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::StateChanged(ActorState::Busy) => told.push("busy".to_string()),
                ActorEvent::ActivityStarted { activity, .. } => told.push(format!("started {activity}")),
                ActorEvent::ActivityFinished { output, .. } => told.push(format!("finished: {output}")),
                ActorEvent::TurnFinished { messages, .. } => break Ok(messages),
                ActorEvent::TurnGivenUp { reason, .. } => break Err(reason.cause().to_string()),
                _ => {}
            }
        }
    }).await.expect("the turn ends");
    (told, ended)
}

#[tokio::test]
async fn an_agent_whose_context_fills_up_compacts_it_on_its_own_before_its_next_call() {

    let Stuck { channel, agent, mut events, running, .. } =
        with_a_full_context(vec![measured(800, model_says("They greeted each other.")), measured(300, model_says("Done."))]).await;
    let (told, answer) = activities_until_the_turn_ends(&mut events, agent).await;

    assert_eq!(told, [
        "busy".to_string(),
        "started compacting its context".to_string(),
        "finished: helper compacted its context: 2 messages replaced, 3 kept as they were.".to_string(),
    ], "the users see it compact, and what came of it");
    let said: Vec<_> = answer.expect("answered").iter().filter_map(|m| m.content.clone()).filter(|c| !c.is_empty()).collect();
    assert_eq!(said, ["Done."], "the turn goes on, the summary no part of its answer");
    let recorded = channel.history_entries().await.into_iter().any(|item| matches!(item, HistoryEntry::ActivityFinished { activity, .. } if activity == "compacting its context"));
    assert!(recorded, "the channel records it where it happened");
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let now: Vec<_> = stored.transcript.messages().into_iter().filter_map(|m| m.content).filter(|c| !c.is_empty()).collect();
    assert!(now[0].contains("They greeted each other.") && !now.iter().any(|c| c == "hello" || c == "Hi."), "the older messages replaced: {now:?}");
    assert_eq!(stored.used.map(|u| u.calls), Some(4), "the summary's call counts as used");
    running.abort();
}

#[tokio::test]
async fn a_compaction_that_fails_ends_the_turn() {
    // The model writes no summary.
    let Stuck { channel, agent, mut events, running, .. } = with_a_full_context(vec![model_says("")]).await;
    let (told, ended) = activities_until_the_turn_ends(&mut events, agent).await;

    assert_eq!(told, ["busy".to_string(), "started compacting its context".to_string()]);
    assert!(ended.is_err(), "given up: the call it was to make room for would not fit");
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.transcript.messages().iter().any(|m| m.content.as_deref() == Some("Hi.")), "the transcript as it was");
    running.abort();
}
