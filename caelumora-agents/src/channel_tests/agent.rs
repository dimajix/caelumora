//! The agent actor in a running channel: its turns, plans, retries, approvals, tool calls, and how
//! it is cancelled, removed or lowered.

use super::*;
use crate::components::planning::PLANNING_KIND;
use crate::message::testing::given_up_turns;
use crate::prompt::request_reason::REQUEST_REASON_KIND;
use crate::testing::{activities_until_the_turn_ends, saying};
use crate::tool::calculator::CALCULATOR_KIND;
use crate::tool::sleep::SLEEP_KIND;
use crate::tool::turn_end::TURN_END_KIND;
use crate::turn::MAX_MODEL_CALLS;
use anyhow::Result;
use caelumora_api::model::{LanguageModelClient, ModelError, ModelProfileProvider, ModelResult};
use caelumora_domain::agent::{
    AgentChannelState, AgentId, HearsDuringTurn, HearsFrom, RetryRefused,
};
use caelumora_domain::approval::{ApprovalId, ApprovalRequest, ApprovalStrategy};
use caelumora_domain::channel::{ThreadId, ThreadOrigin};
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::context::ModelProfile;
use caelumora_domain::history::HistoryEntry;
use caelumora_domain::message::testing::make_message;
use caelumora_domain::message::{MessageEvent, MessageId, MessageRole};
use caelumora_domain::tool::testing::make_tool_call;
use caelumora_domain::tool::{ToolDescription, ToolResultStatus};
use caelumora_domain::transcript::TranscriptEntry;
use caelumora_domain::turn::GivenUpReason;
use caelumora_domain::work::TurnId;
use caelumora_kernel::actor::user::UserActor;
use caelumora_kernel::session::SessionLoop;
use caelumora_security::AllowAllPolicy;
use futures::stream;
use futures::stream::BoxStream;
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::broadcast::Receiver;
use tokio::sync::Semaphore;
use tokio::time;

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn an_agent_takes_a_change_in_after_its_turn_and_is_given_what_stands_from_then_on() {
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let line = ThreadId::line(channel.id());
    time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent asks its model about hello");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let hello = channel.whole_history().await[0].id;
    let until = |what: &'static str, done: Box<dyn Fn(&AgentChannelState) -> bool + Send>| {
        let channel = Arc::clone(&channel);
        async move {
            time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(state) = channel.stored_agent_channel_state(agent).await {
                        if done(&state) {
                            break;
                        }
                    }
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{what}"));
        }
    };

    // Modified while the turn about it runs: the turn ends as it began.
    alice
        .modify_message(hello, Some("good evening".to_string()), None, Vec::new())
        .await
        .expect("hers");
    time::timeout(Duration::from_secs(5), async {
        while channel.message_of(hello).await.and_then(|m| m.content).as_deref()
            != Some("good evening")
        {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the channel takes it in");
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["first"]);

    alice.send_message(None, Some("and now?".to_string()), None, Vec::new()).await.unwrap();
    model.gate.add_permits(1);
    assert_eq!(said(answered(&mut events, agent).await), ["second"]);
    let seen = model.seen.lock().unwrap().clone();
    let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(heard(&seen[0], "hello"), "the turn under way saw what was said then");
    assert!(
        heard(&seen[1], "good evening") && !heard(&seen[1], "hello"),
        "the next is given what stands"
    );
    until(
        "the agent's transcript holds the modification",
        Box::new(move |state| {
            state.transcript.messages(line).iter().any(|m| m.id == hello && m.modified.is_some())
        }),
    )
    .await;

    let question = channel
        .whole_history()
        .await
        .iter()
        .find(|m| m.content.as_deref() == Some("and now?"))
        .expect("said")
        .id;
    alice.revoke_message(question).await.expect("hers to take back");
    until(
        "the agent takes it back",
        Box::new(move |state| !state.transcript.messages(line).iter().any(|m| m.id == question)),
    )
    .await;
    assert!(
        channel.message_of(question).await.is_some_and(|m| m.revoked.is_some()),
        "so does the channel"
    );

    // Another's message released (issue 239), as the policy lets alice
    // here: it reaches the agent that said it, which is not its sender.
    let first = channel
        .whole_history()
        .await
        .iter()
        .find(|m| m.content.as_deref() == Some("first"))
        .expect("answered")
        .id;
    alice.reclassify_message(first, Classification::PUBLIC).await.expect("hers to release");
    until(
        "the agent takes the release in",
        Box::new(move |state| {
            state
                .transcript
                .messages(line)
                .iter()
                .any(|m| m.id == first && m.classification == Classification::PUBLIC)
        }),
    )
    .await;
    running.abort();
}

#[tokio::test]
async fn an_agent_asked_to_answer_again_takes_its_answer_back_and_answers_without_it() {
    let model = Arc::new(Gated::saying(&["first", "second"], 1));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();

    alice.request_retry(first.id).await.expect("asked");
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Message(first.id)
    ));
    assert!(matches!(
        shown_by(&mut events, agent, "MessageRevoked").await,
        ActorEvent::MessageRevoked { message, .. } if message == first.id
    ));
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "answered again");

    let seen = model.seen.lock().unwrap().clone();
    let given = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(
        given(&seen[1], "hello") && !given(&seen[1], "first"),
        "given what was said before it, not the answer replaced: {:?}",
        seen[1]
    );
    assert!(
        channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_some()),
        "the channel shows it taken back"
    );
    let history: Vec<_> =
        channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(history, ["hello", "second"], "in its place, the new answer at the end");
    running.abort();
}

#[tokio::test]
async fn an_agent_does_not_answer_again_once_more_was_said() {
    let model = Arc::new(Gated::saying(&["first", "second", "third"], 2));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message(None, Some("and now?".to_string()), None, Vec::new()).await.unwrap();
    answered(&mut events, agent).await;

    alice.request_retry(first.id).await.expect("asked");
    let ActorEvent::RetryRefused { target, reason } =
        shown_by(&mut events, agent, "RetryRefused").await
    else {
        unreachable!()
    };
    assert_eq!(target, RetryTarget::Message(first.id));
    assert_eq!(reason, RetryRefused::NotLast);
    assert!(
        channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_none()),
        "nothing taken back"
    );
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
    time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().len() < calls {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the model is asked {calls} times"));
}

/// Until `text` is said in `channel`, as `alice` sees it - read from the
/// store, since the channel holds a window of its history alone -: what
/// she sees said then.
async fn until_said(channel: &ChannelLoop, alice: UserId, text: &str) -> Vec<Message> {
    time::timeout(Duration::from_secs(10), async {
        loop {
            let said = channel.visible_messages(ActorId::User(alice)).await;
            if said.iter().any(|m| m.content.as_deref() == Some(text)) {
                break said;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{text:?} is said in the channel"))
}

/// The state of `agent` as stored once no turn is under way and its
/// transcript holds `answers` answers with text.
async fn stored_once_answered(
    channel: &ChannelLoop,
    agent: AgentId,
    answers: usize,
) -> AgentChannelState {
    time::timeout(Duration::from_secs(10), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turns().next().is_none()
                && contents_of(&state, ThreadId::line(channel.id()), MessageRole::Assistant).len()
                    == answers
            {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the agent answers {answers} times"))
}

/// What the messages of `role` on the branch of `thread` in the transcript of `state` say, in
/// order.
fn contents_of(state: &AgentChannelState, thread: ThreadId, role: MessageRole) -> Vec<String> {
    state
        .transcript
        .messages(thread)
        .into_iter()
        .filter(|m| m.role == role)
        .filter_map(|m| m.content)
        .collect()
}

/// A nudge that comes while the agent answers, in the authority of that answer, is answered by
/// its turn (issue 567): the agent begins no turn of its own for it - the next call of its model
/// answers what alice says after, which a turn for the nudge would not have heard.
#[tokio::test]
async fn a_nudge_while_the_agent_answers_is_answered_by_its_turn() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) =
        asking_a_gated_model(&["first", "second"]).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.request_response(ActorId::Agent(agent), None).await.expect("asked");
    // The nudge is in the agent's inbox once the channel took it in.
    time::timeout(Duration::from_secs(5), async {
        while !matches!(
            events.recv().await.expect("running").event,
            ActorEvent::NudgeRequested { .. }
        ) {}
    })
    .await
    .expect("the nudge is taken in");

    model.gate.add_permits(1);
    answered(&mut events, agent).await;
    alice
        .send_message(None, Some("@helper and now?".to_string()), None, Vec::new())
        .await
        .expect("said");
    model.gate.add_permits(1);
    asked(&model, 2).await;
    let seen = model.seen.lock().unwrap().clone();
    assert!(
        seen[1].iter().any(|said| said.contains("and now?")),
        "the second call answers what she said after, not the nudge: {:?}",
        seen[1]
    );
    running.abort();
}

#[tokio::test]
async fn an_answer_under_way_asked_for_again_is_given_up_and_answered_anew() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) =
        asking_a_gated_model(&["first", "second"]).await;
    let ActorEvent::ResponseStarted { turn_id: turn } =
        shown_by(&mut events, agent, "ResponseStarted").await
    else {
        unreachable!()
    };
    assert_eq!(channel.turn_of(ActorId::Agent(agent)), Some(turn), "the turn its start announced");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    // Not another turn.
    retry_of_turn_rejected(&channel, alice_of(&context), agent, TurnId::new()).await;

    alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)
    ));
    let ActorEvent::ResponseStarted { turn_id: next } =
        shown_by(&mut events, agent, "ResponseStarted").await
    else {
        unreachable!()
    };
    assert_ne!(next, turn, "a turn of its own answers");
    model.gate.add_permits(1);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["second"],
        "the answer under way never came"
    );

    let seen = model.seen.lock().unwrap().clone();
    assert!(
        seen[1].iter().any(|c| c.contains("hello")),
        "asked about what the given-up turn was asked: {:?}",
        seen[1]
    );
    let history: Vec<_> =
        channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(history, ["hello", "second"], "nothing of the given-up turn reached the channel");
    // And it is over.
    retry_of_turn_rejected(&channel, alice_of(&context), agent, turn).await;
    running.abort();
}

#[tokio::test]
async fn what_was_said_during_a_turn_asked_for_again_is_answered_in_the_one_that_follows() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) =
        asking_a_gated_model(&["first", "second", "third"]).await;
    let ActorEvent::ResponseStarted { turn_id: turn } =
        shown_by(&mut events, agent, "ResponseStarted").await
    else {
        unreachable!()
    };
    let alice_id = alice_of(&context);
    let alice = channel.user_actor(alice_id).unwrap();
    alice
        .send_message(None, Some("and this too".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice_id)
                && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_))
            {
                break;
            }
        }
    })
    .await
    .expect("the message is said in the channel");

    alice.request_retry_of_turn(ActorId::Agent(agent), turn).await.expect("asked");
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)
    ));
    model.gate.add_permits(3);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["second"]);
    time::sleep(Duration::from_millis(200)).await;
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one turn answers both, not one each: {seen:?}");
    assert!(
        seen[1].iter().any(|c| c.contains("hello"))
            && seen[1].iter().any(|c| c.contains("and this too")),
        "{:?}",
        seen[1]
    );
    running.abort();
}

#[tokio::test]
async fn an_older_answer_asked_for_again_leaves_the_answer_under_way_alone() {
    let (Stuck { channel, context, agent, helper, mut events, running }, model) =
        asking_a_gated_model(&["first", "second", "third"]).await;
    model.gate.add_permits(1);
    let first = answered(&mut events, agent).await.pop().expect("an answer");
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message(None, Some("and now?".to_string()), None, Vec::new()).await.unwrap();
    asked(&model, 2).await;

    // Asked while the next answer is under way: it waits for it, and
    // so does what is said meanwhile.
    alice.request_retry(first.id).await.expect("asked");
    alice.send_message(None, Some("one more".to_string()), None, Vec::new()).await.unwrap();
    time::sleep(Duration::from_millis(200)).await;
    assert_eq!(helper.state(), ActorState::Planning, "the answer under way goes on");
    model.gate.add_permits(2);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["second"], "it ends as it began");

    // Judged against what stands, beside the request to answer "one
    // more": not its last answer any more.
    let ActorEvent::RetryRefused { target, reason } =
        shown_by(&mut events, agent, "RetryRefused").await
    else {
        unreachable!()
    };
    assert_eq!((target, reason), (RetryTarget::Message(first.id), RetryRefused::NotLast));
    assert_eq!(said(answered(&mut events, agent).await), ["third"], "and the request is answered");
    assert!(
        channel.message_of(first.id).await.is_some_and(|m| m.revoked.is_none()),
        "nothing taken back"
    );
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
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
            gate: Arc::new(Semaphore::new(permits)),
            seen: Default::default(),
        }
    }

    /// A gated model that says `answers`, the first `permits` of them at once.
    fn saying(answers: &[&str], permits: usize) -> Self {
        Self::new(answers.iter().map(|answer| saying(answer)), permits)
    }
}

#[async_trait::async_trait]
impl LanguageModelClient for Gated {
    async fn chat_completion(
        &self,
        _: ActorId,
        messages: &[Message],
        _: &[ToolDescription],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        self.seen.lock().unwrap().push(messages.iter().filter_map(|m| m.content.clone()).collect());
        let Some(answer) = self.answers.lock().unwrap().pop_front() else {
            return Box::pin(stream::pending());
        };
        let gate = Arc::clone(&self.gate);
        Box::pin(stream::once(async move {
            gate.acquire().await.unwrap().forget();
            Ok(MessageEvent::Complete(answer))
        }))
    }
}

impl ModelProfileProvider for Gated {
    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

#[tokio::test]
async fn what_a_user_says_during_a_turn_is_answered_after_it() {
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent asks its model about hello");

    // Taken while the turn waits for its model: nothing holds the user back.
    let alice = alice_of(&context);
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("and this too".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice)
                && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_))
            {
                break;
            }
        }
    })
    .await
    .expect("the message is said in the channel");
    model.gate.add_permits(2);

    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["first"],
        "the running turn ends as it began"
    );
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["second"],
        "and a turn of its own answers the message"
    );
    let seen = model.seen.lock().unwrap().clone();
    let heard = |call: &[String], text: &str| call.iter().any(|c| c.contains(text));
    assert!(
        heard(&seen[0], "hello") && !heard(&seen[0], "and this too"),
        "the running turn does not see it"
    );
    assert!(heard(&seen[1], "and this too"), "the next one does");
    running.abort();
}

