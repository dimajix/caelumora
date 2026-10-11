//! Questions an agent's tool call puts to the users of a running channel.

use super::*;

/// A call of the `asking` tool, the component that gives it -
/// `sensitive` or not - and what the model says once it has its result.
fn a_call_asking(sensitive: bool) -> (Message, Message, caelumora_model::component::ComponentConfig) {
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::message::MessageRole;
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "asking".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
        id: "asking".to_string(),
        kind: "test_asking_tool".to_string(),
        params: serde_json::json!({ "sensitive": sensitive }),
        tags: vec![],
        fail_policy: ComponentFailPolicy::FailClosed,
    }] };
    (call, done, components)
}

/// The questions the users are asked, from `events`.
async fn questions_requested(events: &mut broadcast::Receiver<Envelope>) -> caelumora_model::question::QuestionRequest {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if let ActorEvent::QuestionRequested(request) = envelope.event {
                break request;
            }
        }
    }).await.expect("the users are asked")
}

#[tokio::test]
async fn a_call_waits_for_the_answers_and_the_turn_goes_on_with_them() {
    use caelumora_model::question::Response;
    let (call, done, components) = a_call_asking(false);
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
    let asked = questions_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;
    let alice = alice_of(&context);
    assert_eq!(asked.questions.asked(), [ActorId::User(alice)], "the owner, found among the users");

    // The turn is their record, as for approvals.
    let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    assert_eq!(stored.pending_questions().into_iter().map(|q| q.id).collect::<Vec<_>>(), [asked.id]);

    let actor = channel.user_actor(alice).unwrap();
    let error = actor.answer_questions(asked.id, Some(vec![])).await.expect_err("one response per question");
    assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::Question { .. })), "{error}");
    actor.answer_questions(asked.id, Some(vec![Response::Text { text: "Kaya".to_string() }])).await.expect("answered");

    let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Done."], "the turn goes on");
    settled(&helper, ActorState::Idle).await;
    let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    let result = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).next().expect("the call ran");
    assert!(result.content.contains("Kaya") && result.content.contains("alice"), "the tool gave the model what it was answered: {}", result.content);
    let recorded = |result: &caelumora_model::tool::ToolCallResult| result.answers.as_ref().map(|a| (a.answered_by, a.responses.clone()));
    let kaya = Some((ActorId::User(alice), vec![Response::Text { text: "Kaya".to_string() }]));
    assert_eq!(recorded(&result), kaya, "the transcript keeps the record of the answers");
    let history = channel.history().await;
    assert_eq!(history.iter().flat_map(|m| &m.tool_results).map(recorded).next(), Some(kaya), "and so does the history");
    let error = actor.answer_questions(asked.id, Some(vec![Response::Text { text: "again".to_string() }])).await.expect_err("closed");
    assert!(matches!(error.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: caelumora_model::question::QuestionError::NotOpen, .. })), "{error}");
    running.abort();
}

#[tokio::test]
async fn a_message_instead_of_the_answers_declines_them_and_is_heard_after_the_turn() {
    let (call, done, components) = a_call_asking(false);
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
    questions_requested(&mut events).await;
    settled(&helper, ActorState::WaitingForUser).await;

    channel.user_actor(alice_of(&context)).unwrap().send_message("rather not".to_string(), None).await.unwrap();
    let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Done."], "the turn goes on without the answers");
    let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    let result = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).next().expect("the call is answered");
    assert_eq!(result.status, caelumora_model::tool::ToolResultStatus::Declined);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap().transcript.messages().iter().any(|m| m.content.as_deref() == Some("rather not")) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the message is heard after the turn");
    running.abort();
}

#[tokio::test]
async fn sensitive_answers_reach_the_tool_alone_and_are_never_stored() {
    use caelumora_model::question::Response;
    let (call, done, components) = a_call_asking(true);
    let Stuck { channel, context, agent, helper, mut events, running } =
        with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([call, done].into()))), components).await;
    let asked = questions_requested(&mut events).await;
    assert_eq!(asked.shown_to(), Some(vec![ActorId::User(alice_of(&context))]));
    settled(&helper, ActorState::WaitingForUser).await;

    channel.user_actor(alice_of(&context)).unwrap().answer_questions(asked.id, Some(vec![Response::Text { text: "hunter2".to_string() }])).await.expect("answered");
    answered(&mut events, agent).await;
    settled(&helper, ActorState::Idle).await;
    let stored = serde_json::to_string(&channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap()).unwrap();
    assert!(!stored.contains("hunter2"), "neither in the turn nor in the transcript");
    assert!(channel.history().await.iter().all(|m| !format!("{m:?}").contains("hunter2")), "nor in the history");
    running.abort();
}

#[tokio::test]
async fn ask_user_asks_whom_it_names_and_gives_the_model_the_answers() {
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::message::MessageRole;
    use caelumora_model::question::Response;
    use caelumora_model::tool::{ToolCall, ToolResultStatus, ToolVisibility};

    let ask = |id: &str, whom: &str| Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some(id.to_string()),
            function_name: "ask_user".to_string(),
            arguments: serde_json::json!({ "questions": [{ "header": "Name", "question": "What should it be called?" }], "ask": [whom] }),
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        }],
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let done = Message { role: MessageRole::Assistant, content: Some("Done.".to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let components = ComponentConfig { component_nodes: vec![ComponentDescriptor::new("ask", crate::tool::ask_user_tool::ASK_USER_TOOL_KIND)] };
    let model = Scripted(std::sync::Mutex::new([ask("1", "@carol"), ask("2", "@alice"), done].into()));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_agent(Arc::new(model), components).await;

    // Nobody called carol takes part: the first call is answered at
    // once, and the model asks alice.
    let asked = questions_requested(&mut events).await;
    assert_eq!(asked.questions.asked(), [ActorId::User(alice_of(&context))]);
    settled(&helper, ActorState::WaitingForUser).await;
    channel.user_actor(alice_of(&context)).unwrap().answer_questions(asked.id, Some(vec![Response::Text { text: "Caelumora".to_string() }])).await.expect("answered");
    let said: Vec<_> = answered(&mut events, agent).await.iter().filter_map(|m| m.content.clone()).collect();
    assert_eq!(said, ["Done."]);

    let stored = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
    let results: Vec<_> = stored.transcript.messages().into_iter().flat_map(|m| m.tool_results).collect();
    let [unknown, answer] = results.as_slice() else { panic!("two calls: {results:?}") };
    assert_eq!(unknown.status, ToolResultStatus::InvalidArguments);
    assert!(unknown.content.contains("@carol"), "the model is told whom it named: {}", unknown.content);
    assert!(answer.content.contains("Caelumora") && answer.content.contains("alice"), "{}", answer.content);
    running.abort();
}
