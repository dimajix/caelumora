//! The external agent actor in a running channel: what it is given, how it answers, and the remote
//! tasks it keeps open.

use crate::plugin::AgentsPlugin;
use super::*;
use caelumora_model::value::name::Name;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::command::CommandExecutor;
use caelumora_runtime::runtime::{ChannelRuntime, RuntimeManager};
use crate::testing::test_registry;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, HistoryAccess, moderation_components};
use caelumora_model::security::{SecurityAttributes, SecurityContext, AccessPolicy};
use caelumora_model::session::{ChannelId, SessionId, Participant};
use std::sync::Arc;
use caelumora_model::value::name::ScopedName;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::AllowAllPolicy;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::actor::{ActorEvent, Envelope};
use caelumora_runtime::session::SessionManager;
use caelumora_runtime::session::testing::*;
use caelumora_model::confidentiality::{ConfidentialityRules, Classification, Visibility};
use caelumora_model::config::AppConfig;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::prompt::PromptProvider;
use caelumora_model::digest;
use caelumora_model::agent::{AgentChannelState, AgentId, RetryRefused};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::context::ContextItem;
use caelumora_model::external_agent::{OpenTaskState, TaskId};
use caelumora_model::work::Work;
use chrono::Utc;
use crate::external::ExternalAgent;
use crate::external::testing::ScriptedExternalAgent;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::sync::broadcast::Receiver;

/// A running channel with alice and an external agent (issue 179) that
/// `remote` stands in for, after alice said "hello". The remote agent is
/// its personality's component, of a kind registered for the test.
async fn with_an_external_agent(remote: Arc<ScriptedExternalAgent>) -> Stuck {
    let channel = external_channel(remote, external_components(), |_| None).await.expect("the external agent can be run");
    start(channel).await
}

/// The components of an external personality: the remote agent, of the
/// kind `external_channel` registers for it.
fn external_components() -> ComponentConfig {
    ComponentConfig { component_nodes: vec![ComponentDescriptor::new("remote", "test_remote")] }
}

/// A channel of alice and an external agent with `components`, built but
/// not running - with the agent's state in it `stored`, as a restart
/// finds it.
async fn external_channel(remote: Arc<ScriptedExternalAgent>, components: ComponentConfig, stored: impl FnOnce(AgentId) -> Option<AgentChannelState>) -> Result<(Arc<ChannelLoop>, SecurityContext, AgentId, UserId)> {
    use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
    use caelumora_model::scope::Scope;

    let store = Arc::new(MemoryStore::new());
    let personality = Personality {
        description: None,
        introduction: None,
        reasoning_visibility: Visibility::Channel,
        id: PersonalityId::new(),
        name: ScopedName::new("remote", Default::default()).unwrap(),
        kind: PersonalityKind::External,
        roles: None,
        components,
        security_attributes: Default::default(),
        deleted: None,
    };
    store.entity_stores().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
    let alice = user("alice");
    let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let agent = Agent::new(ScopedName::new("hermes", Scope::GlobalScope).unwrap(), personality.id, context.initiator_id);
    let agent_id = agent.id;
    store.entity_stores().users.create(&SecurityContext::system(), &alice).await.unwrap();
    store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();

    let access_policy: Arc<dyn AccessPolicy> = Arc::new(AllowAllPolicy::new());
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&access_policy)));
    let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
    registry.register(
        ComponentKind::new("test_remote").provides::<dyn ExternalAgent>(),
        move |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ExternalAgent>(Arc::<ScriptedExternalAgent>::clone(&remote));
            Ok(facets)
        },
    );
    register_no_secrets(&mut registry);
    register_heard(&mut registry);
    register_marking_command(&mut registry);
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), registry, &[&AgentsPlugin]).unwrap());
    let channel = Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId::new(),
        session_id: SessionId::new(),
        name: Name::new("main").unwrap(),
        components: moderation_components(None, Some("broadcast_policy")),
        participants: vec![Participant::User(alice.id), Participant::Agent(agent_id)],
        security_attributes: SecurityAttributes::default(),
        classification: Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        deleted: None,
        former: Vec::new(),
    };
    runtime.entity_stores().channels.create(&SecurityContext::system(), &channel).await.unwrap();
    if let Some(mut stored) = stored(agent_id) {
        runtime.state_store().put_agent_channel_state(agent_id, channel.id, &mut stored).await.unwrap();
    }
    let executor = CommandExecutor::empty(Arc::clone(runtime.access_policy()));
    let session_id = channel.session_id;
    let channel = ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), bare_session(session_id))).await?;
    Ok((channel, context, agent_id, alice.id))
}

/// Registers `Heard` as the kind `test_heard`.
fn register_heard(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_heard").provides::<dyn PromptProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PromptProvider>(Arc::new(Heard));
            Ok(facets)
        },
    );
}

#[tokio::test]
async fn an_external_agent_that_joins_keeps_what_it_may_see_and_on_joining_again_what_it_missed() {
    use caelumora_model::message::{MessageId, MessageRole};
    let remote = Arc::new(ScriptedExternalAgent::default());
    let (channel, context, agent, _) = external_channel(remote, external_components(), |_| None).await.unwrap();
    let message = |sender: ActorId, text: &str| Message {
        id: MessageId::new(), role: MessageRole::User, sender_id: sender, content: Some(text.to_string()),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    };
    let said = |text: &str| message(context.initiator_id, text);
    let (one, two, three) = (said("one"), said("two"), said("three"));
    let own = message(ActorId::Agent(agent), "mine");
    let actor = channel.actor_of(ActorId::Agent(agent)).unwrap();
    let kept = || async {
        channel.stored_agent_channel_state(agent).await.unwrap()
            .history.messages().into_iter().map(|m| m.content.unwrap()).collect::<Vec<_>>()
    };

    actor.catch_up(vec![two.clone()]).await.unwrap();
    assert_eq!(kept().await, ["two"], "what it may see");
    actor.catch_up(vec![one, two, own, three]).await.unwrap();
    assert_eq!(kept().await, ["two", "three"], "what it missed, and nothing older nor its own");
}