/// Issue 223: an agent whose turn outlasts more than its inbox holds - two
/// hundred messages, a modification of one of them, a command and a message
/// after it - hears all of it once its turn is over, in order and as it
/// stands, and carries the command out where it came.
#[tokio::test]
async fn an_agent_whose_turn_outlasts_its_inbox_hears_all_of_it_afterwards() {
    use caelumora_domain::component::ComponentDescriptor;
    const SAID: usize = 200;
    let model = Arc::new(Gated::saying(&["first", "second"], 0));
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor::new("marking", "test_marking")],
    };
    // What the channel takes in is not watched: more than a watcher holds.
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    for n in 0..SAID {
        alice
            .send_message(None, Some(format!("message {n}")), None, Vec::new())
            .await
            .expect("taken during a turn");
    }
    let said = until_said(&channel, alice_of(&context), &format!("message {}", SAID - 1)).await;
    let first =
        said.into_iter().find(|m| m.content.as_deref() == Some("message 0")).expect("said first");
    alice
        .modify_message(first.id, Some("message 0, corrected".to_string()), None, Vec::new())
        .await
        .expect("hers to modify");
    alice.execute_command(None, "mark @helper during").await.expect("sent to the agent");
    alice
        .send_message(None, Some("after the command".to_string()), None, Vec::new())
        .await
        .expect("said");
    until_said(&channel, alice_of(&context), "after the command").await;
    model.gate.add_permits(2);

    let state = stored_once_answered(&channel, agent, 2).await;
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::Assistant),
        ["first", "second"],
        "the turn ends as it began, and one turn answers all of it"
    );
    let heard = contents_of(&state, ThreadId::line(channel.id()), MessageRole::User);
    let mut expected: Vec<String> = vec!["hello".to_string(), "message 0, corrected".to_string()];
    expected.extend((1..SAID).map(|n| format!("message {n}")));
    expected.push("after the command".to_string());
    assert_eq!(heard, expected, "all of it, once, in order, as it stands");
    let seen = model.seen.lock().unwrap().clone();
    assert!(
        seen[1].iter().any(|c| c.contains("after the command")),
        "the next turn is given all of it"
    );
    running.abort();
}

/// Issue 385: a channel that stops while an agent is in its turn, and runs
/// again: what was delivered to the agent and not taken in - a message, a
/// request to answer it, a change of what was said - is delivered again from
/// the history, and taken in once; what it took in and stored is not
/// delivered again.
#[tokio::test]
async fn what_an_agent_was_delivered_and_did_not_store_before_a_restart_is_delivered_again_once() {
    let (Stuck { channel, context, agent, running, .. }, model) =
        asking_a_gated_model(&["never given", "Answered.", "Again."]).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let line = ThreadId::line(channel.id());
    let hello = channel
        .whole_history()
        .await
        .into_iter()
        .find(|m| m.content.as_deref() == Some("hello"))
        .expect("said");
    alice
        .send_message(None, Some("and this too".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    alice
        .modify_message(hello.id, Some("hello, corrected".to_string()), None, Vec::new())
        .await
        .expect("hers to modify");
    time::timeout(Duration::from_secs(5), async {
        while channel.message_of(hello.id).await.and_then(|m| m.content).as_deref()
            != Some("hello, corrected")
        {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the change is recorded");

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(
        stored
            .transcript
            .messages(line)
            .iter()
            .all(|m| m.content.as_deref() != Some("and this too")),
        "not taken in before the stop"
    );

    let (_restarted, mut events, running) = run_again(&channel).await;
    asked(&model, 2).await;
    model.gate.add_permits(2);
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["Answered."], "the turn resumed");
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["Again."],
        "and what it was delivered asks it again"
    );
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let heard: Vec<String> = state
        .transcript
        .messages(line)
        .into_iter()
        .filter(|m| m.role == MessageRole::User)
        .filter_map(|m| m.content)
        .collect();
    assert_eq!(heard, ["hello, corrected", "and this too"], "each once, as it stands");
    running.abort();
}

/// A gated model whose first answer calls the calculator and whose next say "done" and "again",
/// its agent hearing between the steps of a turn as `hears` says (issue 431): the agent asked
/// about "hello", the call held back, and alice saying "and this too" meanwhile.
async fn told_while_calculating(hears: HearsDuringTurn) -> (Stuck, Arc<Gated>) {
    let (calculate, components) = calculator();
    let model = Arc::new(Gated::new([calculate, saying("done"), saying("again")], 0));
    let mut stuck = with_an_agent_hearing(hears, Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let alice = alice_of(&stuck.context);
    stuck
        .channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("and this too".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = stuck.events.recv().await.expect("the channel is running");
            if let ActorEvent::CompleteMessageGenerated(message) = envelope.event {
                if message.content.as_deref() == Some("and this too") {
                    break;
                }
            }
        }
    })
    .await
    .expect("the message is said in the channel");
    (stuck, model)
}

/// Where in `call` - what a model was given - a content with `text` is.
fn given_at(call: &[String], text: &str) -> Option<usize> {
    call.iter().position(|content| content.contains(text))
}

#[tokio::test]
async fn what_a_user_says_between_the_steps_of_a_turn_is_given_to_its_next_step_and_answered_by_it()
{
    let (Stuck { agent, mut events, running, .. }, model) =
        told_while_calculating(HearsDuringTurn { from: HearsFrom::Users, ..Default::default() })
            .await;
    model.gate.add_permits(3);

    let answer = answered(&mut events, agent).await;
    assert_eq!(
        answer.last().and_then(|m| m.content.as_deref()),
        Some("done"),
        "the turn ends after its tool step"
    );
    let seen = model.seen.lock().unwrap().clone();
    assert!(
        given_at(&seen[0], "and this too").is_none(),
        "the first step was asked before it was said"
    );
    let heard = given_at(&seen[1], "and this too").expect("the next step is given it");
    let told =
        given_at(&seen[1], "while you were working").expect("and told that it was said meanwhile");
    assert!(heard < told, "after what was heard: {:?}", seen[1]);
    time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        model.seen.lock().unwrap().len(),
        2,
        "the turn answered its request: no other follows"
    );
    running.abort();
}

#[tokio::test]
async fn an_agent_that_hears_nobody_during_a_turn_answers_what_was_said_after_it() {
    let (Stuck { agent, mut events, running, .. }, model) =
        told_while_calculating(HearsDuringTurn::default()).await;
    model.gate.add_permits(3);

    let said = |messages: Vec<Message>| messages.last().and_then(|m| m.content.clone());
    assert_eq!(said(answered(&mut events, agent).await).as_deref(), Some("done"));
    assert_eq!(
        said(answered(&mut events, agent).await).as_deref(),
        Some("again"),
        "a turn of its own answers it"
    );
    let seen = model.seen.lock().unwrap().clone();
    assert!(given_at(&seen[1], "and this too").is_none(), "the running turn does not hear it");
    assert!(
        given_at(&seen[2], "and this too").is_some()
            && given_at(&seen[2], "while you were working").is_none(),
        "the next turn does, as it hears between turns"
    );
    running.abort();
}

/// Issue 439: an agent whose personality has the component `request_reason`, chosen by
/// the channel's policy, is told so before the message it answers - and how to stay silent; one
/// mentioned is told nothing.
#[tokio::test]
async fn an_agent_only_chosen_is_told_so_and_one_mentioned_is_not() {
    let component = |id: &str, kind: &str| ComponentDescriptor {
        id: id.to_string(),
        kind: kind.to_string(),
        params: Value::Null,
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    };
    let components = ComponentConfig {
        component_nodes: vec![
            component("stop", TURN_END_KIND),
            component("reason", REQUEST_REASON_KIND),
        ],
    };
    let model = Arc::new(Gated::saying(&["Hi.", "Yes."], 1));
    let Stuck { channel, context, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let seen = model.seen.lock().unwrap().clone();
    let told = given_at(&seen[0], "[Why you are asked to respond]")
        .expect("chosen for \"hello\": told why");
    assert!(
        seen[0][told].contains("Nobody mentioned you")
            && seen[0][told].contains("call `silence` and write nothing."),
        "{:?}",
        seen[0][told]
    );
    assert!(
        given_at(&seen[0], "hello").is_some_and(|hello| told < hello),
        "before the message it answers: {:?}",
        seen[0]
    );

    channel
        .user_actor(alice_of(&context))
        .unwrap()
        .send_message(None, Some("@helper, are you there?".to_string()), None, Vec::new())
        .await
        .expect("sent");
    asked(&model, 2).await;
    let seen = model.seen.lock().unwrap().clone();
    assert!(given_at(&seen[1], "are you there?").is_some(), "the next turn hears the mention");
    assert!(
        given_at(&seen[1], "[Why you are asked to respond]").is_none(),
        "and, mentioned, is told nothing: {:?}",
        seen[1]
    );
    running.abort();
}

/// What alice says `n`-th in the tests of issue 434: unique as a part of a
/// content - "message 1" would be part of "message 10".
fn numbered(text: &str, n: usize) -> String {
    format!("{text} {n:03}.")
}

/// Asserts that `call` - what a model was given - holds each of `texts`
/// once, in order.
fn given_once_in_order(call: &[String], texts: &[String]) {
    let mut at = Vec::new();
    for text in texts {
        let holding: Vec<usize> = call
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains(text.as_str()))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(holding.len(), 1, "{text:?} is given once: {call:?}");
        at.push(holding[0]);
    }
    assert!(at.is_sorted(), "in order: {call:?}");
}

/// Issue 434: an agent that hears during a turn takes in, between two of
/// its steps, more than its inbox's queue holds - the rest read from the
/// channel's history behind it (issue 223) -, gives the next step all of
/// it, in order and once, and answers the requests among it with that turn.
#[tokio::test]
async fn an_agent_hearing_during_a_turn_hears_what_its_inbox_reads_from_the_history() {
    const SAID: usize = 200;
    let (calculate, components) = calculator();
    let model = Arc::new(Gated::new([calculate, saying("done"), saying("again")], 0));
    let hears = HearsDuringTurn { from: HearsFrom::Users, ..Default::default() };
    // What the channel takes in is not watched: more than a watcher holds.
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent_hearing(hears, Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let alice = alice_of(&context);
    let said: Vec<String> = (0..SAID).map(|n| numbered("message", n)).collect();
    for text in &said {
        channel
            .user_actor(alice)
            .unwrap()
            .send_message(None, Some(text.clone()), None, Vec::new())
            .await
            .expect("taken during a turn");
    }
    until_said(&channel, alice, &said[SAID - 1]).await;
    model.gate.add_permits(3);

    let state = stored_once_answered(&channel, agent, 1).await;
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::Assistant),
        ["done"],
        "the turn ends after its tool step"
    );
    let mut heard = vec!["hello".to_string()];
    heard.extend(said.iter().cloned());
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::User),
        heard,
        "all of it, once, in order"
    );
    let seen = model.seen.lock().unwrap().clone();
    given_once_in_order(&seen[1], &said);
    let told =
        given_at(&seen[1], "while you were working").expect("told that it was said meanwhile");
    assert!(
        given_at(&seen[1], &said[SAID - 1]).is_some_and(|last| last < told),
        "after all that was heard"
    );
    time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        model.seen.lock().unwrap().len(),
        2,
        "the turn answered the requests: no other follows"
    );
    running.abort();
}

/// Issue 434: a command said during a turn, with more behind it than the
/// queue holds, is a barrier there as in the queue: the turn hears what
/// came before it; after the turn the command is carried out where it
/// came, and what follows it, read from the history, is taken in in
/// order, once, and answered by a turn of its own.
#[tokio::test]
async fn behind_a_barrier_an_agent_hearing_during_a_turn_waits_for_what_the_history_holds() {
    const AFTER: usize = 150;
    let (calculate, mut components) = calculator();
    components.component_nodes.push(ComponentDescriptor::new("marking", "test_marking"));
    let model = Arc::new(Gated::new([calculate, saying("done"), saying("again")], 0));
    let hears = HearsDuringTurn { from: HearsFrom::Users, ..Default::default() };
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent_hearing(hears, Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice
        .send_message(None, Some("before the command.".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    alice.execute_command(None, "mark @helper during").await.expect("sent to the agent");
    let after: Vec<String> = (0..AFTER).map(|n| numbered("after the command", n)).collect();
    for text in &after {
        alice
            .send_message(None, Some(text.clone()), None, Vec::new())
            .await
            .expect("taken during a turn");
    }
    until_said(&channel, alice_of(&context), &after[AFTER - 1]).await;
    model.gate.add_permits(3);

    let state = stored_once_answered(&channel, agent, 2).await;
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::Assistant),
        ["done", "again"],
        "the turn, and one turn after it"
    );
    let mut heard = vec!["hello".to_string(), "before the command.".to_string()];
    heard.extend(after.iter().cloned());
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::User),
        heard,
        "all of it, once, in order"
    );
    let seen = model.seen.lock().unwrap().clone();
    assert!(
        given_at(&seen[1], "before the command.").is_some(),
        "the turn hears what came before the command"
    );
    assert!(
        given_at(&seen[1], "after the command").is_none(),
        "and nothing behind it: {:?}",
        seen[1]
    );
    given_once_in_order(&seen[2], &after);
    assert_eq!(seen.len(), 3, "one turn answers what waited");
    running.abort();
}

/// Issue 434: a turn that heard a message and its request between two
/// steps, and is stopped in the next, resumes once its channel runs again:
/// the message is given to it once, neither is delivered again, and no
/// other turn answers the request.
#[tokio::test]
async fn what_a_turn_heard_before_a_restart_is_neither_delivered_nor_answered_again() {
    let (calculate, components) = calculator();
    let model = Arc::new(Gated::new(
        [calculate, saying("never given"), saying("done"), saying("again")],
        0,
    ));
    let hears = HearsDuringTurn { from: HearsFrom::Users, ..Default::default() };
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent_hearing(hears, Arc::<Gated>::clone(&model), components).await;
    asked(&model, 1).await;
    let alice = alice_of(&context);
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("and this too".to_string()), None, Vec::new())
        .await
        .expect("taken during a turn");
    until_said(&channel, alice, "and this too").await;
    model.gate.add_permits(1);
    asked(&model, 2).await;
    assert!(
        given_at(&model.seen.lock().unwrap()[1], "and this too").is_some(),
        "the next step is given it"
    );
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turn(ThreadId::line(channel.id())).is_some(), "the turn is under way");
    assert_eq!(
        contents_of(&stored, ThreadId::line(channel.id()), MessageRole::User),
        ["hello", "and this too"],
        "and what it heard stored before its model is called"
    );

    stop_and_wait(&channel, running).await;
    let (restarted, mut events, running) = run_again(&channel).await;
    asked(&model, 3).await;
    model.gate.add_permits(2);
    let answer = answered(&mut events, agent).await;
    assert_eq!(answer.last().and_then(|m| m.content.as_deref()), Some("done"), "the turn resumed");
    let seen = model.seen.lock().unwrap().clone();
    given_once_in_order(&seen[2], &["and this too".to_string()]);
    time::sleep(Duration::from_millis(100)).await;
    assert_eq!(model.seen.lock().unwrap().len(), 3, "no other turn answers the request");
    let state = restarted.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        contents_of(&state, ThreadId::line(channel.id()), MessageRole::User),
        ["hello", "and this too"],
        "heard once"
    );
    running.abort();
}

#[tokio::test]
async fn an_agent_takes_note_of_who_left_without_responding() {
    let model = Arc::new(Gated::saying(&["first", "second"], 1));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    let line = ThreadId::line(channel.id());
    answered(&mut events, agent).await;

    channel
        .remove_participant(&context, context.initiator_id)
        .await
        .expect("she may leave her channel");

    // In its transcript, where the next turn's model is told of it, and in
    // the channel's history as the agent saw it (issues 408, 417); it asks
    // nobody to respond.
    let state = time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state
                .transcript
                .entries(line)
                .any(|node| matches!(node.entry, TranscriptEntry::ParticipantLeft { .. }))
            {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent takes note of it");
    assert!(state.transcript.entries(line).any(|node| matches!(
        node.entry,
        TranscriptEntry::ParticipantLeft { participant, by, .. }
            if participant == context.initiator_id && by == context.initiator_id
    )));
    assert!(matches!(
        channel.visible_entries(ActorId::Agent(agent)).await.last(),
        Some(HistoryEntry::ParticipantLeft { participant, by, .. })
            if *participant == context.initiator_id && *by == context.initiator_id
    ));
    time::sleep(Duration::from_millis(50)).await;
    assert_eq!(model.seen.lock().unwrap().len(), 1, "no turn begins for it");
    running.abort();
}

#[tokio::test]
async fn an_agent_asked_several_times_during_a_turn_answers_once_after_it() {
    let model = Arc::new(Gated::saying(&["first", "second", "third"], 0));
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), Default::default()).await;
    time::timeout(Duration::from_secs(5), async {
        while model.seen.lock().unwrap().is_empty() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent asks its model about hello");

    // Three messages, each asking the agent, while its turn waits.
    let alice = alice_of(&context);
    let queued = ["one", "two", "three"];
    for text in queued {
        channel
            .user_actor(alice)
            .unwrap()
            .send_message(None, Some(text.to_string()), None, Vec::new())
            .await
            .expect("taken during a turn");
    }
    time::timeout(Duration::from_secs(5), async {
        let mut said = 0;
        while said < queued.len() {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::User(alice)
                && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_))
            {
                said += 1;
            }
        }
    })
    .await
    .expect("the messages are said in the channel");
    model.gate.add_permits(3);

    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["first"],
        "the running turn ends as it began"
    );
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["second"],
        "one turn answers what was queued"
    );
    let seen = model.seen.lock().unwrap().clone();
    assert!(
        queued.iter().all(|text| seen[1].iter().any(|c| c.contains(text))),
        "having heard all of it: {:?}",
        seen[1]
    );

    // Nothing is left to answer: told something new, the agent answers
    // it with the next call of its model, not with a stale one.
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("four".to_string()), None, Vec::new())
        .await
        .expect("sent");
    assert_eq!(said(answered(&mut events, agent).await), ["third"]);
    let seen = model.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "one call per turn, one turn per batch");
    assert!(seen[2].iter().any(|c| c.contains("four")), "the third call answers the new message");
    running.abort();
}

#[tokio::test]
async fn a_sleeping_agent_woken_by_a_mention_answers_it() {
    use caelumora_domain::tool::ToolCall;

    let answer = |text: &str, tool_calls| Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        tool_calls,
        ..make_message(MessageId::new())
    };
    let stop = vec![ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({}),
        ..make_tool_call("stop")
    }];
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: TURN_END_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let model = Scripted::new([answer("Good bye.", stop), answer("I am back.", Vec::new())]);
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(model), components).await;
    settled(&helper, ActorState::Sleeping).await;
    let alice = alice_of(&context);

    // The message and the request reach it together, and it was woken
    // before either: it answers (issue 222).
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("@helper, are you there?".to_string()), None, Vec::new())
        .await
        .expect("sent");
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    loop {
        if said(answered(&mut events, agent).await) == ["I am back."] {
            break;
        }
    }
    running.abort();
}

/// Issue 557: what a delegate gives an agent to read wakes it, and it takes it in as given to it -
/// "alice gave you this to read:", as a skill given with `/skill` -, recorded once as what it
/// was given, and answers it.
#[tokio::test]
async fn an_agent_given_something_to_read_wakes_takes_it_in_and_answers() {
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::{Brief, BriefStage, Delegation, ThreadId, Word};
    use caelumora_domain::tool::ToolCall;
    use std::collections::VecDeque;

    let answer = |text: &str, tool_calls| Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        tool_calls,
        ..make_message(MessageId::new())
    };
    let stop = vec![ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({}),
        ..make_tool_call("stop")
    }];
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: TURN_END_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let model = Scripted::new([answer("Good bye.", stop), answer("Found it.", Vec::new())]);
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(model), components).await;
    settled(&helper, ActorState::Sleeping).await;
    let alice = ActorId::User(alice_of(&context));
    // Given back into the line by the delegate of a thread opened at it, which only the tests make.
    let line = ThreadId::line(channel.id());
    let brief = Message {
        sender_id: alice,
        sender_name: "alice".to_string(),
        content: Some("Look this up.".to_string()),
        ..make_message(MessageId::new())
    };
    let given = |thread| {
        let context = EventContext {
            sender_id: ActorId::Delegate(thread),
            initiator_id: alice,
            thread_id: line,
            ..channel.event_context()
        };
        let message = brief.clone();
        let given = ActorEvent::GivenToRead { recipient: ActorId::Agent(agent), message };
        vec![Envelope::new(context, given)]
    };
    // Delegated by the agent to alice, as far as its delegate goes: alice's answer comes back.
    let briefed = Brief {
        message: Message::new(Classification::CHANNEL_DEFAULT),
        stage: BriefStage::Given { at: Position::START },
    };
    let delegation = Delegation {
        giver: ActorId::Agent(agent),
        recipient: alice,
        initiator: alice,
        briefs: VecDeque::from([briefed]),
        word: Word::NotDue,
    };
    channel.forge_delegate(delegation, given).await;

    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    while said(answered(&mut events, agent).await) != ["Found it."] {}
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let given: Vec<_> = stored
        .transcript
        .entries(line)
        .filter_map(|node| match &node.entry {
            TranscriptEntry::Given { message, source: None } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(given.len(), 1, "taken in once, as given to it: {given:?}");
    assert_eq!((given[0].id, given[0].sender_id), (brief.id, alice), "its giver its sender");
    let recorded = channel.history_entries().await;
    assert!(
        recorded.iter().any(|entry| matches!(
            entry,
            HistoryEntry::Given { message } if message.id == brief.id
        )),
        "its record of what it was given: {recorded:?}"
    );
    // What it is given again from the history, as after a restart (issue 223): by name.
    let readable = channel.visible_entries(ActorId::Agent(agent)).await;
    assert!(
        readable.iter().any(|entry| matches!(
            entry,
            HistoryEntry::GivenToRead { message, .. } if message.id == brief.id
        )),
        "read back from the history as its recipient: {readable:?}"
    );
    running.abort();
}

/// Issue 625: an answer that says nothing - what the Ollama client gives of one made of control
/// tokens alone - ends the turn as an empty one, and the agent is idle again.
#[tokio::test]
async fn an_answer_that_says_nothing_ends_the_turn_as_an_empty_one() {
    let nothing =
        Message { role: MessageRole::Assistant, content: None, ..make_message(MessageId::new()) };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new([nothing])), Default::default()).await;

    let ended = answered(&mut events, agent).await;
    assert!(ended.iter().all(|m| !m.says_something()), "{ended:?}");
    settled(&helper, ActorState::Idle).await;
    let history = channel.whole_history().await;
    assert!(
        history.iter().filter(|m| m.sender_id == ActorId::Agent(agent)).all(|m| !m.is_heard()),
        "nothing of it heard: {history:?}"
    );
    running.abort();
}

/// Issue 557: a message the emit processors drop is not said, and the calls it carries are not run:
/// the turn ends without them - a second call of the model would never be answered.
#[tokio::test]
async fn the_calls_of_a_message_dropped_on_its_way_out_are_not_run() {
    use caelumora_domain::tool::ToolCall;

    let calls = Message {
        role: MessageRole::Assistant,
        content: Some("Let me compute.".to_string()),
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("dropped")
        }],
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "dropping".to_string(),
            kind: "test_dropping_calls".to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, agent, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new([calls])), components).await;

    answered(&mut events, agent).await;
    let line = ThreadId::line(channel.id());
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turns().next().is_none(), "the turn ended");
    let said = stored.transcript.messages(line);
    assert!(
        said.iter().all(|m| m.tool_calls.is_empty() && m.role != MessageRole::Tool),
        "neither the calls nor their results: {said:?}"
    );
    running.abort();
}

/// Issue 557: a step of a plan whose message the emit processors drop ends as one answered without
/// calls - nothing of it said, its calls not run - and the plan goes on with its next step.
#[tokio::test]
async fn a_step_whose_message_is_dropped_ends_and_the_plan_goes_on() {
    use caelumora_domain::tool::ToolCall;

    let said = |content: &str| Message {
        role: MessageRole::Assistant,
        content: Some(content.to_string()),
        ..make_message(MessageId::new())
    };
    let calling = |name: &str, arguments| Message {
        tool_calls: vec![ToolCall { id: Some("1".to_string()), arguments, ..make_tool_call(name) }],
        ..said("")
    };
    let steps = serde_json::json!({
        "summary": "two steps",
        "steps": [{ "description": "first" }, { "description": "second" }]
    });
    let script = [
        calling("create_plan", steps),
        calling("dropped", serde_json::json!({})),
        said("second done"),
        said("all done"),
    ];
    let component_nodes =
        [PLANNING_KIND, "test_dropping_calls"].map(|kind| ComponentDescriptor::new(kind, kind));
    let components = ComponentConfig { component_nodes: component_nodes.into() };
    let Stuck { channel, agent, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new(script)), components).await;
    let line = ThreadId::line(channel.id());

    let mut seen = Vec::new();
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::TurnFinished { .. } => break,
                ActorEvent::IntermediateMessageGenerated(m) => {
                    seen.push(format!("intermediate:{}", m.content.unwrap_or_default()))
                }
                ActorEvent::PlanStepStarted { step_index, .. } => {
                    seen.push(format!("started:{step_index}"))
                }
                ActorEvent::PlanStepFinished { step_index, .. } => {
                    seen.push(format!("finished:{step_index}"))
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the turn ends");
    assert_eq!(
        seen,
        ["started:0", "finished:0", "started:1", "intermediate:second done", "finished:1"],
        "the first step ended with nothing said, the second went on"
    );
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let said = stored.transcript.messages(line);
    let dropped = |m: &Message| {
        m.tool_calls.iter().any(|c| c.function_name == "dropped")
            || m.tool_results.iter().any(|r| r.tool_name == "dropped")
    };
    assert!(!said.iter().any(dropped), "the dropped call neither said nor run: {said:?}");
    running.abort();
}

/// Issue 557: the call an emit processor makes of the model's is the one taken into the turn and
/// run - said as it left the processor, labelled as its tool declares: hidden, its owner's.
#[tokio::test]
async fn a_call_an_emit_processor_changes_runs_as_it_changed_it() {
    use caelumora_domain::tool::{ToolCall, ToolVisibility};

    let drafted = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("draft")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("Done.".to_string()),
        ..make_message(MessageId::new())
    };
    let component_nodes = ["test_whispering_tool", "test_renaming_calls"]
        .map(|kind| ComponentDescriptor::new(kind, kind));
    let components = ComponentConfig { component_nodes: component_nodes.into() };
    let model = Scripted::new([drafted, answer]);
    let Stuck { channel, agent, mut events, running, .. } =
        with_an_agent(Arc::new(model), components).await;

    answered(&mut events, agent).await;
    let line = ThreadId::line(channel.id());
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let said = stored.transcript.messages(line);
    let called: Vec<_> = said
        .iter()
        .flat_map(|m| m.tool_calls.iter().map(|c| (c.function_name.clone(), c.visibility)))
        .collect();
    assert_eq!(
        called,
        [("whisper".to_string(), ToolVisibility::HIDDEN)],
        "said as the processor made it, labelled as its tool declares: {said:?}"
    );
    let results: Vec<_> = said
        .iter()
        .flat_map(|m| m.tool_results.iter())
        .map(|r| (r.tool_name.clone(), r.status))
        .collect();
    assert_eq!(results, [("whisper".to_string(), ToolResultStatus::Ok)], "and run");
    running.abort();
}

/// Issue 557: an emit processor that fails closed gives the turn up, as every error of a turn does,
/// as soon as a message streams; one that fails open is left out, and the message is said as it
/// was.
#[tokio::test]
async fn an_emit_processor_that_fails_closed_gives_the_turn_up() {
    let answer = |text: &str| Message {
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        ..make_message(MessageId::new())
    };
    let failing = |fail_policy| ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "failing".to_string(),
            kind: "test_failing_emit".to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy,
        }],
    };
    let model = Scripted::new([answer("Hello.")]);
    let Stuck { agent, mut events, running, .. } =
        with_an_agent(Arc::new(model), failing(ComponentFailPolicy::FailClosed)).await;
    let reason = turn_given_up(&mut events, agent).await;
    assert!(reason.contains("the filter is broken"), "given up, and why: {reason}");
    running.abort();

    let model = Scripted::new([answer("Hello.")]);
    let Stuck { agent, mut events, running, .. } =
        with_an_agent(Arc::new(model), failing(ComponentFailPolicy::FailOpen)).await;
    let said = answered(&mut events, agent).await;
    assert_eq!(said[0].content.as_deref(), Some("Hello."), "said as it was");
    running.abort();
    // Already as a part of it streams: the rest of the stream, which never comes, is not waited
    // for.
    let Stuck { agent, mut events, running, .. } =
        with_an_agent(Arc::new(StreamsAPart), failing(ComponentFailPolicy::FailClosed)).await;
    let reason = turn_given_up(&mut events, agent).await;
    assert!(reason.contains("the filter is broken"), "given up as it streams: {reason}");
    running.abort();
}

/// A model that streams a part of its answer and then nothing more.
#[derive(Debug)]
struct StreamsAPart;

#[async_trait::async_trait]
impl LanguageModelClient for StreamsAPart {
    async fn chat_completion(
        &self,
        _: ActorId,
        _: &[Message],
        _: &[ToolDescription],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        use futures::StreamExt;
        let part = Message {
            role: MessageRole::Assistant,
            content: Some("Hel".to_string()),
            ..make_message(MessageId::new())
        };
        stream::iter([Ok(MessageEvent::Partial(part))]).chain(stream::pending()).boxed()
    }
}

impl ModelProfileProvider for StreamsAPart {
    fn profile(&self) -> ModelProfile {
        ModelProfile::default()
    }
}