#[tokio::test]
async fn an_external_agent_does_not_answer_again() {
    use crate::external::ExternalEvent;
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    let built = external_channel(Arc::clone(&remote), external_components(), |_| None).await.expect("the external agent can be run");
    let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
    let answer = answered(&mut events, agent).await.pop().expect("an answer");

    channel.user_actor(alice_of(&context)).unwrap().request_retry(answer.id).await.expect("asked");
    let ActorEvent::RetryRejected { target, reason } = shown_by(&mut events, agent, "RetryRejected").await else { unreachable!() };
    assert_eq!((target, reason), (RetryTarget::Message(answer.id), RetryRefused::Elsewhere), "the remote agent keeps what it was given");
    running.abort();
}

#[tokio::test]
async fn an_external_agent_s_answer_under_way_is_not_asked_for_again() {
    let remote = Arc::new(ScriptedExternalAgent::default());
    let Stuck { channel, context, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    assert_eq!(channel.turn_of(ActorId::Agent(agent)), None, "it has no turn here to give up");
    let refused = channel.user_actor(alice_of(&context)).unwrap().request_retry_of_turn(ActorId::Agent(agent), TurnId::new()).await;
    assert!(matches!(refused.expect_err("refused").downcast_ref::<ChannelError>(), Some(ChannelError::TurnNotRetryable { .. })));
    running.abort();
}

#[tokio::test]
async fn what_an_external_agents_perceive_processors_drop_is_kept_but_never_given() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
        vec![Ok(ExternalEvent::Completed)],
    ]));
    let mut components = external_components();
    components.component_nodes.push(ComponentDescriptor::new("filter", "test_no_secrets"));
    let built = external_channel(Arc::clone(&remote), components, |_| None).await.expect("the external agent can be run");
    let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
    answered(&mut events, agent).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("the secret is 42".to_string(), None).await.unwrap();
    assert!(answered(&mut events, agent).await.is_empty(), "nothing new reached it, so it is not asked");

    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let contents = |messages: Vec<Message>| messages.into_iter().filter_map(|m| m.content).collect::<Vec<_>>();
    assert_eq!(contents(state.history.messages()), vec!["hello", "Hello, alice.", "the secret is 42"], "its history keeps all it was offered");
    assert_eq!(contents(state.transcript.all_entries().filter_map(|e| e.entry.message().cloned()).collect()), vec!["hello"], "its transcript what it was given");
    assert_eq!(remote.sent.lock().unwrap().len(), 1, "the secret was not given to the remote agent");

    alice.send_message("and now?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;
    let sent = remote.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["and now?"], "what was dropped is not offered again");
    running.abort();
}

fn task(id: &str) -> TaskId {
    TaskId(id.to_string())
}

/// What the remote agent was given in the call `answer` came of: the
/// context recorded with it in the `state` of `agent` (issue 205).
fn given_for(state: &AgentChannelState, agent: AgentId, answer: &Message) -> Vec<String> {
    let seen = AgentContextView::of_state(ActorId::Agent(agent), state.clone());
    seen.context_of(answer.id).expect("recorded with the answer").items.into_iter()
        .filter_map(|item| match item {
            ContextItem::Conversation { message } => message.content,
            ContextItem::Prompt { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn an_external_agent_is_given_what_was_said_since_it_was_last_asked_and_answers_in_the_channel() {
    use crate::external::{ExternalEvent, Said};
    use caelumora_model::context::{ModelProfile, SenderFormat};
    use caelumora_model::external_agent::ConversationId;

    let conversation = ConversationId("c-1".to_string());
    // Told who said what, by the handle (issue 135).
    let marking = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![
            Ok(ExternalEvent::Conversation(conversation.clone())),
            Ok(ExternalEvent::Started(task("t-1"))),
            Ok(ExternalEvent::Partial("Hel".to_string())),
            Ok(ExternalEvent::Message("Hello, alice.".to_string())),
            Ok(ExternalEvent::Completed),
        ],
        vec![Ok(ExternalEvent::Started(task("t-2"))), Ok(ExternalEvent::Message("Fine.".to_string())), Ok(ExternalEvent::Completed)],
    ]).marking(marking));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

    let first = answered(&mut events, agent).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].content.as_deref(), Some("Hello, alice."), "the whole message replaces its pieces");
    assert_eq!(first[0].sender_id, ActorId::Agent(agent));
    assert_eq!(channel.whole_history().await.last().map(|m| m.id), Some(first[0].id), "in the channel's history");

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("How are you?".to_string(), None).await.unwrap();
    let second = answered(&mut events, agent).await;
    assert_eq!(second[0].content.as_deref(), Some("Fine."));

    let sent = remote.sent.lock().unwrap().clone();
    let said = |text: &str| Said { sender: "alice".to_string(), text: text.to_string() };
    assert_eq!(sent[0].0, None, "the remote agent opens the conversation");
    assert_eq!(sent[0].1.said, vec![said("@alice: hello")], "marked with its sender, as the component's profile says");
    assert_eq!(sent[1].0, Some(conversation.clone()), "and keeps it");
    assert_eq!(sent[1].1.said, vec![said("@alice: How are you?")], "only what it has not heard, not its own answer");

    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.remote.conversation, Some(conversation));
    assert!(state.remote.open_tasks.is_empty(), "both tasks ended");
    let kept: Vec<_> = state.history.messages().into_iter().filter_map(|m| m.content).collect();
    assert_eq!(kept, vec!["hello", "Hello, alice.", "How are you?", "Fine."], "what it heard and what it answered");
    let given: Vec<_> = state.transcript.all_entries().filter_map(|e| e.entry.message()?.content.clone()).collect();
    assert_eq!(given, vec!["hello", "How are you?"], "what the remote agent was given, as received");
    assert!(state.transcript.messages().is_empty(), "all of it sent: nothing to give next");
    // The context of each answer is what was given in its call (issue 205).
    assert_eq!(given_for(&state, agent, &first[0]), ["@alice: hello"], "as it was given, marked");
    assert_eq!(given_for(&state, agent, &second[0]), ["@alice: How are you?"], "not what it was given before");
    running.abort();
}

/// A message the remote agent was given that is modified or taken back
/// is told to it with its next call, at its place, in the name of its
/// author (issue 236) - and is part of what that call's answer was given.
#[tokio::test]
async fn an_external_agent_tells_its_remote_agent_of_a_message_it_was_given_that_was_modified_or_taken_back() {
    use crate::external::{ExternalEvent, Said};
    use caelumora_model::context::{ModelProfile, SenderFormat};
    use caelumora_model::external_agent::ConversationId;

    let marking = ModelProfile { sender_format: SenderFormat::Prefix, ..ModelProfile::default() };
    let answer = |text: &str| vec![Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
    let mut first = vec![Ok(ExternalEvent::Conversation(ConversationId("c-1".to_string())))];
    first.extend(answer("Hello."));
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![first, answer("Fine."), answer("Bye.")]).marking(marking));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    answered(&mut events, agent).await;
    let alice = channel.user_actor(alice_of(&context)).unwrap();
    let id_of = |history: Vec<Message>, text: &str| history.into_iter().find(|m| m.content.as_deref() == Some(text)).map(|m| m.id).expect("said");

    alice.modify_message(id_of(channel.whole_history().await, "hello"), "hello, corrected".to_string(), None).await.unwrap();
    alice.send_message("And?".to_string(), None).await.unwrap();
    let second = answered(&mut events, agent).await;
    alice.revoke_message(id_of(channel.whole_history().await, "And?")).await.unwrap();
    alice.send_message("Well?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;

    let sent = remote.sent.lock().unwrap().clone();
    let said = |text: &str| Said { sender: "alice".to_string(), text: text.to_string() };
    assert_eq!(sent[1].1.said, vec![said("@alice: [changed an earlier message \"hello\"; it now reads:] hello, corrected"), said("@alice: And?")], "told before what was said since");
    assert_eq!(sent[2].1.said, vec![said("@alice: [took back an earlier message \"And?\"; it no longer stands.]"), said("@alice: Well?")]);
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(given_for(&state, agent, &second[0]), ["@alice: [changed an earlier message \"hello\"; it now reads:] hello, corrected", "@alice: And?"], "what its answer was given, word for word");
    assert!(state.history.messages().iter().all(|m| m.notice_of.is_none()), "a notice is never in the history");
    running.abort();
}

/// An external agent tells where it stands as an agent run here does
/// (issue 243): working from before its answer starts until after it
/// ends.
#[tokio::test]
async fn an_external_agent_tells_that_it_works_while_it_answers() {
    use crate::external::ExternalEvent;
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![vec![
        Ok(ExternalEvent::Started(task("t-1"))),
        Ok(ExternalEvent::Message("Hello, alice.".to_string())),
        Ok(ExternalEvent::Completed),
    ]]));
    let Stuck { mut events, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    let told = tokio::time::timeout(Duration::from_secs(5), async {
        let mut told = Vec::new();
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                // Where it starts, before anything was asked of it.
                ActorEvent::StateChanged(ActorState::Idle) if told.is_empty() => {}
                ActorEvent::StateChanged(ActorState::Idle) => { told.push("Idle"); break told }
                ActorEvent::StateChanged(state) => told.push(if state == ActorState::Planning { "Planning" } else { "other" }),
                ActorEvent::ResponseStarted { .. } => told.push("ResponseStarted"),
                ActorEvent::TurnFinished { .. } => told.push("TurnFinished"),
                _ => {}
            }
        }
    }).await.expect("it answers and is idle again");
    assert_eq!(told, ["Planning", "ResponseStarted", "TurnFinished", "Idle"]);
    running.abort();
}

#[tokio::test]
async fn an_external_agent_is_shown_carrying_out_a_command_as_an_agent_run_here_is() {
    use crate::external::ExternalEvent;
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![vec![
        Ok(ExternalEvent::Message("Hello, alice.".to_string())),
        Ok(ExternalEvent::Completed),
    ]]));
    let mut components = external_components();
    components.component_nodes.push(ComponentDescriptor::new("marking", "test_marking"));
    let Stuck { channel, context, agent, mut events, running, .. } = start(external_channel(remote, components, |_| None).await.expect("runs")).await;
    answered(&mut events, agent).await;
    let actor = channel.user_actor(alice_of(&context)).unwrap();
    let mut watch = actor.watch();

    actor.execute_command("mark", "@hermes once").await.expect("sent to the agent");
    let (started, states, answer) = command_events(&mut watch).await;
    assert!(matches!(started.map(|e| e.event), Some(ActorEvent::CommandStarted { .. })), "she is told it began");
    assert_eq!(states.last(), Some(&ActorState::CarryingOutCommand), "shown carrying it out: {states:?}");
    assert!(matches!(answer.event, ActorEvent::CommandExecuted { .. }), "{:?}", answer.event);
    assert_eq!(told_states(&mut watch.events, agent, ActorState::Idle).await, [ActorState::Idle], "and idle again after");
    running.abort();
}

#[tokio::test]
async fn an_external_agent_s_answer_carries_the_turn_its_start_announced() {
    use crate::external::ExternalEvent;
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![vec![
        Ok(ExternalEvent::Started(task("t-1"))),
        Ok(ExternalEvent::Partial("Hel".to_string())),
        Ok(ExternalEvent::Message("Hello, alice.".to_string())),
        Ok(ExternalEvent::Completed),
    ]]));
    let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

    let (started, partial, answer) = tokio::time::timeout(Duration::from_secs(5), async {
        let (mut started, mut partial) = (None, None);
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::ResponseStarted { turn_id } => started = Some(turn_id),
                ActorEvent::PartialMessageGenerated(m) => partial = m.turn_id,
                ActorEvent::TurnFinished { messages, .. } => break (started, partial, messages),
                _ => {}
            }
        }
    }).await.expect("it answers");
    let started = started.expect("its answer announced itself");
    assert_eq!(partial, Some(started), "what it streams is of the turn");
    assert_eq!(answer.iter().map(|m| m.turn_id).collect::<Vec<_>>(), [Some(started)], "and what it answers");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.history.messages().last().and_then(|m| m.turn_id), Some(started), "kept so in its history");
    running.abort();
}