#[tokio::test]
async fn an_agent_that_calls_stop_ends_its_turn_with_the_call_and_sleeps() {
    use caelumora_domain::tool::ToolCall;

    // One answer only: a second call of the model would never return.
    let bye = Message {
        role: MessageRole::Assistant,
        content: Some("Good bye.".to_string()),
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("stop")
        }],
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: TURN_END_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, agent, helper, running, .. } =
        with_an_agent(Arc::new(Scripted::new([bye])), components).await;

    // The turn ends with the call - the model is not asked again - and
    // the agent sleeps, as the tool asked; no turn is left behind.
    settled(&helper, ActorState::Sleeping).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turns().next().is_none(), "the turn was taken over");
    assert!(
        channel
            .visible_messages(ActorId::Agent(agent))
            .await
            .iter()
            .any(|m| m.content.as_deref() == Some("Good bye.")),
        "and its answer recorded in the channel's history"
    );
    running.abort();
}

/// An agent tells the users where it stands as it starts and moves
/// (issue 243): through its turn into its sleep - and woken by `/wakeup`,
/// which sends it no message, it tells them so too.
#[tokio::test]
async fn an_agent_tells_the_users_where_it_stands_and_that_it_was_woken() {
    use caelumora_domain::tool::ToolCall;

    let bye = Message {
        role: MessageRole::Assistant,
        content: Some("Good bye.".to_string()),
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("stop")
        }],
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "stop".to_string(),
            kind: TURN_END_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let (channel, context, agent, alice) =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted::new([bye])), components)
            .await;
    // What reaches alice, watched since before it ran.
    let mut shown = channel.user_actor(alice).unwrap().watch();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    assert_eq!(
        told_states(&mut shown, agent, ActorState::Idle).await,
        [ActorState::Idle],
        "where it starts, before it is asked"
    );
    channel
        .user_actor(alice)
        .unwrap()
        .send_message(None, Some("hello".to_string()), None, Vec::new())
        .await
        .unwrap();
    let helper = channel.actor_of(ActorId::Agent(agent)).unwrap();
    assert_eq!(
        told_states(&mut shown, agent, ActorState::Sleeping).await,
        [
            ActorState::Planning,
            ActorState::ExecutingTools,
            ActorState::Reflecting,
            ActorState::Sleeping
        ],
        "each step it moves to, once"
    );

    let woken = channel.wakeup(&context, None).await.expect("alice may wake it");
    assert_eq!(woken, ["helper"]);
    assert_eq!(
        told_states(&mut shown, agent, ActorState::Idle).await,
        [ActorState::Idle],
        "woken without a message, it tells so itself"
    );
    assert_eq!(helper.state(), ActorState::Idle);
    running.abort();
}

#[tokio::test]
async fn a_plan_whose_steps_need_more_calls_than_a_turn_runs_to_its_end() {
    use caelumora_domain::tool::ToolCall;

    // Two steps, each calling a tool more than half as often as a turn may
    // call its model (issue 412).
    let said = |content: &str| Message {
        role: MessageRole::Assistant,
        content: Some(content.to_string()),
        ..make_message(MessageId::new())
    };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("plan".to_string()),
            arguments: serde_json::json!({ "summary": "two steps", "steps": [{ "description": "first" }, { "description": "second" }] }),
            ..make_tool_call("create_plan")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let (calculate, calculator) = calculator();
    let calls = MAX_MODEL_CALLS / 2 + 1;
    let step = |done: &str| {
        (0..calls)
            .map(|_| Message { id: MessageId::new(), ..calculate.clone() })
            .chain([said(done)])
            .collect::<Vec<_>>()
    };
    let script: VecDeque<Message> = [plan]
        .into_iter()
        .chain(step("first done"))
        .chain(step("second done"))
        .chain([said("all done")])
        .collect();
    assert!(
        script.len() > MAX_MODEL_CALLS as usize,
        "more calls than a turn without a plan may make"
    );
    let components = ComponentConfig {
        component_nodes: [ComponentDescriptor {
            id: "plan".to_string(),
            kind: PLANNING_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }]
        .into_iter()
        .chain(calculator.component_nodes)
        .collect(),
    };
    let Stuck { agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new(script)), components).await;

    let final_messages = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::TurnGivenUp { reason, .. } => panic!("given up: {reason:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the turn ends");
    let contents: Vec<_> = final_messages.iter().filter_map(|m| m.content.as_deref()).collect();
    assert_eq!(contents, ["all done"], "the plan came to its final answer");
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_plan_runs_its_steps_as_intermediate_messages_and_ends_with_the_final_answer() {
    use caelumora_domain::tool::ToolCall;

    let said = |content: &str| Message {
        role: MessageRole::Assistant,
        content: Some(content.to_string()),
        ..make_message(MessageId::new())
    };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "summary": "two steps", "steps": [{ "description": "first" }, { "description": "second" }] }),
            ..make_tool_call("create_plan")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let script = [plan, said("first done"), said("second done"), said("all done")];
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: PLANNING_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new(script)), components).await;

    // The plan needs no approval: it runs at once.
    let mut seen = Vec::new();
    let final_messages = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            assert!(
                !matches!(envelope.event, ActorEvent::ApprovalRequested { .. }),
                "nobody is asked"
            );
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::IntermediateMessageGenerated(m) => {
                    seen.push(format!("intermediate:{}", m.content.unwrap_or_default()))
                }
                // Not the plan's call nor the tool's result: they say nothing.
                ActorEvent::CompleteMessageGenerated(m) => {
                    seen.extend(m.content.map(|c| format!("complete:{c}")))
                }
                ActorEvent::PlanStepStarted { step_index, .. } => {
                    seen.push(format!("started:{step_index}"))
                }
                ActorEvent::PlanStepFinished { step_index, .. } => {
                    seen.push(format!("finished:{step_index}"))
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the turn ends");
    assert_eq!(
        seen,
        [
            "started:0",
            "intermediate:first done",
            "finished:0",
            "started:1",
            "intermediate:second done",
            "finished:1",
            "complete:all done",
        ]
    );
    let contents: Vec<_> = final_messages.iter().filter_map(|m| m.content.as_deref()).collect();
    assert_eq!(contents, ["all done"], "the steps are not part of the answer");
    settled(&helper, ActorState::Idle).await;

    // The channel keeps every message as it was said, the steps too,
    // each once, and the turn's end after them (issue 258).
    let history = channel.whole_history().await;
    let kept: Vec<_> = history
        .iter()
        .filter(|m| m.sender_id == ActorId::Agent(agent))
        .map(|m| (m.internal, m.content.clone().unwrap_or_default()))
        .collect();
    assert_eq!(
        kept,
        [
            (false, String::new()),
            (false, String::new()),
            (true, "first done".to_string()),
            (true, "second done".to_string()),
            (false, "all done".to_string()),
        ],
        "the plan's call and its result, the steps, the answer"
    );
    // The call stands with what its tool says it does, as it was admitted (issue 586).
    let described: Vec<_> =
        history.iter().flat_map(|m| &m.tool_calls).filter_map(|c| c.description.clone()).collect();
    assert_eq!(described, ["Proposed plan: two steps\n1. first\n2. second"]);
    // Each step begins before what it says (issue 263).
    let items: Vec<String> = channel
        .history_entries()
        .await
        .into_iter()
        .filter_map(|item| match item {
            HistoryEntry::Said { message, .. }
                if message.sender_id == ActorId::Agent(agent) && message.internal =>
            {
                message.content
            }
            HistoryEntry::PlanStepStarted {
                actor, step_index, total_steps, description, ..
            } if actor == ActorId::Agent(agent) => {
                Some(format!("step {}/{total_steps}: {description}", step_index + 1))
            }
            _ => None,
        })
        .collect();
    assert_eq!(items, ["step 1/2: first", "first done", "step 2/2: second", "second done"]);
    let ended = channel.last_history_entry().await;
    assert!(
        matches!(
            ended,
            Some(HistoryEntry::TurnFinished { turn_id, .. })
                if Some(turn_id) == history.last().and_then(|m| m.turn_id)
        ),
        "{ended:?}"
    );
    running.abort();
}

#[tokio::test]
async fn every_message_of_a_turn_carries_the_turn_its_start_announced() {
    use caelumora_domain::tool::ToolCall;

    // A plan: a call, its result, the steps' internal answers and the
    // final one - every kind of message a turn produces.
    let said = |content: &str| Message {
        role: MessageRole::Assistant,
        content: Some(content.to_string()),
        ..make_message(MessageId::new())
    };
    let plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "summary": "one step", "steps": [{ "description": "first" }] }),
            ..make_tool_call("create_plan")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let script = [plan, said("first done"), said("all done")];
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: PLANNING_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new(script)), components).await;
    let line = ThreadId::line(channel.id());

    let mut started = None;
    let mut shown = Vec::new();
    let mut said = Vec::new();
    let final_messages = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            said.push(envelope.context.event_id);
            match envelope.event {
                ActorEvent::ResponseStarted { turn_id } => started = Some(turn_id),
                ActorEvent::TurnFinished { messages, .. } => break messages,
                ActorEvent::CompleteMessageGenerated(m)
                | ActorEvent::IntermediateMessageGenerated(m) => shown.push(m.turn_id),
                _ => {}
            }
        }
    })
    .await
    .expect("the turn ends");
    settled(&helper, ActorState::Idle).await;

    let started = started.expect("the turn announced itself");
    let ids: HashSet<_> = said.iter().collect();
    assert_eq!(ids.len(), said.len(), "every event of the turn is one of its own (issue 352)");
    assert_eq!(
        shown.len(),
        4,
        "the call, its result, the step's answer and the final answer: {shown:?}"
    );
    assert!(shown.iter().all(|turn| *turn == Some(started)), "each shown as the turn's: {shown:?}");
    assert!(final_messages.iter().all(|m| m.turn_id == Some(started)), "and published so");
    let answer = final_messages
        .iter()
        .find(|m| m.content.as_deref() == Some("all done"))
        .expect("the final answer");
    assert_eq!(
        channel.message_of(answer.id).await.and_then(|m| m.turn_id),
        Some(started),
        "the channel's history keeps it"
    );

    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let (own, heard): (Vec<_>, Vec<_>) = stored
        .transcript
        .messages(line)
        .into_iter()
        .partition(|m| m.sender_id == ActorId::Agent(agent));
    assert!(
        own.len() >= 4 && own.iter().all(|m| m.turn_id == Some(started)),
        "its transcript keeps every message of the turn as the turn's"
    );
    assert!(heard.iter().all(|m| m.turn_id.is_none()), "what it heard is no turn's of its own");
    running.abort();
}

#[tokio::test]
async fn a_turn_is_stored_with_the_step_that_comes_next() {
    use caelumora_domain::tool::ToolCall;

    // The model calls a tool that needs no approval, and then never
    // answers again: the agent waits in `Planning` with the result.
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "expression": "1+1" }),
            ..make_tool_call("calculate")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: CALCULATOR_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, agent, running, .. } =
        with_an_agent(Arc::new(Scripted::new([calculate])), components).await;
    let line = ThreadId::line(channel.id());

    // What a crash now would leave: `Planning`, with the tool's result
    // in the turn - so that a resume asks the model about the result
    // instead of asking again for the call or running the tool twice.
    let stored = time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            let has_result = state.turn(ThreadId::line(channel.id())).is_some_and(|t| {
                state
                    .transcript
                    .messages(line)
                    .iter()
                    .any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty())
            });
            if has_result
                && state
                    .turn(ThreadId::line(channel.id()))
                    .is_some_and(|turn| turn.step == ActorState::Planning)
            {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stored in Planning, with the tool's result");
    assert!(
        stored.turn(ThreadId::line(channel.id())).is_some_and(|t| t.tool_calls.is_empty()),
        "the calls of the step are done"
    );
    running.abort();
}

/// A model that fails where its script says `None`, and answers where
/// it says `Some`; it waits for ever once the script is done.
#[derive(Debug)]
struct FailsWhere(Mutex<VecDeque<Option<Message>>>);

#[async_trait::async_trait]
impl LanguageModelClient for FailsWhere {
    async fn chat_completion(
        &self,
        _: ActorId,
        _: &[Message],
        _: &[ToolDescription],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        match self.0.lock().unwrap().pop_front() {
            Some(Some(answer)) => Box::pin(stream::iter([Ok(MessageEvent::Complete(answer))])),
            Some(None) => Box::pin(stream::iter([Err(ModelError::Backend {
                endpoint: "test".to_string(),
                model: "test".to_string(),
                message: "gone".to_string(),
            })])),
            None => Box::pin(stream::pending()),
        }
    }
}

impl ModelProfileProvider for FailsWhere {
    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// The turn the agent `agent` gives up next, by its `TurnGivenUp`.
async fn given_up_turn(events: &mut Receiver<Envelope>, agent: AgentId) -> TurnId {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            if let ActorEvent::TurnGivenUp { turn_id, .. } = envelope.event {
                break turn_id;
            }
        }
    })
    .await
    .expect("the turn is given up")
}

#[tokio::test]
async fn a_turn_that_failed_is_answered_anew_on_request_and_stays() {
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("second try".to_string()),
        ..make_message(MessageId::new())
    };
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_agent(
        Arc::new(FailsWhere(Mutex::new([None, Some(answer)].into()))),
        Default::default(),
    )
    .await;
    let failed = given_up_turn(&mut events, agent).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice
        .request_retry_of_turn(ActorId::Agent(agent), failed)
        .await
        .expect("its last turn, given up as failed (issue 259)");
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(failed)
    ));
    let said: Vec<_> =
        answered(&mut events, agent).await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(said, ["second try"], "answered anew after it");

    let ended: Vec<_> = channel
        .history_entries()
        .await
        .into_iter()
        .filter_map(|item| match item {
            HistoryEntry::TurnGivenUp { turn_id, .. } => Some(turn_id),
            _ => None,
        })
        .collect();
    assert_eq!(ended, [failed], "the turn given up stays, with its end");
    running.abort();
}

#[tokio::test]
async fn a_turn_given_up_is_not_asked_for_again_once_something_followed_it() {
    let Stuck { channel, context, agent, mut events, running, .. } =
        with_an_agent(Arc::new(FailsWhere(Mutex::new([None].into()))), Default::default()).await;
    let failed = given_up_turn(&mut events, agent).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message(None, Some("are you there?".to_string()), None, Vec::new()).await.unwrap();
    // The message is in the channel's history once it was routed.
    time::timeout(Duration::from_secs(5), async {
        while channel.whole_history().await.len() < 2 {
            time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .expect("the message is said");

    // The conversation moved on.
    retry_of_turn_rejected(&channel, alice_of(&context), agent, failed).await;
    running.abort();
}

#[tokio::test]
async fn a_turn_that_reaches_the_limit_of_model_calls_is_given_up_and_may_be_answered_anew() {
    // The model calls a tool at every call, as one that does not come to an
    // end; asked anew, it answers.
    let (calculate, components) = calculator();
    let mut answers: VecDeque<Message> = (0..MAX_MODEL_CALLS)
        .map(|_| Message { id: MessageId::new(), ..calculate.clone() })
        .collect();
    answers.push_back(Message {
        role: MessageRole::Assistant,
        content: Some("going on".to_string()),
        ..make_message(MessageId::new())
    });
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted::new(answers)), components).await;

    // The users are told why the turn ended (issue 411).
    let (turn, reason) = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            if let ActorEvent::TurnGivenUp { turn_id, reason } = envelope.event {
                break (turn_id, reason);
            }
        }
    })
    .await
    .expect("the turn is given up");
    assert_eq!(reason, GivenUpReason::CallLimit { calls: MAX_MODEL_CALLS });
    settled(&helper, ActorState::Idle).await;

    // What its tools did stays, its end follows, and its model is told why.
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let turns = given_up_turns(&stored, ThreadId::line(channel.id()));
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].1,
        GivenUpReason::CallLimit { calls: MAX_MODEL_CALLS },
        "kept in the history"
    );
    let results = turns[0].2.iter().filter(|m| m.role == MessageRole::Tool).count();
    assert_eq!(results, MAX_MODEL_CALLS as usize, "a result for every call");
    let told = given_next(&stored, ThreadId::line(channel.id()), agent)
        .last()
        .and_then(|m| m.content.clone())
        .unwrap_or_default();
    assert!(
        told.contains(
            "because it asked the model as often as one answer, or one step of a plan, may"
        ),
        "{told}"
    );

    // It may be asked to answer anew.
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice
        .request_retry_of_turn(ActorId::Agent(agent), turn)
        .await
        .expect("its last turn, given up at the limit");
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)
    ));
    let said: Vec<_> =
        answered(&mut events, agent).await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(said, ["going on"], "answered anew after it");
    running.abort();
}

/// A model that gives the answers it was given, one per call, and then
/// fails.
#[derive(Debug)]
struct FailsAfter(Mutex<VecDeque<Message>>);

#[async_trait::async_trait]
impl LanguageModelClient for FailsAfter {
    async fn chat_completion(
        &self,
        _: ActorId,
        _: &[Message],
        _: &[ToolDescription],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        match self.0.lock().unwrap().pop_front() {
            Some(answer) => Box::pin(stream::iter([Ok(MessageEvent::Complete(answer))])),
            None => Box::pin(stream::iter([Err(ModelError::Backend {
                endpoint: "test".to_string(),
                model: "test".to_string(),
                message: "gone".to_string(),
            })])),
        }
    }
}

impl ModelProfileProvider for FailsAfter {
    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

#[tokio::test]
async fn a_turn_that_fails_after_a_tool_ran_keeps_what_it_said_and_the_agent_is_told() {
    use caelumora_domain::tool::ToolCall;

    // The model calls a tool that needs no approval, and fails when
    // asked about its result.
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "expression": "1+1" }),
            ..make_tool_call("calculate")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: CALCULATOR_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, context: _, agent, helper, mut events, running } =
        with_an_agent(Arc::new(FailsAfter(Mutex::new([calculate.clone()].into()))), components)
            .await;
    let line = ThreadId::line(channel.id());

    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::TurnGivenUp { .. })
            {
                break;
            }
        }
    })
    .await
    .expect("the turn fails");
    settled(&helper, ActorState::Idle).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();

    // The turn's messages stay where they were said, followed by its end
    // (issue 258); what the model saw for them can still be told.
    let seen = channel.agent_context(agent).await;
    let turns = given_up_turns(&stored, ThreadId::line(channel.id()));
    assert_eq!(turns.len(), 1);
    assert!(matches!(turns[0].1, GivenUpReason::Failed { .. }));
    let roles: Vec<_> = turns[0].2.iter().map(|m| m.role).collect();
    assert_eq!(roles, [MessageRole::Assistant, MessageRole::Tool], "the call and its result");
    assert!(seen.context_of(calculate.id).await.is_ok(), "the call's context is resolved");
    assert!(
        stored.transcript.messages(line).iter().any(|m| m.id == calculate.id),
        "in the transcript"
    );
    assert!(
        channel.visible_messages(ActorId::Agent(agent)).await.iter().any(|m| m.id == calculate.id),
        "and in the channel's history, as the agent said it"
    );

    // Its model is given the call in full, and told why the turn ended.
    let given = given_next(&stored, ThreadId::line(channel.id()), agent);
    assert!(
        given.iter().any(|m| m
            .tool_calls
            .iter()
            .any(|c| c.arguments == serde_json::json!({ "expression": "1+1" }))),
        "{given:?}"
    );
    let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
    assert!(told.contains("because it failed"), "{told}");
    running.abort();
}

#[tokio::test]
async fn an_agent_removed_during_a_turn_gives_it_up() {
    use caelumora_domain::tool::ToolCall;

    // The model calls a tool, and never answers again.
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "expression": "1+1" }),
            ..make_tool_call("calculate")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: CALCULATOR_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent(Arc::new(Scripted::new([calculate])), components).await;
    time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            let line = ThreadId::line(channel.id());
            if state.turn(ThreadId::line(channel.id())).is_some_and(|t| {
                state
                    .transcript
                    .messages(line)
                    .iter()
                    .any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty())
            }) {
                break;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the tool ran");

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    let turns = time::timeout(Duration::from_secs(5), async {
        loop {
            let turns = given_up_turns(
                &channel.stored_agent_channel_state(agent).await.unwrap(),
                ThreadId::line(channel.id()),
            );
            if !turns.is_empty() {
                break turns;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the turn is given up");
    assert_eq!(turns[0].1, GivenUpReason::Left);
    running.abort();
}

/// What the model of the agent whose state is `stored` is given next in `thread`:
/// the branch of its transcript as it stands, resolved as a turn's context is.
fn given_next(stored: &AgentChannelState, thread: ThreadId, agent: AgentId) -> Vec<Message> {
    use crate::testing::catalogue;
    use caelumora_domain::context::{ContextItem, ContextRef, ContextResolver};
    let reference =
        ContextRef { transcript: stored.transcript.digest(thread), ..ContextRef::default() };
    ContextResolver::of(stored, thread, &catalogue())
        .resolve(&reference, TurnId::new(), ActorId::Agent(agent))
        .expect("worded")
        .expect("resolved")
        .items
        .into_iter()
        .filter_map(|item| match item {
            ContextItem::Conversation { message } => Some(message),
            ContextItem::Prompt { .. } => None,
        })
        .collect()
}

fn calculator() -> (Message, ComponentConfig) {
    use caelumora_domain::tool::ToolCall;
    let calculate = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "expression": "1+1" }),
            ..make_tool_call("calculate")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "calculator".to_string(),
            kind: CALCULATOR_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    (calculate, components)
}

#[tokio::test]
async fn lowering_the_channel_gives_up_a_running_turn() {
    use caelumora_domain::confidentiality::Confidentiality;

    // The model calls a tool, and then is asked about its result for
    // ever.
    let (calculate, components) = calculator();
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent_under(
        Arc::new(AllowAllPolicy::new()),
        Arc::new(Scripted::new([calculate.clone()])),
        components,
    )
    .await;
    time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            let line = ThreadId::line(channel.id());
            if state.turn(ThreadId::line(channel.id())).is_some_and(|t| {
                state
                    .transcript
                    .messages(line)
                    .iter()
                    .any(|m| m.turn_id == Some(t.work.turn_id) && !m.tool_results.is_empty())
            }) {
                break;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the tool ran");

    let public = Classification::general(Confidentiality::Public);
    channel.reclassify_by_procedure(&context, public.clone()).await.expect("permitted");

    // The model call is interrupted, and the turn given up rather than
    // taken over: the users are told, nothing of it is in the history.
    let told = turn_given_up(&mut events, agent).await;
    assert!(told.contains("lowered"), "{told}");
    settled(&helper, ActorState::Idle).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let turns = given_up_turns(&stored, ThreadId::line(channel.id()));
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].1, GivenUpReason::Lowered { channel: public.clone() });
    // What it said before the lowering was held by the channel: it stays,
    // fitted into the channel as it is now (issue 258).
    let said = channel
        .whole_history()
        .await
        .into_iter()
        .find(|m| m.id == calculate.id)
        .expect("said before the lowering");
    assert_eq!(said.classification, public);

    // The agent keeps what the turn said, but its model is given which
    // tool ran and not what it saw (issues 141, 258).
    assert!(
        stored
            .transcript
            .messages(ThreadId::line(channel.id()))
            .iter()
            .any(|m| m.id == calculate.id),
        "the record keeps it"
    );
    let given = given_next(&stored, ThreadId::line(channel.id()), agent);
    let call = given.iter().find(|m| m.id == calculate.id).expect("the call is given");
    assert_eq!(
        (call.tool_calls[0].function_name.as_str(), &call.tool_calls[0].arguments),
        ("calculate", &Value::Null)
    );
    assert_eq!(call.classification, public);
    let results: Vec<_> = given.iter().flat_map(|m| &m.tool_results).collect();
    assert!(
        !results.is_empty() && results.iter().all(|r| r.content == r.status.as_content(None)),
        "how it ended, nothing of what it returned: {results:?}"
    );
    let told = given.last().and_then(|m| m.content.clone()).unwrap_or_default();
    assert!(told.contains("lowered") && told.contains("withheld"), "{told}");
    running.abort();
}

#[tokio::test]
async fn lowering_the_channel_ends_a_turn_waiting_for_approval_at_once() {
    use caelumora_domain::confidentiality::Confidentiality;

    // Nobody approves the call.
    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent_under(
        Arc::new(AllowAllPolicy::new()),
        Arc::new(Scripted::new([call])),
        components,
    )
    .await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel
        .reclassify_by_procedure(&context, Classification::general(Confidentiality::Public))
        .await
        .expect("permitted");

    turn_given_up(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn raising_the_channel_lets_a_running_turn_go_on() {
    use caelumora_domain::confidentiality::Confidentiality;
    let stuck = with_an_agent_under(
        Arc::new(AllowAllPolicy::new()),
        Arc::new(NeverAnswers),
        Default::default(),
    )
    .await;
    settled(&stuck.helper, ActorState::Planning).await;

    stuck
        .channel
        .reclassify_by_procedure(
            &stuck.context,
            Classification::general(Confidentiality::Confidential),
        )
        .await
        .expect("permitted");

    time::sleep(Duration::from_millis(100)).await;
    assert_eq!(stuck.helper.state(), ActorState::Planning, "the model call goes on");
    stuck.running.abort();
}

#[tokio::test]
async fn a_cancel_ends_a_turn_waiting_for_approval_at_once() {
    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted::new([call])), components).await;
    let line = ThreadId::line(channel.id());
    settled(&helper, ActorState::WaitingForUser).await;

    let ended = channel.cancel(&context, None).await.expect("permitted");
    assert_eq!(ended, [helper.name().to_string()], "it had a turn to end (issue 611)");

    // Without anyone deciding on the call.
    settled(&helper, ActorState::Idle).await;
    let ended = channel.cancel(&context, None).await.expect("permitted");
    assert!(ended.is_empty(), "nothing is left to end: {ended:?}");

    // The request is withdrawn, and the approver tells the users
    // (issue 148).
    let (requested, withdrawn) = time::timeout(Duration::from_secs(5), async {
        let mut requested = None;
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            match envelope.event {
                ActorEvent::ApprovalRequested { request, .. }
                    if matches!(envelope.context.sender_id, ActorId::Approver(_)) =>
                {
                    requested = Some(request.id)
                }
                ActorEvent::ApprovalRequestWithdrawn { id }
                    if matches!(envelope.context.sender_id, ActorId::Approver(_)) =>
                {
                    break (requested, id)
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the users learn that the request was withdrawn");
    assert_eq!(requested, Some(withdrawn));
    running.abort();

    // The call is answered as not executed (issue 148).
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let last = stored.transcript.messages(line).pop().expect("the turn was taken over");
    let answered: Vec<_> = last.tool_results.iter().map(|r| r.status).collect();
    assert_eq!(answered, [ToolResultStatus::NotExecuted]);
}

/// Issue 602: a turn in a thread that waits for an approval is set aside: the agent answers in
/// the line meanwhile, telling that it waits, and the approval given then resumes the thread's
/// turn, which goes on where it waited.
#[tokio::test]
async fn a_turn_waiting_for_a_person_is_set_aside_while_the_agent_answers_elsewhere() {
    let (call, components) = a_call_needing_approval();
    let answers = [call, saying("In the line."), saying("Done.")];
    let model = Arc::new(Scripted::new(answers));
    let (channel, context, agent, alice) =
        agent_channel(Arc::new(AllowAllPolicy::new()), model, components).await;
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let helper = channel.actor_of(ActorId::Agent(agent)).unwrap();
    let line = ThreadId::line(channel.id());
    let origin = ThreadOrigin { thread: line, message: MessageId::new() };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");
    let hers = channel.user_actor(alice).unwrap();
    let say = |thread, text: &str| {
        let hers = Arc::clone(&hers);
        let text = text.to_string();
        async move { hers.send_message(Some(thread), Some(text), None, Vec::new()).await.unwrap() }
    };

    say(thread, "@helper, sleep").await;
    let asked = approval_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;

    say(line, "@helper, are you there?").await;
    let in_line = answered_in(&mut events, agent).await;
    assert_eq!(in_line.0, line, "it answers in the line meanwhile");
    assert_eq!(texts(&in_line.1), ["In the line."]);
    settled(&helper, ActorState::WaitingForUser).await;

    hers.send_approval(asked.id, true).await.unwrap();
    let resumed = answered_in(&mut events, agent).await;
    assert_eq!(resumed.0, thread, "the approval resumes the thread's turn");
    assert_eq!(texts(&resumed.1), ["Done."], "which goes on where it waited");
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

/// Issue 602: a turn set aside is ended by a cancel of its thread, not by one of another; what was
/// said in its thread while it waited the agent hears only once it ended - after the results of
/// the turn's calls, never between them and the calls -, and then answers what asked it.
#[tokio::test]
async fn a_turn_set_aside_ends_with_its_thread_and_what_was_said_there_waits_for_it() {
    let (call, components) = a_call_needing_approval();
    let model = Arc::new(Scripted::new([call]));
    let (channel, context, agent, alice) =
        agent_channel(Arc::new(AllowAllPolicy::new()), model, components).await;
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let helper = channel.actor_of(ActorId::Agent(agent)).unwrap();
    let line = ThreadId::line(channel.id());
    let origin = ThreadOrigin { thread: line, message: MessageId::new() };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");
    let hers = channel.user_actor(alice).unwrap();
    let say = |text: &str| hers.send_message(Some(thread), Some(text.to_string()), None, vec![]);
    say("@helper, sleep").await.unwrap();
    approval_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;
    say("meanwhile").await.unwrap();

    let ended = channel.cancel(&context, Some(line)).await.expect("permitted");
    assert!(ended.is_empty(), "nothing of it in the line (issue 611): {ended:?}");
    time::sleep(Duration::from_millis(50)).await;
    assert_eq!(helper.state(), ActorState::WaitingForUser, "a cancel of the line ends it not");
    let ended = channel.cancel(&context, Some(thread)).await.expect("permitted");
    assert_eq!(ended, [helper.name().to_string()], "the turn set aside there (issue 611)");
    let ended = answered_in(&mut events, agent).await;
    assert_eq!(ended.0, thread, "a cancel of its thread ends it");
    // Asked before the cancel, it answers what was said meanwhile, its model then silent.
    settled(&helper, ActorState::Planning).await;
    running.abort();

    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let messages = stored.transcript.messages(thread);
    let results = messages.iter().position(|m| !m.tool_results.is_empty());
    let meanwhile = messages.iter().position(|m| m.content.as_deref() == Some("meanwhile"));
    assert!(results.is_some() && meanwhile > results, "heard after the results: {messages:?}");
    let calls = messages.iter().position(|m| !m.tool_calls.is_empty());
    assert_eq!(calls.map(|at| at + 1), results, "the results right after the calls");
}

/// Issue 602: a turn set aside is asked for again from where it waits: it is given up, and the
/// request answers it anew in its thread; leaving the channel then gives up the turn set aside
/// there too.
#[tokio::test]
async fn a_turn_set_aside_is_answered_again_and_given_up_as_its_agent_leaves() {
    let (call, components) = a_call_needing_approval();
    let (again, _) = a_call_needing_approval();
    let model = Arc::new(Scripted::new([call, again]));
    let (channel, context, agent, alice) =
        agent_channel(Arc::new(AllowAllPolicy::new()), model, components).await;
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let helper_id = ActorId::Agent(agent);
    let helper = channel.actor_of(helper_id).unwrap();
    let line = ThreadId::line(channel.id());
    let origin = ThreadOrigin { thread: line, message: MessageId::new() };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");
    let hers = channel.user_actor(alice).unwrap();
    hers.send_message(Some(thread), Some("@helper, sleep".to_string()), None, vec![])
        .await
        .unwrap();
    approval_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;

    let waiting = channel.turn_in(helper_id, thread).expect("a turn set aside in the thread");
    assert_eq!(channel.turn_of(helper_id), None, "none in the line");
    hers.request_retry_of_turn(helper_id, waiting).await.expect("on its way");
    let given_up = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if let ActorEvent::TurnGivenUp { turn_id, reason } = envelope.event {
                break (turn_id, reason, envelope.context.thread_id);
            }
        }
    })
    .await
    .expect("given up");
    assert_eq!(given_up, (waiting, GivenUpReason::Retried, thread), "from where it waits");
    let asked_again = approval_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;
    let anew = channel.turn_in(helper_id, thread).expect("answered anew in its thread");
    assert_ne!(anew, waiting, "by a turn of its own");

    channel.remove_participant(&context, helper_id).await.expect("permitted");
    let left = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            match envelope.event {
                ActorEvent::TurnGivenUp { turn_id, reason } => break (turn_id, reason),
                ActorEvent::ApprovalRequestWithdrawn { id } => assert_eq!(id, asked_again.id),
                _ => {}
            }
        }
    })
    .await
    .expect("given up as it leaves");
    assert_eq!(left, (anew, GivenUpReason::Left), "the turn set aside, as its agent leaves");
    running.abort();
}