#[tokio::test]
async fn an_external_agent_asked_several_times_while_it_answers_is_called_once_after_it() {
    use crate::external::{ExternalEvent, Said};

    let answer = |id: &str, text: &str| vec![Ok(ExternalEvent::Started(task(id))), Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
    let gate = Arc::new(Semaphore::new(0));
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        answer("t-1", "First."), answer("t-2", "Second."), answer("t-3", "Third."),
    ]).gated(Arc::clone(&gate)));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while remote.sent.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the remote agent is asked about hello");

    // Three messages, each asking the agent, while the remote one works.
    let alice = alice_of(&context);
    let queued = ["one", "two", "three"];
    for text in queued {
        channel.user_actor(alice).unwrap().send_message(text.to_string(), None).await.expect("taken while it answers");
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
    gate.add_permits(3);

    let said = |messages: Vec<Message>| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
    assert_eq!(said(answered(&mut events, agent).await), ["First."]);
    assert_eq!(said(answered(&mut events, agent).await), ["Second."], "one call answers what was queued");

    // Nothing is left to answer: told something new, the agent asks the
    // remote one about it, not about something stale.
    channel.user_actor(alice).unwrap().send_message("four".to_string(), None).await.expect("sent");
    assert_eq!(said(answered(&mut events, agent).await), ["Third."]);
    let sent: Vec<Vec<Said>> = remote.sent.lock().unwrap().iter().map(|(_, message)| message.said.clone()).collect();
    let texts = |said: &[Said]| said.iter().map(|s| s.text.clone()).collect::<Vec<_>>();
    assert_eq!(sent.len(), 3, "one call per batch");
    assert_eq!(texts(&sent[1]), queued, "given all that was queued, at once");
    assert_eq!(texts(&sent[2]), ["four"]);
    running.abort();
}

#[tokio::test]
async fn an_external_agent_that_fails_or_cannot_be_reached_fails_its_answer_and_goes_on() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Failed("out of coffee".to_string()))],
        vec![Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "refused".to_string() })],
    ]));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(remote).await;

    let failed = turn_given_up(&mut events, agent).await;
    assert!(failed.contains("hermes could not answer: out of coffee"), "{failed}");

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("Still there?".to_string(), None).await.unwrap();
    let unreachable = turn_given_up(&mut events, agent).await;
    assert!(unreachable.contains("cannot reach the external agent at https://agent.example"), "{unreachable}");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "the failed task ended");
    running.abort();
}

#[tokio::test]
async fn what_an_external_agent_could_not_send_is_sent_with_its_next_call() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Back again.".to_string())), Ok(ExternalEvent::Completed)],
    ]).unreachable_for(1));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    let unreachable = turn_given_up(&mut events, agent).await;
    assert!(unreachable.contains("cannot reach the external agent"), "{unreachable}");

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("Still there?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;
    let sent = remote.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["hello", "Still there?"], "nothing lost, nothing twice");
    running.abort();
}

/// The components of an external personality with prompt parts (issue
/// 204): a fixed one, and one that changes with what it hears.
fn external_components_with_prompts() -> ComponentConfig {
    let mut components = external_components();
    let mut frame = ComponentDescriptor::new("frame", "static_system_prompt_provider");
    frame.params = serde_json::json!({ "prompt": "[Frame]\nBe brief.", "position": "before_turn_input" });
    components.component_nodes.push(frame);
    components.component_nodes.push(ComponentDescriptor::new("heard", "test_heard"));
    components
}

#[tokio::test]
async fn an_external_agent_is_given_its_prompt_parts_first_and_again_only_when_they_change() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
        vec![Ok(ExternalEvent::Message("Fine.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| None).await.expect("the external agent can be run");
    let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
    let first = answered(&mut events, agent).await;
    channel.user_actor(alice_of(&context)).unwrap().send_message("How are you?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;

    let sent = remote.sent.lock().unwrap().clone();
    let prompts = &sent[0].1.prompts;
    assert_eq!(prompts, &["[Frame]\nBe brief.", "[Heard]\n1"], "first, in the order of the configuration, whatever their position");
    assert_eq!(sent[1].1.prompts, ["[Heard]\n3"], "again only what changed");

    // What it was given is the context of its answer (issue 205).
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    let seen = AgentContextView::of_state(ActorId::Agent(agent), state);
    let items = seen.context_of(first[0].id).expect("recorded with the answer").items;
    let given_prompts: Vec<_> = items.iter().filter_map(|item| match item {
        ContextItem::Prompt { text, .. } => Some(text.as_str()),
        _ => None,
    }).collect();
    assert_eq!(given_prompts, [prompts[0].as_str(), prompts[1].as_str()]);
    running.abort();
}

#[tokio::test]
async fn after_a_restart_an_external_agent_is_given_only_the_prompt_parts_it_was_not_given() {
    use crate::external::ExternalEvent;
    use caelumora_model::external_agent::ConversationId;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Hello again.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    // Before the restart it was given the frame, and how much it heard
    // then.
    let mut stored = AgentChannelState::default();
    stored.remote.converse_in(ConversationId("c-1".to_string()));
    stored.remote.give("frame".to_string(), digest::prompt("[Frame]\nBe brief."));
    stored.remote.give("heard".to_string(), digest::prompt("[Heard]\n0"));
    let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| Some(stored)).await.expect("the external agent can be run");
    let Stuck { agent, mut events, running, .. } = start(built).await;
    answered(&mut events, agent).await;

    let sent = remote.sent.lock().unwrap().clone();
    assert_eq!(sent[0].1.prompts, ["[Heard]\n1"], "what it has is not given again, what changed is");
    running.abort();
}

#[tokio::test]
async fn the_prompt_parts_an_external_agent_could_not_send_are_sent_with_its_next_call() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Back again.".to_string())), Ok(ExternalEvent::Completed)],
    ]).unreachable_for(1));
    let built = external_channel(Arc::clone(&remote), external_components_with_prompts(), |_| None).await.expect("the external agent can be run");
    let Stuck { channel, context, agent, mut events, running, .. } = start(built).await;
    turn_given_up(&mut events, agent).await;

    channel.user_actor(alice_of(&context)).unwrap().send_message("Still there?".to_string(), None).await.unwrap();
    answered(&mut events, agent).await;
    let sent = remote.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.prompts.len(), 2, "not lost with the send that failed");
    running.abort();
}