/// The thread and the messages of the next turn `agent` finished, from `events`.
async fn answered_in(
    events: &mut broadcast::Receiver<Envelope>,
    agent: AgentId,
) -> (ThreadId, Vec<Message>) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                    break (envelope.context.thread_id, messages);
                }
            }
        }
    })
    .await
    .expect("the agent answers")
}

/// What `messages` say.
fn texts(messages: &[Message]) -> Vec<String> {
    messages.iter().filter_map(|message| message.content.clone()).collect()
}

/// A call of `sleep` that needs approval, and the component that gives
/// it, for an agent to wait for its approval.
fn a_call_needing_approval() -> (Message, ComponentConfig) {
    use caelumora_domain::tool::ToolCall;

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({ "seconds": 0 }),
            ..make_tool_call("sleep")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: SLEEP_KIND.to_string(),
            params: serde_json::json!({ "requires_approval": true }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    (call, components)
}

#[tokio::test]
async fn a_rule_of_the_channel_asks_for_a_call_its_tool_would_run() {
    use caelumora_domain::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};

    let (call, mut components) = a_call_needing_approval();
    components.component_nodes[0].params = serde_json::json!({ "requires_approval": false });
    let built =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(Scripted::new([call])), components)
            .await;
    let ask = ApprovalRule::new(
        ToolMatcher { tool: "sleep".to_string(), arguments: Default::default() },
        RuleDecision::Ask,
        Reach::Everywhere,
        None,
    )
    .unwrap();
    built
        .0
        .controller()
        .update_approval_rules(&SecurityContext::system(), vec![ask])
        .await
        .expect("the system may");

    let Stuck { helper, mut events, running, .. } = start(built).await;
    let request = approval_requested(&mut events).await;
    assert!(request.explicit, "for a decision of a person's own");
    settled(&helper, ActorState::WaitingForUser).await;
    running.abort();
}

/// A running session of alice, bob and an agent whose one call waits for approval, in a channel
/// of theirs with an approver deciding by `strategy`, which bob takes part in if `with_bob`: the
/// agent asked as alice said hello, and the approval waits for alice (issue 603).
struct Approving {
    session: Arc<SessionLoop>,
    channel: Arc<ChannelLoop>,
    context: SecurityContext,
    alice: UserId,
    bob: UserId,
    /// The approval asked of the users.
    request: ApprovalId,
}

async fn approving(strategy: ApprovalStrategy, with_bob: bool) -> Approving {
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::template::InternalActorTemplate;
    use caelumora_domain::user::testing::make_user;
    use caelumora_kernel::session::testing::{create, spec};
    use caelumora_kernel::session::ChannelSpec;
    use caelumora_store::testing::record_users;
    let (call, components) = a_call_needing_approval();
    let store = Persistence::in_memory();
    let personality = Personality { components, ..make_personality("plain") };
    let (context, agent, alice) = alice_and_an_agent(&store, "helper", personality).await;
    let model = Arc::new(Scripted::new([call]));
    let kernel = agent_kernel(store, Arc::new(AllowAllPolicy::new()), model);
    let bob = make_user("bob");
    let mut session = make_session("approving", alice);
    session.participants = record_users(kernel.entity_stores(), &[&bob]).await;
    session.participants.extend([Participant::User(alice), Participant::Agent(agent)]);
    let session = create(&kernel, &context, session).await.expect("the session starts");
    let mut taking_part = vec![ActorId::User(alice), ActorId::Agent(agent)];
    if with_bob {
        taking_part.push(ActorId::User(bob.id));
    }
    let spec = ChannelSpec {
        internal_actors: vec![InternalActorTemplate::Approver { strategy }],
        ..spec("approving", taking_part)
    };
    let channel = session.create_channel(&context, spec).await.expect("alice makes it");
    let hers = channel.user_actor(alice).expect("alice takes part");
    let mut shown = hers.watch();
    hers.send_message(None, Some("hello".to_string()), None, Vec::new()).await.unwrap();
    let request = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = shown.recv().await.expect("followed");
            if let ActorEvent::ApprovalRequested { request, .. } = envelope.event {
                break request.id;
            }
        }
    })
    .await
    .expect("the call waits for alice");
    assert!(waiting(&hers, 1).await, "and she is told");
    Approving { session, channel, context, alice, bob: bob.id, request }
}

/// Whether `approvals` approvals wait for the user of `actor` within five seconds.
async fn waiting(actor: &UserActor, approvals: usize) -> bool {
    time::timeout(Duration::from_secs(5), async {
        while actor.waiting().approvals != approvals {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok()
}

/// Issue 603: a user who joins a channel while an approval waits there is asked it - they alone -,
/// so that what waits for them tells it, as it tells the user who was there.
#[tokio::test]
async fn a_user_who_joins_while_an_approval_waits_is_asked_it() {
    use caelumora_domain::channel::HistoryAccess;
    let Approving { session, channel, context, alice, bob, .. } =
        approving(ApprovalStrategy::Any, false).await;
    let into = Some((channel.id(), HistoryAccess::All));
    session.add_participant(&context, ActorId::User(bob), into).await.expect("added");
    let his = channel.user_actor(bob).expect("bob takes part");
    assert!(waiting(&his, 1).await, "and for bob, who joined while it waited");
    let hers = channel.user_actor(alice).expect("alice takes part");
    assert_eq!(hers.waiting().approvals, 1, "still waiting for alice");
}

/// Issue 603: as a user leaves the channel, the approver, told so by the channel, waits for them
/// no more - an approval all the others gave is decided.
#[tokio::test]
async fn an_approval_waits_no_more_for_a_user_who_left() {
    let Approving { session, channel, context, alice, bob, request: id } =
        approving(ApprovalStrategy::All, true).await;
    let hers = channel.user_actor(alice).expect("alice takes part");
    let his = channel.user_actor(bob).expect("bob takes part");
    assert!(waiting(&his, 1).await, "it waits for bob too");
    hers.send_approval(id, true).await.unwrap();
    assert!(waiting(&hers, 0).await, "answered by alice");

    let mut shown = hers.watch();
    session.remove_participant(&context, ActorId::User(bob)).await.expect("bob is let go");
    let decided = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = shown.recv().await.expect("followed");
            if let ActorEvent::ApprovalGranted(verdict) = envelope.event {
                break verdict.id;
            }
        }
    })
    .await
    .expect("decided by alice alone, bob gone");
    assert_eq!(decided, id);
}

/// The approval the approver asks the users for, from `events`.
async fn approval_requested(events: &mut broadcast::Receiver<Envelope>) -> ApprovalRequest {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if let ActorEvent::ApprovalRequested { request, .. } = envelope.event {
                if matches!(envelope.context.sender_id, ActorId::Approver(_)) {
                    break request;
                }
            }
        }
    })
    .await
    .expect("the users are asked")
}

#[tokio::test]
async fn a_turn_waiting_for_approval_can_be_decided_after_a_restart() {
    let (call, components) = a_call_needing_approval();
    let done = Message {
        role: MessageRole::Assistant,
        content: Some("Done.".to_string()),
        ..make_message(MessageId::new())
    };
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(Arc::new(Scripted::new([call, done])), components).await;
    settled(&helper, ActorState::WaitingForUser).await;
    let asked = approval_requested(&mut events).await;

    // The turn is the record of what it waits for (issue 172).
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    let pending: Vec<_> = stored
        .turn(ThreadId::line(channel.id()))
        .expect("a turn under way")
        .pending_approvals()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(pending, [asked.id]);

    // A restart: the channel and its actors are gone, the approver's
    // memory with them; the channel is built again from the stores.
    running.abort();
    let _ = running.await;
    let document = Channel::clone(&channel.channel());
    let restarted = ChannelLoop::new(channel.restarted_runtime()).await.unwrap();
    let mut events = restarted.observe();
    let running = tokio::spawn({
        let restarted = Arc::clone(&restarted);
        async move { restarted.run().await }
    });
    let helper = restarted.actor_of(ActorId::Agent(agent)).unwrap();

    // The agent asks again, with the same id, and the users can decide.
    let asked_again = approval_requested(&mut events).await;
    assert_eq!(asked_again.id, asked.id);
    let alice = document
        .participants
        .iter()
        .find_map(|p| match p {
            Participant::User(id) => Some(*id),
            _ => None,
        })
        .expect("alice");
    restarted.user_actor(alice).unwrap().send_approval(asked.id, true).await.unwrap();
    let said: Vec<_> =
        answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
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
    use caelumora_domain::tool::ToolCall;

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: calls
            .iter()
            .map(|id| ToolCall {
                id: Some(id.to_string()),
                arguments: serde_json::json!({}),
                ..make_tool_call("sleep")
            })
            .collect(),
        content: None,
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: SLEEP_KIND.to_string(),
            params: serde_json::json!({ "seconds": 60 }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let unasked = Message {
        role: MessageRole::Assistant,
        content: Some("never given".to_string()),
        ..make_message(MessageId::new())
    };
    let model = Arc::new(Scripted::new([call, unasked]));
    let stuck = with_an_agent(Arc::<Scripted>::clone(&model), components).await;
    settled(&stuck.helper, ActorState::ExecutingTools).await;
    (stuck, model)
}

#[tokio::test]
async fn a_turn_asked_for_again_while_it_waits_for_an_approval_withdraws_the_request() {
    let (call, components) = a_call_needing_approval();
    let instead = Message {
        role: MessageRole::Assistant,
        content: Some("no wait needed".to_string()),
        ..make_message(MessageId::new())
    };
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted::new([call, instead])), components).await;
    let ActorEvent::ApprovalRequested { request, .. } =
        shown_by(&mut events, agent, "ApprovalRequested").await
    else {
        unreachable!()
    };
    settled(&helper, ActorState::WaitingForUser).await;

    let turn = channel.turn_of(ActorId::Agent(agent)).expect("waiting is under way");
    channel
        .user_actor(alice_of(&context))
        .unwrap()
        .request_retry_of_turn(ActorId::Agent(agent), turn)
        .await
        .expect("asked");
    assert!(
        matches!(
            shown_by(&mut events, agent, "ApprovalRequestWithdrawn").await,
            ActorEvent::ApprovalRequestWithdrawn { id } if id == request.id
        ),
        "nobody is asked any longer"
    );
    assert!(matches!(
        shown_by(&mut events, agent, "RetryAccepted").await,
        ActorEvent::RetryAccepted { target } if target == RetryTarget::Turn(turn)
    ));
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(said(answered(&mut events, agent).await), ["no wait needed"]);
    running.abort();
}

#[tokio::test]
async fn a_turn_asked_for_again_while_a_tool_runs_tells_the_next_one_of_the_call() {
    let (Stuck { channel, context, agent, mut events, running, .. }, _) =
        with_an_agent_in_a_long_tool().await;
    let turn = channel.turn_of(ActorId::Agent(agent)).expect("under way");
    channel
        .user_actor(alice_of(&context))
        .unwrap()
        .request_retry_of_turn(ActorId::Agent(agent), turn)
        .await
        .expect("asked");

    assert_eq!(
        turn_given_up(&mut events, agent).await,
        "your answer was asked for again",
        "the users learn why the turn ended (issue 258)"
    );
    let said = |messages: Vec<Message>| {
        messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        said(answered(&mut events, agent).await),
        ["never given"],
        "the tool is interrupted, and the next turn answers"
    );
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let given_up = given_up_turns(&state, ThreadId::line(channel.id()));
    assert_eq!(given_up.len(), 1, "the turn is kept as given up");
    assert_eq!(given_up[0].1, GivenUpReason::Retried);
    assert_eq!(given_up[0].0, turn);
    assert!(
        given_up[0].2.iter().any(|m| m.tool_calls.iter().any(|c| c.function_name == "sleep")),
        "the call that ran stays"
    );
    let told: Vec<_> = given_next(&state, ThreadId::line(channel.id()), agent)
        .into_iter()
        .filter_map(|m| m.content)
        .filter(|c| c.contains("given up"))
        .collect();
    assert!(
        told.iter().any(|c| c.contains("asked for again")),
        "the model is told why the turn was given up: {told:?}"
    );
    let history: Vec<_> =
        channel.whole_history().await.into_iter().filter_map(|m| m.content).collect();
    assert_eq!(
        history,
        ["hello", "never given"],
        "the channel's history takes a turn's answer alone"
    );
    running.abort();
}

#[tokio::test]
async fn a_cancel_while_a_tool_runs_interrupts_the_tool_and_ends_the_turn() {
    let (Stuck { channel, context, agent, helper, mut events, running }, model) =
        with_an_agent_in_a_long_tool().await;

    channel.cancel(&context, None).await.expect("permitted");

    // The turn ends at once, not when the tool would have (issue 161),
    // and the model is not asked again.
    let ended = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                    break messages;
                }
            }
        }
    })
    .await
    .expect("the cancelled turn ends without waiting for the tool");
    let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.len(), 1, "{ended:?}");
    assert_eq!(results[0].status, ToolResultStatus::Interrupted);
    assert!(results[0].started_at.is_some(), "it ran");
    settled(&helper, ActorState::Idle).await;
    assert_eq!(model.left(), 1, "the second answer was never asked for");
    running.abort();
}

#[tokio::test]
async fn a_tool_call_interrupted_alone_is_answered_so_and_the_turn_goes_on() {
    let (Stuck { channel, context, agent, mut events, running, .. }, model) =
        with_an_agent_in_long_tools(&["1", "2"]).await;
    let turn = channel.turn_of(ActorId::Agent(agent)).expect("under way");
    let not_running = |refused: Result<()>| {
        matches!(
            refused.expect_err("refused").downcast_ref::<ChannelError>(),
            Some(ChannelError::ToolCallNotRunning { .. })
        )
    };
    assert!(
        not_running(
            channel.interrupt_tool_call(&context, ActorId::Agent(agent), TurnId::new(), "1").await
        ),
        "not of another turn"
    );
    assert!(
        not_running(channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "3").await),
        "no such call"
    );

    // The second call, still to run, first; then the one that runs.
    channel
        .interrupt_tool_call(&context, ActorId::Agent(agent), turn, "2")
        .await
        .expect("it waits to run");
    channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "1").await.expect("it runs");
    assert!(
        not_running(channel.interrupt_tool_call(&context, ActorId::Agent(agent), turn, "1").await),
        "once"
    );

    let finished = answered(&mut events, agent).await;
    let results: Vec<_> = finished.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(
        results.iter().map(|r| r.status).collect::<Vec<_>>(),
        [ToolResultStatus::Interrupted, ToolResultStatus::NotExecuted],
        "{finished:?}"
    );
    assert!(
        results[0].started_at.is_some() && results[1].started_at.is_none(),
        "the first ran, the second never did"
    );
    let alice = channel.actor_of(context.initiator_id).expect("she takes part").name().to_string();
    assert!(
        results.iter().all(|r| r.content.contains(&format!("{alice} interrupted this tool call"))),
        "the model is told who: {results:?}"
    );
    let said: Vec<_> = finished.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["never given"], "the turn goes on with the results");
    assert_eq!(model.left(), 0, "the model was asked again");
    running.abort();
}

#[tokio::test]
async fn an_agent_removed_while_a_tool_runs_gives_up_its_turn_and_keeps_the_call() {
    let (Stuck { channel, context, agent, running, .. }, _) = with_an_agent_in_a_long_tool().await;
    let line = ThreadId::line(channel.id());

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // Within the grace of a stop (issue 161): the turn is given up rather
    // than the actor aborted with the turn left as it was before the tool.
    let stored = time::timeout(STOP_GRACE, async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turns().next().is_none()
                && !given_up_turns(&state, ThreadId::line(channel.id())).is_empty()
            {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the turn is given up before the grace runs out");
    assert_eq!(given_up_turns(&stored, ThreadId::line(channel.id()))[0].1, GivenUpReason::Left);
    let interrupted = stored
        .transcript
        .messages(line)
        .iter()
        .flat_map(|m| m.tool_results.clone())
        .find(|r| r.tool_name == "sleep")
        .map(|r| r.content)
        .unwrap_or_default();
    assert!(
        interrupted.contains("interrupted"),
        "the call that ran stays, with how it ended: {interrupted}"
    );

    // The agent tells the channel of its turn's end as it leaves, and
    // the channel records it after the call it said (issues 258, 262), and
    // that it left after that (issue 408).
    let entries = time::timeout(Duration::from_secs(5), async {
        loop {
            let entries = channel.history_entries().await;
            if matches!(entries.last(), Some(HistoryEntry::ParticipantLeft { .. })) {
                break entries;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the channel records that it left");
    let ended = match &entries[entries.len() - 2] {
        HistoryEntry::TurnGivenUp { actor, reason, .. } => Some((*actor, reason.clone())),
        _ => None,
    };
    assert_eq!(
        ended,
        Some((ActorId::Agent(agent), GivenUpReason::Left)),
        "the turn's end comes before: {entries:?}"
    );
    assert!(
        matches!(
            entries.last(),
            Some(HistoryEntry::ParticipantLeft { participant, by, .. })
                if *participant == ActorId::Agent(agent) && *by == context.caller_id
        ),
        "in the name of who removed it"
    );
    assert!(
        channel
            .whole_history()
            .await
            .iter()
            .any(|m| m.tool_calls.iter().any(|c| c.function_name == "sleep")),
        "what it said stays"
    );
    running.abort();
}

#[tokio::test]
async fn an_agent_stopped_in_a_model_call_keeps_its_turn_and_asks_again_when_its_channel_runs_again(
) {
    let (Stuck { channel, agent, running, .. }, model) =
        asking_a_gated_model(&["never given", "Answered."]).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        stored.turn(ThreadId::line(channel.id())).map(|turn| turn.step),
        Some(ActorState::Planning),
        "stored at the step it stood at"
    );
    assert!(stored.turn(ThreadId::line(channel.id())).is_some(), "the turn is kept");
    assert!(given_up_turns(&stored, ThreadId::line(channel.id())).is_empty(), "not given up");

    let (_restarted, mut events, running) = run_again(&channel).await;
    asked(&model, 2).await;
    model.gate.add_permits(1);
    let said: Vec<_> =
        answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Answered."], "the turn resumed with the call it stood at");
    running.abort();
}

/// The call under way is interrupted and recorded so; the one still to
/// run is not run when the turn resumes, but told the model as not
/// executed, as after a restart (issue 75) - and the turn goes on.
#[tokio::test]
async fn an_agent_stopped_while_a_tool_runs_records_the_call_as_stopped_and_goes_on_when_its_channel_runs_again(
) {
    let (Stuck { channel, agent, running, .. }, model) =
        with_an_agent_in_long_tools(&["1", "2"]).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        stored.turn(ThreadId::line(channel.id())).map(|turn| turn.step),
        Some(ActorState::ExecutingTools),
        "the second call has no result yet"
    );
    assert!(given_up_turns(&stored, ThreadId::line(channel.id())).is_empty(), "not given up");

    let (_restarted, mut events, running) = run_again(&channel).await;
    let finished = answered(&mut events, agent).await;
    let results: Vec<_> = finished.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(
        results.iter().map(|r| r.status).collect::<Vec<_>>(),
        [ToolResultStatus::Interrupted, ToolResultStatus::NotExecuted],
        "{finished:?}"
    );
    assert!(
        results[0].content.contains("because the session was stopped"),
        "the model is told why: {}",
        results[0].content
    );
    assert!(
        results[0].started_at.is_some() && results[1].started_at.is_none(),
        "the first ran, the second never did"
    );
    let said: Vec<_> = finished.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["never given"], "the turn goes on with the results");
    assert_eq!(model.left(), 0, "the model was asked again");
    running.abort();
}

#[tokio::test]
async fn an_idle_agent_stopped_ends_at_once_where_it_stands() {
    let model = Arc::new(Scripted::new([saying("Hello.")]));
    let Stuck { channel, agent, helper, mut events, running, .. } =
        with_an_agent(model, Default::default()).await;
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(!stored.asleep, "idle");
    assert!(
        stored.turns().next().is_none()
            && given_up_turns(&stored, ThreadId::line(channel.id())).is_empty(),
        "nothing under way, nothing given up"
    );
}

#[tokio::test]
async fn an_agent_whose_channel_is_deleted_while_a_tool_runs_gives_its_turn_up() {
    let (Stuck { channel, agent, running, .. }, _) = with_an_agent_in_a_long_tool().await;

    let leaving = Instant::now();
    channel.leave_all_actors().await;
    assert!(leaving.elapsed() < STOP_GRACE, "the actors ended by themselves");
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(stored.turns().next().is_none(), "the turn is not kept");
    assert_eq!(
        given_up_turns(&stored, ThreadId::line(channel.id()))
            .iter()
            .map(|(_, reason, _)| reason.clone())
            .collect::<Vec<_>>(),
        [GivenUpReason::Left]
    );
    // Then its task is aborted, as deleting the channel does (issue 593).
    running.abort();
}

#[tokio::test]
async fn an_agent_removed_while_waiting_for_approval_gives_up_its_turn() {
    let (call, components) = a_call_needing_approval();
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted::new([call])), components).await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // It stops waiting and gives the turn up, instead of being aborted
    // with the turn left in its state.
    let stored = time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turns().next().is_none()
                && !given_up_turns(&state, ThreadId::line(channel.id())).is_empty()
            {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the turn is given up");
    assert_eq!(given_up_turns(&stored, ThreadId::line(channel.id()))[0].1, GivenUpReason::Left);
    // The request it waited for is withdrawn, as for any turn given up
    // (issue 262).
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::ApprovalRequestWithdrawn { .. })
            {
                break;
            }
        }
    })
    .await
    .expect("the approval request is withdrawn");
    running.abort();
}

#[tokio::test]
async fn a_call_with_invalid_arguments_is_answered_without_asking_anyone() {
    use caelumora_domain::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};
    use caelumora_domain::tool::ToolCall;

    // A rule asks for every `create_plan` (issue 13); this call's
    // arguments are no plan.
    let not_a_plan = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            arguments: serde_json::json!({}),
            ..make_tool_call("create_plan")
        }],
        content: None,
        ..make_message(MessageId::new())
    };
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("Let me answer directly.".to_string()),
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: PLANNING_KIND.to_string(),
            params: Value::Null,
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    let built = agent_channel(
        Arc::new(AllowAllPolicy::new()),
        Arc::new(Scripted::new([not_a_plan, answer])),
        components,
    )
    .await;
    let ask = ApprovalRule::new(
        ToolMatcher { tool: "create_plan".to_string(), arguments: Default::default() },
        RuleDecision::Ask,
        Reach::Everywhere,
        None,
    )
    .unwrap();
    built
        .0
        .controller()
        .update_approval_rules(&SecurityContext::system(), vec![ask])
        .await
        .expect("the system may");
    let Stuck { agent, helper, mut events, running, .. } = start(built).await;

    // The turn ends without anyone approving anything: the call was
    // answered as invalid, and the model tried again.
    let ended = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                    break messages;
                }
            }
        }
    })
    .await
    .expect("the turn ends without an approval");
    let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
    assert_eq!(results.len(), 1, "{ended:?}");
    assert_eq!(
        results[0].status,
        ToolResultStatus::InvalidArguments,
        "the model is asked to correct the call"
    );
    assert_eq!(ended.last().and_then(|m| m.content.as_deref()), Some("Let me answer directly."));
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn a_cancel_interrupts_a_model_call_that_is_in_flight() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        stuck_in_a_model_call().await;
    let agent_id = ActorId::Agent(agent);

    channel.cancel(&context, None).await.expect("permitted");

    // The turn ends at once - with an empty answer, so that observers
    // stop waiting - instead of when the model would have answered.
    let ended = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == agent_id
                && matches!(envelope.event, ActorEvent::TurnFinished { .. })
            {
                break envelope;
            }
        }
    })
    .await
    .expect("the cancelled turn ends without the model");
    assert!(matches!(
        ended.event,
        ActorEvent::TurnFinished { ref messages, .. } if messages.is_empty()
    ));
    settled(&helper, ActorState::Idle).await;
    running.abort();
}

#[tokio::test]
async fn a_user_learns_that_a_turn_cancelled_before_it_said_anything_ended() {
    let Stuck { channel, context, agent, running, .. } = stuck_in_a_model_call().await;
    let mut followed = channel.user_actor(alice_of(&context)).unwrap().watch();

    channel.cancel(&context, None).await.expect("permitted");

    // Nothing to hear of it - but the end, for whoever shows the answer
    // as under way (issue 224).
    let ended = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = followed.recv().await.expect("the user's actor runs");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                    break messages;
                }
            }
        }
    })
    .await
    .expect("the user learns that the answer ended");
    assert!(ended.is_empty(), "with nothing said: {ended:?}");
    running.abort();
}

#[tokio::test]
async fn a_removed_agent_gives_its_turn_up_says_so_and_leaves_an_idle_state_behind() {
    let Stuck { channel, context, agent, helper, mut events, running } =
        stuck_in_a_model_call().await;
    let line = ThreadId::line(channel.id());

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

    // The turn ends without an answer, and the agent says so itself
    // (issue 262); the state in the store is the one a later actor of
    // the agent starts from: idle, with no turn to resume.
    settled(&helper, ActorState::Idle).await;
    let stored = time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if state.turn(ThreadId::line(channel.id())).is_none() && !state.asleep {
                break state;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the idle state is stored");
    assert!(stored.turns().next().is_none(), "the turn was abandoned, not stored");
    assert!(
        stored.transcript.messages(line).iter().all(|m| m.sender_id != ActorId::Agent(agent)),
        "nothing of it was recorded"
    );
    let farewell = time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::TurnGivenUp { reason, .. } => break reason,
                ActorEvent::TurnFinished { .. } => panic!("a stopped agent announces no answer"),
                _ => {}
            }
        }
    })
    .await
    .expect("the agent tells the channel it gave its turn up");
    assert_eq!(farewell, GivenUpReason::Left);
    // The channel is still running: alice is still in it.
    assert!(!running.is_finished());
    running.abort();
}