#[tokio::test]
async fn after_a_restart_an_external_agent_sends_what_it_heard_but_had_not_sent() {
    use crate::external::ExternalEvent;
    use caelumora_model::message::MessageRole;

    let said = |text: &str| Message { role: MessageRole::User, sender_name: "bob".to_string(), content: Some(text.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
    let mut stored = AgentChannelState::default();
    stored.perceive(vec![said("sent before")], vec![said("sent before")]);
    stored.transcript.start_anew();
    stored.perceive(vec![said("heard, then the crash")], vec![said("heard, then the crash")]);
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Noted.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    let built = external_channel(Arc::clone(&remote), external_components(), |_| Some(stored)).await.unwrap();
    let Stuck { agent, mut events, running, .. } = start(built).await;

    answered(&mut events, agent).await;
    let sent = remote.sent.lock().unwrap().clone();
    assert_eq!(sent[0].1.said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["heard, then the crash", "hello"]);
    running.abort();
}

#[tokio::test]
async fn a_cancel_ends_the_answer_of_an_external_agent_and_its_remote_task() {
    use crate::external::ExternalEvent;

    // The remote agent takes the task on and then works on it for ever.
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Partial("Let me think".to_string()))],
    ]));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
    settled(&helper, ActorState::Planning).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    channel.cancel(&context).await.expect("permitted");
    let ended = answered(&mut events, agent).await;
    assert_eq!(ended.first().and_then(|m| m.content.as_deref()), Some("Let me think"), "what came so far");
    settled(&helper, ActorState::Idle).await;
    assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the remote task is cancelled too");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty());
    running.abort();
}

#[tokio::test]
async fn an_external_agent_removed_while_it_answers_cancels_its_task_and_gives_its_answer_up() {
    use crate::external::ExternalEvent;
    use caelumora_model::turn::GivenUpReason;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1")))],
    ]));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
    settled(&helper, ActorState::Planning).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");
    tokio::time::timeout(Duration::from_secs(5), async {
        while remote.cancelled.lock().unwrap().is_empty() { tokio::time::sleep(Duration::from_millis(10)).await }
    }).await.expect("the remote task is cancelled");
    // It says so, as an agent does (issue 262), and answers nothing.
    let farewell = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::TurnGivenUp { reason, .. } => break reason,
                ActorEvent::TurnFinished { .. } => panic!("a stopped agent answers nothing"),
                _ => {}
            }
        }
    }).await.expect("the agent tells the channel it gave its answer up");
    assert_eq!(farewell, GivenUpReason::Stopped);
    running.abort();
}

/// What a deleted channel does with its actors before it aborts its
/// task (`SessionLoop::delete_channel`, issue 187): an external agent
/// has cancelled the remote task it followed by the time `stop_actors`
/// returns, and every actor - the approver too - ended by itself.
#[tokio::test]
async fn a_stopped_channel_waits_for_its_external_agent_to_cancel_its_open_task() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
    ]));
    let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    answered(&mut events, agent).await;

    channel.stop_all_actors().await;
    assert_eq!(*remote.cancelled.lock().unwrap(), vec![task("t-1")], "the open task is cancelled before the channel is aborted");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "and that is stored");
    tokio::time::timeout(Duration::from_secs(1), running).await
        .expect("every actor ended by itself, and with them the channel")
        .expect("the channel's task was not aborted")
        .expect("the channel ended without an error");
}

#[tokio::test]
async fn an_external_agent_that_asked_back_is_answered_within_the_same_task() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
        vec![Ok(ExternalEvent::Message("Sunny in Bonn.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

    let question = answered(&mut events, agent).await;
    assert_eq!(question[0].content.as_deref(), Some("Which city?"), "the question is the answer");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::InputRequired)]);

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("Bonn".to_string(), None).await.unwrap();
    let answer = answered(&mut events, agent).await;
    assert_eq!(answer[0].content.as_deref(), Some("Sunny in Bonn."));
    assert_eq!(remote.sent.lock().unwrap()[1].1.answers, Some(task("t-1")), "the reply continues the task that asked");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "and it ended");
    assert_eq!(given_for(&state, agent, &question[0]), ["hello"], "the question back came of the first call");
    assert_eq!(given_for(&state, agent, &answer[0]), ["Bonn"], "the answer of the call that answered it (issue 205)");
    running.abort();
}

#[tokio::test]
async fn a_task_taken_up_again_is_done_as_the_call_that_took_it_up_though_the_remote_agent_does_not_tell_it() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    // The answer to the question comes without the task told again,
    // and breaks off; re-attached, the task finishes (issue 213).
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::InputRequired("Which city?".to_string()))],
        vec![Ok(ExternalEvent::Partial("Sun".to_string())), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
    ]).resuming(vec![vec![Ok(ExternalEvent::Message("Sunny in Bonn.".to_string())), Ok(ExternalEvent::Completed)]]));
    let Stuck { channel, context, agent, mut events, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;
    answered(&mut events, agent).await;

    let alice = channel.user_actor(alice_of(&context)).unwrap();
    alice.send_message("Bonn".to_string(), None).await.unwrap();
    let (request, said) = tokio::time::timeout(Duration::from_secs(5), async {
        let mut answers = Vec::new();
        while answers.len() < 2 {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            if let ActorEvent::TurnFinished { messages, .. } = envelope.event {
                answers.push((envelope.context, messages));
            }
        }
        (answers.remove(0), answers.remove(0))
    }).await.expect("answered with nothing, and said unasked");
    assert!(request.1.is_empty(), "the answer broke off: the task goes on");
    assert_eq!(said.1[0].content.as_deref(), Some("Sunny in Bonn."));
    assert_eq!(said.0.span_id, request.0.span_id, "in the span of the request that took it up, not the first one");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(given_for(&state, agent, &said.1[0]), ["Bonn"], "with what the call that took it up was given");
    running.abort();
}

#[tokio::test]
async fn an_external_personality_that_names_no_remote_agent_cannot_be_run() {
    let remote = Arc::new(ScriptedExternalAgent::default());
    let refused = external_channel(remote, Default::default(), |_| None).await.expect_err("nothing to reach");
    let message = format!("{refused:#}");
    assert!(message.contains("no component provides ExternalAgent, and it has no default"), "{message}");
}

#[tokio::test]
async fn an_open_task_whose_answer_broke_off_is_followed_and_its_end_said_unasked() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    // The connection breaks while the remote agent works; re-attached,
    // it finishes.
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
    ]).resuming(vec![vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)]]));
    let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(remote).await;

    assert!(answered(&mut events, agent).await.is_empty(), "the request is answered with nothing: the task has not failed, it goes on (issue 195)");
    let mut woke = false;
    let said = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            match envelope.event {
                ActorEvent::WakeupRequested => woke = true,
                ActorEvent::TurnFinished { messages, .. } => break messages,
                _ => {}
            }
        }
    }).await.expect("said without being asked");
    assert!(!woke, "nobody is woken for it: sleeping agents hear it all the same (issue 199)");
    assert_eq!(said[0].content.as_deref(), Some("Here it is."));
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "the task ended");
    assert_eq!(state.history.messages().last().and_then(|m| m.content.clone()).as_deref(), Some("Here it is."), "kept as said");
    assert_eq!(given_for(&state, agent, &said[0]), ["hello"], "with what the call that began the task was given (issue 205)");
    running.abort();
}

#[tokio::test]
async fn what_an_open_task_says_unasked_continues_the_answer_to_its_request() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() })],
    ]).resuming(vec![vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)]]));
    let Stuck { agent, mut events, running, .. } = with_an_external_agent(remote).await;

    // The answer to the request, with nothing, and later what the task
    // says unasked.
    let contexts = tokio::time::timeout(Duration::from_secs(5), async {
        let mut contexts = Vec::new();
        while contexts.len() < 2 {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::TurnFinished { .. }) {
                contexts.push(envelope.context);
            }
        }
        contexts
    }).await.expect("answered, and said unasked");
    assert_eq!(contexts[1].span_id, contexts[0].span_id, "in the span of the request");
    assert_eq!(contexts[1].agent_hops, contexts[0].agent_hops, "as far from the last user's message as the answer (issue 45)");
    running.abort();
}

#[tokio::test]
async fn an_answer_that_broke_off_is_said_once_in_full_when_its_task_ends() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    let broken = || vec![
        Ok(ExternalEvent::Started(task("t-1"))),
        Ok(ExternalEvent::Partial("Here ".to_string())),
        Ok(ExternalEvent::Partial("it".to_string())),
        Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() }),
    ];
    // Re-attached, one remote agent gives the whole result again (as
    // A2A's GetTask does), the other only what is new.
    for (resumed, expected) in [
        (vec![Ok(ExternalEvent::Message("Here it is.".to_string())), Ok(ExternalEvent::Completed)], "Here it is."),
        (vec![Ok(ExternalEvent::Message("is.".to_string())), Ok(ExternalEvent::Completed)], "Here it\n\nis."),
    ] {
        let remote = Arc::new(ScriptedExternalAgent::answering(vec![broken()]).resuming(vec![resumed]));
        let Stuck { channel, agent, mut events, running, .. } = with_an_external_agent(remote).await;

        assert!(answered(&mut events, agent).await.is_empty(), "nothing is said cut off");
        let said = answered(&mut events, agent).await;
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].content.as_deref(), Some(expected), "said once, in full");
        let state = channel.stored_agent_channel_state(agent).await.unwrap();
        let own: Vec<_> = state.history.messages().into_iter().filter(|m| m.sender_id == ActorId::Agent(agent)).filter_map(|m| m.content).collect();
        assert_eq!(own, vec![expected.to_string()], "and kept once");
        running.abort();
    }
}

#[tokio::test]
async fn an_external_agent_answers_as_what_it_was_given_whatever_it_declares() {
    use crate::external::ExternalEvent;

    // It declares to bring in nothing that is not public; it was told
    // what is internal.
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Partial("He".to_string())), Ok(ExternalEvent::Message("Hello, alice.".to_string())), Ok(ExternalEvent::Completed)],
    ]).declaring(Classification::PUBLIC));
    let Stuck { mut events, agent, running, .. } = with_an_external_agent(remote).await;

    let mut partials = Vec::new();
    let answer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::PartialMessageGenerated(m) => partials.push(m.classification),
                ActorEvent::TurnFinished { messages, .. } => break messages,
                _ => {}
            }
        }
    }).await.expect("the agent answers");
    assert_eq!(answer[0].classification, Classification::CHANNEL_DEFAULT, "what it was given, not what it declares");
    assert_eq!(partials, [Classification::CHANNEL_DEFAULT], "its pieces too");
    running.abort();
}

#[tokio::test]
async fn a_task_is_stored_as_soon_as_it_is_begun() {
    use crate::external::ExternalEvent;

    // The remote agent takes the request on, and is still at work.
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![vec![Ok(ExternalEvent::Started(task("t-1")))]]));
    let Stuck { channel, agent, running, .. } = with_an_external_agent(Arc::clone(&remote)).await;

    let state = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.stored_agent_channel_state(agent).await.unwrap();
            if !state.remote.open_tasks.is_empty() { break state }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("stored before the task ends, as a crash would find it");
    assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::Working)]);
    assert!(state.transcript.messages().is_empty(), "what it was given for it is sent: a restart gives nothing twice");
    let work = state.remote.work_of(&task("t-1")).expect("with the work it is");
    let given = state.transcript.prefix(&work.context.transcript.expect("with what it was given for it")).unwrap();
    assert_eq!(given.iter().filter_map(|e| e.entry.message()?.content.clone()).collect::<Vec<_>>(), ["hello"]);
    let sent_as = remote.called_as.lock().unwrap()[0].1.clone();
    assert_eq!(work.security_context, sent_as, "the authority of the request it was sent with");
    assert_eq!(work.event_context.sender_id, ActorId::Agent(agent), "the request's envelope, continued in the agent's name");
    assert_eq!(work.classification, Classification::CHANNEL_DEFAULT, "bound by the channel as it began");
    running.abort();
}

#[tokio::test]
async fn after_a_restart_an_open_task_is_followed_again_and_one_that_asked_back_waits() {
    use crate::external::ExternalEvent;
    use caelumora_model::external_agent::{OpenTask, RemoteState};

    let stored = AgentChannelState {
        remote: RemoteState {
            open_tasks: vec![
                OpenTask { task: task("t-working"), since: Utc::now(), state: OpenTaskState::Working, unsaid: None, work: None },
                OpenTask { task: task("t-asking"), since: Utc::now(), state: OpenTaskState::InputRequired, unsaid: None, work: None },
            ],
            ..RemoteState::default()
        },
        ..AgentChannelState::default()
    };
    let remote = Arc::new(ScriptedExternalAgent::default()
        .resuming(vec![vec![Ok(ExternalEvent::Message("Finished while you were away.".to_string())), Ok(ExternalEvent::Completed)]]));
    let (channel, _, agent, owner) = external_channel(Arc::clone(&remote), external_components(), |_| Some(stored)).await.unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let said = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            if let ActorEvent::CompleteMessageGenerated(message) = envelope.event { break message }
        }
    }).await.expect("the agent speaks");
    assert_eq!(said.content.as_deref(), Some("Finished while you were away."), "what it did meanwhile, unasked");
    assert!(said.turn_id.is_none(), "of no turn: answered by itself (issue 258)");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.remote.open_tasks.iter().map(|t| t.task.clone()).collect::<Vec<_>>(), vec![task("t-asking")], "the one that asked back waits for the next request");
    assert!(remote.sent.lock().unwrap().is_empty(), "nothing was sent to it");
    let called_as: Vec<_> = remote.called_as.lock().unwrap().iter().map(|(call, security)| (*call, security.initiator_id, security.owner_id, security.principal_id)).collect();
    assert_eq!(called_as, [("resume", ActorId::Agent(agent), ActorId::User(owner), ActorId::User(owner))], "a task stored without its contexts is followed as the agent itself, on its own initiative and its owner's behalf (issue 341)");
    running.abort();
}