#[tokio::test]
async fn an_agent_that_starts_hears_another_agents_answer_as_its_words_without_the_tool_traffic() {
    use caelumora_domain::message::MessageId;
    use caelumora_domain::tool::{ToolCall, ToolCallResult};
    let (channel, _, agent, _) =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), Default::default())
            .await;
    let line = ThreadId::line(channel.id());
    let other = ActorId::Agent(AgentId::new());
    let call = ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({}),
        ..make_tool_call("noop")
    };
    let result = ToolCallResult::of(
        &call,
        Default::default(),
        "done".to_string(),
        Classification::CHANNEL_DEFAULT,
    );
    // Another agent's answer, as the channel's history keeps it.
    let answer = Message {
        sender_id: other,
        sender_name: "hermes".to_string(),
        role: MessageRole::Assistant,
        content: Some("Let me check.".to_string()),
        tool_calls: vec![call],
        ..make_message(MessageId::new())
    };
    let tool = Message {
        sender_id: other,
        role: MessageRole::Tool,
        content: Some("done".to_string()),
        tool_results: vec![result],
        ..make_message(MessageId::new())
    };

    channel.record_said(vec![answer.clone(), tool.clone()]).await;
    let state = channel.run_until(agent, |state| state.last_message == Some(answer.id)).await;

    let heard = state.transcript.messages(line);
    assert_eq!(heard.len(), 1, "nothing of the tool message: {heard:?}");
    assert_eq!(
        (heard[0].id, heard[0].role, heard[0].content.as_deref()),
        (answer.id, MessageRole::User, Some("Let me check.")),
        "the other's words, as another's"
    );
    assert!(heard[0].tool_calls.is_empty(), "not the other's call, as if its own");
    let kept: Vec<_> = channel
        .visible_messages(ActorId::Agent(agent))
        .await
        .iter()
        .map(|m| (m.id, m.role))
        .collect();
    assert_eq!(kept, [(answer.id, MessageRole::Assistant)], "the channel's history gives it what it heard as it was said; the tool traffic, which it would not have heard live, is not its (issues 258, 417)");
}

/// Issue 587: a person's call - a command they ran in the workspace - is what they did in the
/// channel, and heard with its result, though it says nothing in words.
#[tokio::test]
async fn an_agent_hears_a_person_s_call_with_its_result() {
    use caelumora_domain::message::MessageId;
    use caelumora_domain::tool::{ToolCall, ToolCallResult};
    let (channel, context, agent, _) =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), Default::default())
            .await;
    let line = ThreadId::line(channel.id());
    let call = ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({ "command": "ls" }),
        ..make_tool_call("run_command")
    };
    let result = ToolCallResult::of(
        &call,
        Default::default(),
        r#"{"output":"notes.md\n"}"#.to_string(),
        Classification::CHANNEL_DEFAULT,
    );
    let made = Message {
        sender_id: context.initiator_id,
        tool_calls: vec![call],
        tool_results: vec![result],
        content: None,
        ..make_message(MessageId::new())
    };

    channel.record_said(vec![made.clone()]).await;
    let state = channel.run_until(agent, |state| state.last_message == Some(made.id)).await;

    let heard = state.transcript.messages(line);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(
        (heard[0].id, heard[0].tool_calls.len(), heard[0].tool_results.len()),
        (made.id, 1, 1),
        "the call with its result, as the person's"
    );
}

#[tokio::test]
async fn an_agent_that_starts_takes_in_what_it_missed_as_what_it_hears() {
    use caelumora_domain::message::MessageId;
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor::new("filter", "test_no_secrets")],
    };
    let (channel, context, agent, _) =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), components).await;
    let line = ThreadId::line(channel.id());
    let said = |text: &str| Message {
        sender_id: context.initiator_id,
        content: Some(text.to_string()),
        ..make_message(MessageId::new())
    };
    let secret = said("the secret is 42");

    channel.record_said(vec![said("hello"), secret.clone()]).await;
    let state = channel.run_until(agent, |state| state.last_message == Some(secret.id)).await;

    let contents =
        |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
    assert_eq!(
        contents(channel.visible_messages(ActorId::Agent(agent)).await),
        ["hello", "the secret is 42"],
        "the channel's history holds all it missed, for it"
    );
    assert_eq!(
        contents(state.transcript.messages(line)),
        ["hello"],
        "its transcript what passed its processors"
    );
}

#[tokio::test]
async fn all_an_agent_was_offered_is_in_its_channels_history_and_what_passes_its_processors_in_its_transcript(
) {
    let hi = Message {
        role: MessageRole::Assistant,
        content: Some("Hi.".to_string()),
        ..make_message(MessageId::new())
    };
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor::new("filter", "test_no_secrets")],
    };
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted::new([hi])), components).await;
    let line = ThreadId::line(channel.id());
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;

    // Heard between turns; the turn it starts waits for a model that
    // does not answer any more.
    channel
        .user_actor(alice_of(&context))
        .unwrap()
        .send_message(None, Some("the secret is 42".to_string()), None, Vec::new())
        .await
        .unwrap();
    settled(&helper, ActorState::Planning).await;

    let contents =
        |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(
        contents(channel.visible_messages(ActorId::Agent(agent)).await)
            .contains(&"the secret is 42".to_string()),
        "the channel's history holds all it was offered, for it"
    );
    assert!(
        !contents(state.transcript.messages(line)).contains(&"the secret is 42".to_string()),
        "its transcript only what passed"
    );
    assert!(contents(state.transcript.messages(line)).contains(&"hello".to_string()));
    running.abort();
}

/// A channel whose agent, with the components `listed` besides the sleep
/// tool, answered alice's "hello" with "Hi." and then heard "take a nap",
/// which it answers by calling the sleep tool, and then by `afterwards`,
/// one answer a call.
async fn asked_to_nap(listed: Vec<ComponentDescriptor>, afterwards: Vec<Message>) -> Stuck {
    use caelumora_domain::tool::ToolCall;

    let nap = ToolCall {
        id: Some("1".to_string()),
        arguments: serde_json::json!({ "seconds": 0 }),
        ..make_tool_call("sleep")
    };
    let mut script = vec![saying("Hi."), Message { tool_calls: vec![nap], ..saying("") }];
    script.extend(afterwards);
    let mut components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "sleep".to_string(),
            kind: SLEEP_KIND.to_string(),
            params: serde_json::json!({ "requires_approval": false }),
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
        }],
    };
    components.component_nodes.extend(listed);
    let mut stuck = with_an_agent(Arc::new(Scripted::new(script)), components).await;
    answered(&mut stuck.events, stuck.agent).await;
    settled(&stuck.helper, ActorState::Idle).await;
    stuck
        .channel
        .user_actor(alice_of(&stuck.context))
        .unwrap()
        .send_message(None, Some("take a nap".to_string()), None, Vec::new())
        .await
        .unwrap();
    stuck
}

#[tokio::test]
async fn an_agent_carries_out_each_activity_due_once_before_each_call_one_after_the_other() {
    let tidying = |id: &str| ComponentDescriptor::new(id, "test_tidying");
    let Stuck { agent, mut events, running, .. } =
        asked_to_nap(vec![tidying("dusting"), tidying("sweeping")], vec![saying("Done.")]).await;
    let (told, answer) = activities_until_the_turn_ends(&mut events, agent).await;

    let before_a_call = [
        "busy",
        "started dusting",
        "finished: dusting done",
        "busy",
        "started sweeping",
        "finished: sweeping done",
    ];
    let expected: Vec<String> =
        before_a_call.iter().chain(before_a_call.iter()).map(|told| told.to_string()).collect();
    assert_eq!(
        told, expected,
        "both, in their order, before the call of the sleep tool and the one after it"
    );
    assert!(answer.is_ok(), "the turn goes on after them");
    running.abort();
}

#[tokio::test]
async fn an_activity_cut_short_by_a_stop_is_carried_out_again_as_the_turn_resumes() {
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor::new("hanging", "test_hanging")],
    };
    let Stuck { channel, agent, mut events, running, .. } =
        with_an_agent(Arc::new(NeverAnswers), components).await;
    activity_started(&mut events, agent).await;

    stop_and_wait(&channel, running).await;
    let stored = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(
        stored.turn(ThreadId::line(channel.id())).map(|turn| turn.step),
        Some(ActorState::Busy),
        "stored at the step it stood at"
    );
    assert!(
        stored
            .turn(ThreadId::line(channel.id()))
            .is_some_and(|turn| turn.activities_tried.is_empty()),
        "not tried, since it was not done"
    );

    let (_restarted, mut events, running) = run_again(&channel).await;
    activity_started(&mut events, agent).await;
    running.abort();
}

/// Waits until the agent `agent` began an activity.
async fn activity_started(events: &mut broadcast::Receiver<Envelope>, agent: AgentId) {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent)
                && matches!(envelope.event, ActorEvent::ActivityStarted { .. })
            {
                break;
            }
        }
    })
    .await
    .expect("it begins an activity");
}

/// Issue 537: a part placed `Given` is given into the agent's transcript as its turn begins, and
/// its channel's history records it as given; once it holds nothing, the next turn takes it back
/// and is not given it, and the history records where the agent stopped reading it.
#[tokio::test]
async fn a_part_given_into_the_transcript_is_recorded_as_given_and_as_withdrawn() {
    let model = Arc::new(Gated::saying(&["Fine.", "Still."], 2));
    let components = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            params: serde_json::json!({ "position": "given" }),
            ..ComponentDescriptor::new("rules", "test_first_only")
        }],
    };
    let Stuck { channel, context, agent, running, .. } =
        with_an_agent(Arc::<Gated>::clone(&model), components).await;
    stored_once_answered(&channel, agent, 1).await;
    channel
        .user_actor(alice_of(&context))
        .unwrap()
        .send_message(None, Some("Still there?".to_string()), None, Vec::new())
        .await
        .expect("said");
    stored_once_answered(&channel, agent, 2).await;

    let seen = model.seen.lock().unwrap().clone();
    let rules = "[Rules]\nBe brief.";
    assert!(seen[0].iter().any(|c| c.contains(rules)), "the first turn is given it: {:?}", seen[0]);
    assert!(
        !seen[1].iter().any(|c| c.contains("[Rules]")),
        "the second is not given what no longer holds: {:?}",
        seen[1]
    );
    let entries = channel.history_entries().await;
    let given = entries
        .iter()
        .find_map(|entry| match entry {
            HistoryEntry::Given { message } if message.content.as_deref() == Some(rules) => {
                Some(message.id)
            }
            _ => None,
        })
        .expect("the history records the part as given");
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            HistoryEntry::Withdrawn { message, .. } if *message == given
        )),
        "and where the agent stopped reading it: {entries:?}"
    );
    running.abort();
}

/// Issue 602: an agent of the line taken into a thread begins its branch there as its branch of
/// the line stood at the message the thread hangs at - forked, what it heard up to it on it, not
/// what came after -, its join noted after; its branch of the line goes on without it.
#[tokio::test]
async fn an_agent_taken_into_a_thread_forks_its_branch_at_the_message_the_thread_hangs_at() {
    use caelumora_domain::channel::ThreadOrigin;
    use caelumora_domain::message::MessageId;
    let (channel, context, agent, _) =
        agent_channel(Arc::new(AllowAllPolicy::new()), Arc::new(NeverAnswers), Default::default())
            .await;
    let line = ThreadId::line(channel.id());
    let before = Message {
        sender_id: ActorId::Agent(AgentId::new()),
        sender_name: "hermes".to_string(),
        role: MessageRole::Assistant,
        content: Some("said before".to_string()),
        ..make_message(MessageId::new())
    };
    let after =
        Message { id: MessageId::new(), content: Some("said after".to_string()), ..before.clone() };
    channel.record_said(vec![before.clone(), after.clone()]).await;
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let stored_once = |done: Box<dyn Fn(&AgentChannelState) -> bool>| {
        let channel = Arc::clone(&channel);
        async move {
            time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(state) = channel.stored_agent_channel_state(agent).await {
                        if done(&state) {
                            break state;
                        }
                    }
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the agent's state comes to stand so")
        }
    };
    stored_once(Box::new(move |state| state.last_message == Some(after.id))).await;

    let origin = ThreadOrigin { thread: line, message: before.id };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");
    let helper = ActorId::Agent(agent);
    channel.take_into_thread_by_procedure(&context, thread, helper).await.expect("taken in");
    let joined = move |state: &AgentChannelState| {
        state.transcript.entries(thread).any(|node| joined_entry(&node.entry, helper))
    };
    let state = stored_once(Box::new(joined)).await;

    let contents = |thread| -> Vec<String> {
        state.transcript.messages(thread).into_iter().filter_map(|m| m.content).collect()
    };
    assert_eq!(contents(thread), ["said before"], "what it heard up to the thread's message");
    assert_eq!(contents(line), ["said before", "said after"], "the line went on");
    let joins = |thread| {
        state.transcript.entries(thread).filter(|node| joined_entry(&node.entry, helper)).count()
    };
    assert_eq!(joins(thread), joins(line) + 1, "its join of the thread on that branch alone");
    running.abort();
}

/// Issue 602: an agent mentioned in a thread it takes no part in is drawn into it: its branch of
/// the thread forks at the message the thread hangs at, it hears what mentioned it, and it answers
/// there.
#[tokio::test]
async fn an_agent_mentioned_in_a_thread_is_drawn_in_and_answers_there() {
    use caelumora_domain::channel::ThreadOrigin;
    use caelumora_domain::message::MessageId;
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("It is the plan.".to_string()),
        ..make_message(MessageId::new())
    };
    let model = Arc::new(Scripted::new([answer]));
    let (channel, context, agent, alice) =
        agent_channel(Arc::new(AllowAllPolicy::new()), model, Default::default()).await;
    let line = ThreadId::line(channel.id());
    let before = Message {
        sender_id: ActorId::Agent(AgentId::new()),
        sender_name: "hermes".to_string(),
        role: MessageRole::Assistant,
        content: Some("said before".to_string()),
        ..make_message(MessageId::new())
    };
    channel.record_said(vec![before.clone()]).await;
    let mut events = channel.observe();
    let running = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.run().await }
    });
    let origin = ThreadOrigin { thread: line, message: before.id };
    let thread =
        channel.open_thread_by_procedure(&context, ThreadSpec::new(origin)).await.expect("opened");

    let question = "@helper, what is this?";
    let actor = channel.user_actor(alice).unwrap();
    actor.send_message(Some(thread), Some(question.to_string()), None, Vec::new()).await.unwrap();
    let answered = time::timeout(Duration::from_secs(5), async {
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
    .expect("the agent answers");
    assert_eq!(answered.context.thread_id, thread, "there");
    let state = time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(state) = channel.stored_agent_channel_state(agent).await {
                if state.transcript.messages(thread).len() == 3 {
                    break state;
                }
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("its branch of the thread holds its answer");
    let contents: Vec<String> =
        state.transcript.messages(thread).into_iter().filter_map(|m| m.content).collect();
    assert_eq!(contents, ["said before", question, "It is the plan."], "forked, asked, answered");
    running.abort();
}

/// Whether `entry` notes that `participant` joined.
fn joined_entry(entry: &TranscriptEntry, participant: ActorId) -> bool {
    matches!(entry, TranscriptEntry::ParticipantJoined { participant: p, .. } if *p == participant)
}