#[tokio::test]
async fn an_open_task_is_followed_and_told_of_in_the_contexts_of_the_request_that_began_it() {
    use crate::external::ExternalEvent;
    use caelumora_model::external_agent::{OpenTask, RemoteState};

    let bob = ActorId::User(UserId::new());
    // Bob's request, answered by the agent - continued in its name.
    let request = EventContext::new(SessionId::default(), ChannelId::default(), bob, "bob".to_string());
    let begun = request.clone();
    let stored = move |agent| { let hermes = ActorId::Agent(agent); Some(AgentChannelState {
        remote: RemoteState {
            open_tasks: vec![OpenTask {
                task: task("t-1"),
                since: Utc::now(),
                state: OpenTaskState::Working,
                unsaid: None,
                work: Some(Work::new(
                    begun.next(hermes, "hermes".to_string()),
                    SecurityContext::new(bob, SessionId::default(), ChannelId::default()).next(hermes, ActorId::None, ActorId::None),
                    Classification::CHANNEL_DEFAULT,
                )),
            }],
            ..RemoteState::default()
        },
        ..AgentChannelState::default()
    }) };
    let remote = Arc::new(ScriptedExternalAgent::default()
        .resuming(vec![vec![Ok(ExternalEvent::Message("Done for bob.".to_string())), Ok(ExternalEvent::Completed)]]));
    let (channel, _, agent, _) = external_channel(Arc::clone(&remote), external_components(), stored).await.unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let told = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel runs");
            if envelope.context.sender_id == ActorId::Agent(agent) && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_)) {
                return envelope;
            }
        }
    }).await.expect("what came of the task is said");
    assert_eq!(told.security.initiator_id, bob, "said with the authority of bob's request");
    assert_eq!(told.context.span_id, request.span_id, "in its span");
    let called_as: Vec<_> = remote.called_as.lock().unwrap().iter().map(|(call, security)| (*call, security.initiator_id)).collect();
    assert_eq!(called_as, [("resume", bob)], "followed in bob's name, as it was begun");
    running.abort();
}

/// The state of external agent `agent` with one task `standing` so,
/// begun while its channel was confidential - above the channel as it is
/// now - in its own name.
fn begun_above(agent: AgentId, open: &str, standing: OpenTaskState) -> AgentChannelState {
    use caelumora_model::confidentiality::Confidentiality;
    use caelumora_model::external_agent::{OpenTask, RemoteState};
    let own = ActorId::Agent(agent);
    let work = Work::new(EventContext::default().next(own, "hermes".to_string()), SecurityContext::default().next(own, ActorId::None, ActorId::None), Classification::general(Confidentiality::Confidential));
    AgentChannelState {
        remote: RemoteState {
            open_tasks: vec![OpenTask { task: task(open), since: Utc::now(), state: standing, unsaid: None, work: Some(work) }],
            ..RemoteState::default()
        },
        ..AgentChannelState::default()
    }
}

#[tokio::test]
async fn lowering_the_channel_gives_up_an_external_agents_answer_under_way() {
    use crate::external::ExternalEvent;
    use caelumora_model::confidentiality::Confidentiality;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Started(task("t-1"))), Ok(ExternalEvent::Partial("Let me think".to_string()))],
    ]));
    let Stuck { channel, context, agent, helper, mut events, running } = with_an_external_agent(Arc::clone(&remote)).await;
    settled(&helper, ActorState::Planning).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    channel.reclassify(&context, Classification::general(Confidentiality::Public)).await.expect("permitted");
    let told = turn_given_up(&mut events, agent).await;
    assert!(told.contains("lowered"), "{told}");
    settled(&helper, ActorState::Idle).await;
    assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the remote task is cancelled");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "and forgotten");
    assert!(state.history.messages().iter().all(|m| m.sender_id != ActorId::Agent(agent)), "nothing of the answer is kept");
    running.abort();
}

#[tokio::test]
async fn a_task_begun_before_its_channel_was_lowered_is_given_up_instead_of_followed() {

    let remote = Arc::new(ScriptedExternalAgent::default());
    let (channel, _, agent, _) = external_channel(Arc::clone(&remote), external_components(), |agent| Some(begun_above(agent, "t-1", OpenTaskState::Working))).await.unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let told = turn_given_up(&mut events, agent).await;
    assert!(told.contains("lowered"), "{told}");
    let calls: Vec<&str> = remote.called_as.lock().unwrap().iter().map(|(call, _)| *call).collect();
    assert_eq!(calls, ["cancel"], "cancelled, not followed");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "and forgotten");
    running.abort();
}

#[tokio::test]
async fn a_task_that_asked_back_before_its_channel_was_lowered_is_not_taken_up() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::answering(vec![
        vec![Ok(ExternalEvent::Message("Hello.".to_string())), Ok(ExternalEvent::Completed)],
    ]));
    let built = external_channel(Arc::clone(&remote), external_components(), |agent| Some(begun_above(agent, "t-1", OpenTaskState::InputRequired))).await.unwrap();
    let Stuck { mut events, agent, running, .. } = start(built).await;

    answered(&mut events, agent).await;
    assert_eq!(remote.cancelled.lock().unwrap().as_slice(), &[task("t-1")], "the task that asked back is cancelled");
    assert_eq!(remote.sent.lock().unwrap()[0].1.answers, None, "and the request begins a new one");
    running.abort();
}

/// The state of an external agent with one task working, as a restart
/// finds it.
fn working_on(open: &str) -> AgentChannelState {
    use caelumora_model::external_agent::{OpenTask, RemoteState};
    AgentChannelState {
        remote: RemoteState {
            open_tasks: vec![OpenTask { task: task(open), since: Utc::now(), state: OpenTaskState::Working, unsaid: None, work: None }],
            ..RemoteState::default()
        },
        ..AgentChannelState::default()
    }
}

/// Whether `agent` said anything - a message or an answer - among the
/// events published so far.
fn said_anything(events: &mut Receiver<Envelope>, agent: AgentId) -> bool {
    std::iter::from_fn(|| events.try_recv().ok()).any(|envelope| {
        envelope.context.sender_id == ActorId::Agent(agent)
            && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_) | ActorEvent::TurnFinished { .. })
    })
}

#[tokio::test]
async fn a_followed_task_that_fails_says_so_and_keeps_nothing_of_what_came_before() {
    use crate::external::ExternalEvent;

    let remote = Arc::new(ScriptedExternalAgent::default()
        .resuming(vec![vec![Ok(ExternalEvent::Message("Half of it".to_string())), Ok(ExternalEvent::Failed("out of coffee".to_string()))]]));
    let (channel, _, agent, _) = external_channel(remote, external_components(), |_| Some(working_on("t-1"))).await.unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });

    let failed = turn_given_up(&mut events, agent).await;
    assert!(failed.contains("hermes could not finish: out of coffee"), "{failed}");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert!(state.remote.open_tasks.is_empty(), "the failed task ended");
    assert!(state.history.messages().is_empty(), "nothing the channel never saw is kept as said");
    running.abort();
}

#[tokio::test]
async fn a_followed_task_whose_stream_breaks_says_nothing_and_stays_open() {
    use crate::external::{ExternalAgentError, ExternalEvent};

    let remote = Arc::new(ScriptedExternalAgent::default()
        .resuming(vec![vec![
            Ok(ExternalEvent::Message("Half of it".to_string())),
            Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "reset".to_string() }),
        ]]));
    let (channel, _, agent, _) = external_channel(remote, external_components(), |_| Some(working_on("t-1"))).await.unwrap();
    let mut events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(!said_anything(&mut events, agent), "no half answer is published");
    let state = channel.stored_agent_channel_state(agent).await.unwrap();
    assert_eq!(state.remote.open_tasks.iter().map(|t| (t.task.clone(), t.state)).collect::<Vec<_>>(), vec![(task("t-1"), OpenTaskState::Working)], "left open, to be followed again");
    assert!(state.history.messages().is_empty(), "nothing is kept as said");
    running.abort();
}

#[tokio::test]
async fn an_external_agent_is_told_who_is_in_its_channel_and_who_joined_since() {
    use crate::external::ExternalEvent;
    use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
    use caelumora_model::scope::Scope;

    let answer = |text: &str| vec![Ok(ExternalEvent::Message(text.to_string())), Ok(ExternalEvent::Completed)];
    let remote = Arc::new(ScriptedExternalAgent::answering(vec![answer("Hello."), answer("Hi, bob.")]));
    let store = Arc::new(MemoryStore::new());
    let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
    registry.register(
        ComponentKind::new("test_remote").provides::<dyn ExternalAgent>(),
        { let remote = Arc::clone(&remote); move |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ExternalAgent>(Arc::<ScriptedExternalAgent>::clone(&remote));
            Ok(facets)
        } },
    );
    let access_policy = Arc::new(AllowAllPolicy::new());
    let secrets = Arc::new(SecretManager::new(vec![], Arc::<AllowAllPolicy>::clone(&access_policy)));
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), registry, &[&AgentsPlugin]).unwrap());
    let manager = SessionManager::new(Arc::clone(&runtime));
    let system = SecurityContext::system();

    let (alice, bob) = (user("alice"), user("bob"));
    let participants = record_users(&manager, &[&alice, &bob]).await;
    let mut members = ComponentDescriptor::new("members", "channel_members_prompt_provider");
    members.params = serde_json::json!({ "changes_only": true });
    let personality = Personality {
        id: PersonalityId::new(), name: ScopedName::new("remote", Default::default()).unwrap(), kind: PersonalityKind::External,
        roles: None, description: None, introduction: None,
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("remote", "test_remote"), members] },
        security_attributes: Default::default(), reasoning_visibility: Default::default(),
        deleted: None,
    };
    store.entity_stores().personalities.create(&system, &personality).await.unwrap();
    let hermes = Agent::new(ScopedName::new("hermes", Scope::GlobalScope).unwrap(), personality.id, ActorId::User(alice.id));
    store.entity_stores().agents.create(&system, &hermes).await.unwrap();

    let alice_ctx = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let mut session = make_session("roster", ActorId::None);
    session.participants = participants;
    session.participants.push(Participant::Agent(hermes.id));
    let session = create(&manager, &alice_ctx, session).await.unwrap();
    let channel = session.create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), ActorId::Agent(hermes.id)])).await.unwrap();
    let mut events = channel.user_actor(alice.id).unwrap().watch().events;
    let answered = |events: &mut Receiver<Envelope>| {
        let mut events = events.resubscribe();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let envelope = events.recv().await.expect("the channel is running");
                    if envelope.context.sender_id == ActorId::Agent(hermes.id) && matches!(envelope.event, ActorEvent::TurnFinished { .. }) { break }
                }
            }).await.expect("the agent answers")
        }
    };

    let waiting = answered(&mut events);
    channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
    waiting.await;
    let waiting = answered(&mut events);
    channel.add_participant(&alice_ctx, ActorId::User(bob.id), HistoryAccess::All).await.unwrap();
    channel.user_actor(alice.id).unwrap().send_message("bob is here".to_string(), None).await.unwrap();
    waiting.await;

    let sent = remote.sent.lock().unwrap().clone();
    let first = &sent[0].1.prompts;
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("[Who is in this channel]\nYou are hermes (@hermes).\n"), "{}", first[0]);
    assert!(first[0].contains("- alice (@alice), a person.") && !first[0].contains("bob"), "{}", first[0]);
    let second = &sent[1].1.prompts;
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].contains("- bob (@bob), a person.\n"), "the whole roster again: {}", second[0]);
    assert!(second[0].ends_with("Since you were last told who is here:\n- Joined: bob (@bob), a person."), "{}", second[0]);
    manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
}
