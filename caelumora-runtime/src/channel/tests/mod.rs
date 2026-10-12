//! The tests of `ChannelLoop` itself, and the fixtures of every test of a running channel.

use super::*;
use caelumora_model::value::name::Name;
use crate::actor::Actor;
use crate::component::{ComponentImpl, ComponentKind, ComponentRegistry};
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::transport::EventSender;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext, RetryTarget};
use caelumora_model::channel::{Channel, ChannelId, Participant};
use caelumora_model::approval::Approver;
use caelumora_model::confidentiality::Classification;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::message_history::HistoryEntry;
use caelumora_model::work::TurnId;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext, SecurityManager};
use caelumora_model::session::SessionId;
use caelumora_model::user::UserId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use caelumora_model::value::name::ScopedName;
use crate::command::CommandExecutor;
use crate::command::help_command::HelpCommandProvider;
use crate::command::wakeup_command::WakeupCommandProvider;
use caelumora_model::component::{ComponentConfig, ComponentFailPolicy};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::DummySecurityManager;
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::user::User;
use caelumora_model::confidentiality::ConfidentialityRules;
use caelumora_model::config::AppConfig;
use caelumora_model::agent::AgentId;
use crate::llm::LLMClient;

mod agent;
mod commands;
mod external_agent;
mod questions;

pub(super) use crate::session::testing::user;

/// A channel of `users`, who are recorded in the store its actors are
/// read from.
pub(super) async fn channel_with(users: &[&User], security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
    let store = Arc::new(MemoryStore::new());
    let mut participants = Vec::new();
    for user in users {
        store.entity_stores().users.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    channel_of(store, participants, security).await
}

/// A channel of `participants`, whose actors are read from `store`.
async fn channel_of(store: Arc<MemoryStore>, participants: Vec<Participant>, security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
    let registry = ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
    channel_built(store, participants, security, registry, caelumora_model::channel::moderation_components(None, Some("broadcast_policy"))).await
}

/// A channel of `participants` with `components`, built by `registry`.
async fn channel_built(store: Arc<MemoryStore>, participants: Vec<Participant>, security: Arc<dyn SecurityManager>, registry: ComponentRegistry, components: ComponentConfig) -> Arc<ChannelLoop> {
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store.entity_stores(), secrets, security, Arc::new(ConfidentialityRules::default()), &AppConfig::default(), registry).unwrap());
    let channel = Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId::new(),
        session_id: SessionId::new(),
        name: Name::new("main").unwrap(),
        components,
        participants,
        security_attributes: SecurityAttributes::default(),
        classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        deleted: None,
        former: Vec::new(),
    };
    let builtin = |id: &str, facet: Arc<dyn crate::command::ChannelCommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
    let executor = CommandExecutor::new(
        Arc::clone(runtime.security_manager()),
        vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
    ).unwrap();
    let session_id = channel.session_id;
    ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await.unwrap()
}

#[tokio::test]
async fn a_participant_that_no_longer_exists_is_left_out() {
    let alice = user("alice");
    let store = Arc::new(MemoryStore::new());
    store.entity_stores().users.create(&SecurityContext::system(), &alice).await.unwrap();
    let (gone_user, gone_agent) = (UserId::new(), AgentId::new());

    let channel = channel_of(
        store,
        vec![Participant::User(alice.id), Participant::User(gone_user), Participant::Agent(gone_agent)],
        Arc::new(DummySecurityManager::new()),
    ).await;

    assert!(channel.user_actor(alice.id).is_some(), "the others take part");
    assert!(channel.actor_by_id(ActorId::User(gone_user)).is_none(), "a deleted user has no actor");
    assert!(channel.actor_by_id(ActorId::Agent(gone_agent)).is_none(), "nor has a deleted agent");
}

#[tokio::test]
async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
    let alice = user("alice");
    let bob = user("bob");
    let channel = channel_with(&[&alice, &bob], Arc::new(DummySecurityManager::new())).await;

    for participant in [&alice, &bob] {
        let actor = channel.user_actor(participant.id).expect("a participant has an actor");
        assert_eq!(actor.id(), ActorId::User(participant.id));
        assert_eq!(actor.name(), participant.username.as_str(), "the actor writes in this user's name");
    }
    assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
}

/// Answers every event with the same decision: tell `to` what was said,
/// and ask it to respond.
#[derive(Debug)]
pub(super) struct AskBack(pub(super) ActorId);

#[async_trait::async_trait]
impl crate::moderator::Moderator for AskBack {
    async fn route(&self, event: &Envelope) -> Vec<(ActorId, Envelope)> {
        vec![(self.0, event.clone()), (self.0, event.forward(ActorEvent::ResponseRequested))]
    }
    fn add_actor(&self, _: Arc<dyn Actor>) {}
    fn remove_actor(&self, _: ActorId) {}
}

/// A channel of `users` whose moderator is `moderator`: a component of a
/// kind of its own (issue 174).
pub(super) async fn channel_moderated_by(users: &[&User], moderator: Arc<dyn crate::moderator::Moderator>) -> Arc<ChannelLoop> {
    use crate::component::{ComponentFacets, ComponentKind};
    let store = Arc::new(MemoryStore::new());
    let mut participants = Vec::new();
    for user in users {
        store.entity_stores().users.create(&SecurityContext::system(), user).await.unwrap();
        participants.push(Participant::User(user.id));
    }
    let mut registry = ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
    registry.register(ComponentKind::new("test_moderator").provides::<dyn crate::moderator::Moderator>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn crate::moderator::Moderator>(Arc::clone(&moderator));
        Ok(facets)
    });
    let components = caelumora_model::channel::moderation_components(Some("test_moderator"), None);
    channel_built(store, participants, Arc::new(DummySecurityManager::new()), registry, components).await
}

/// An agent asleep that only tells whether it was woken or cancelled.
#[derive(Debug)]
pub(super) struct Sleeper {
    pub(super) id: ActorId,
    name: &'static str,
    pub(super) woken: AtomicBool,
    pub(super) cancelled: AtomicBool,
}

#[async_trait::async_trait]
impl Actor for Sleeper {
    fn id(&self) -> ActorId { self.id }
    fn kind(&self) -> String { "sleeper".to_string() }
    fn name(&self) -> Name { Name::new(self.name).expect("a name") }
    fn event_filter(&self) -> fn(&Envelope) -> bool { |_| true }
    async fn run(&self, _rx: Box<dyn crate::transport::EventReceiver>, _tx: Box<dyn EventSender>) -> Result<()> { Ok(()) }
    fn state(&self) -> ActorState {
        if self.woken.load(Ordering::SeqCst) { ActorState::Idle } else { ActorState::Sleeping }
    }
    fn wakeup(&self) { self.woken.store(true, Ordering::SeqCst); }
    fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); }
    fn stop(&self) {}
}

pub(super) fn sleeper() -> Arc<Sleeper> {
    sleeper_named("sleeper")
}

fn sleeper_named(name: &'static str) -> Arc<Sleeper> {
    Arc::new(Sleeper { id: ActorId::Agent(AgentId::new()), name, woken: AtomicBool::new(false), cancelled: AtomicBool::new(false) })
}

/// A channel of alice's with the sleeping agents `names`, as approvers -
/// who they are does not matter to finding them.
pub(super) async fn channel_with_sleepers(names: &[&'static str]) -> (Arc<ChannelLoop>, UserId, Vec<Arc<Sleeper>>) {
    use caelumora_model::approval::{ApprovalStrategy, ApproverId};
    let alice = user("alice");
    let channel = channel_with(&[&alice], Arc::new(DummySecurityManager::new())).await;
    let sleepers: Vec<_> = names.iter().map(|name| sleeper_named(name)).collect();
    for sleeper in &sleepers {
        channel.adopt(Arc::<Sleeper>::clone(sleeper), Member::Approver(Approver { id: ApproverId::new(), strategy: ApprovalStrategy::Any })).await;
    }
    (channel, alice.id, sleepers)
}

/// What the channel shows next of `wanted`'s kind from `agent`.
async fn shown_by(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: AgentId, wanted: &str) -> ActorEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) && envelope.event.kind() == wanted {
                break envelope.event;
            }
        }
    }).await.unwrap_or_else(|_| panic!("the agent sends {wanted}"))
}

/// A model whose answer never comes: what a cancel has to interrupt.
#[derive(Debug)]
pub(super) struct NeverAnswers;

#[async_trait::async_trait]
impl LLMClient for NeverAnswers {
    async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
        -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
    {
        Box::pin(futures::stream::pending())
    }

    fn profile(&self) -> caelumora_model::context::ModelProfile {
        Default::default()
    }
}

/// A running channel with alice and an agent whose model never answers,
/// the agent already in a turn: what a cancel or a stop has to end.
pub(super) struct Stuck {
    pub(super) channel: Arc<ChannelLoop>,
    pub(super) context: SecurityContext,
    pub(super) agent: AgentId,
    pub(super) helper: Arc<dyn Actor>,
    pub(super) events: broadcast::Receiver<Envelope>,
    pub(super) running: tokio::task::JoinHandle<Result<()>>,
}

pub(super) async fn stuck_in_a_model_call() -> Stuck {
    let stuck = with_an_agent(Arc::new(NeverAnswers), Default::default()).await;
    settled(&stuck.helper, ActorState::Planning).await;
    stuck
}

/// A running channel with alice and an agent that talks to `model` and
/// has `components`, after alice said "hello".
async fn with_an_agent(model: Arc<dyn LLMClient>, components: ComponentConfig) -> Stuck {
    with_an_agent_under(Arc::new(DummySecurityManager::new()), model, components).await
}

/// `with_an_agent`, under `security`.
async fn with_an_agent_under(security: Arc<dyn SecurityManager>, model: Arc<dyn LLMClient>, components: ComponentConfig) -> Stuck {
    start(agent_channel(security, model, components).await).await
}

/// A channel of alice and an agent that talks to `model` and has
/// `components`, under `security`, built but not running.
pub(super) async fn agent_channel(security: Arc<dyn SecurityManager>, model: Arc<dyn LLMClient>, components: ComponentConfig) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::scope::Scope;
    
    let store = Arc::new(MemoryStore::new());
    let personality = Personality {
        description: None,
        introduction: None,
        reasoning_visibility: caelumora_model::confidentiality::Visibility::Channel,
        id: PersonalityId::new(),
        name: ScopedName::new("plain", Default::default()).unwrap(),
        kind: Default::default(),
        roles: None,
        components,
        security_attributes: Default::default(),
        deleted: None,
    };
    store.entity_stores().personalities.create(&SecurityContext::system(), &personality).await.unwrap();
    let alice = user("alice");
    let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
    let agent = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), &personality, context.initiator_id);
    let agent_id = agent.id;
    store.entity_stores().users.create(&SecurityContext::system(), &alice).await.unwrap();
    store.entity_stores().agents.create(&SecurityContext::system(), &agent).await.unwrap();

    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
    // The model is a component (issue 130): the stand-in is registered as
    // a kind of its own and made every agent's default.
    let mut registry = ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
    registry.register(
        ComponentKind::new("test_model").provides::<dyn LLMClient>(),
        move |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn LLMClient>(Arc::clone(&model));
            Ok(facets)
        },
    );
    register_no_secrets(&mut registry);
    register_asking_tool(&mut registry);
    register_marking_command(&mut registry);
    let defaults = ComponentConfig {
        component_nodes: vec![caelumora_model::component::ComponentDescriptor {
            id: "model".to_string(),
            kind: "test_model".to_string(),
            params: serde_json::Value::Null,
            tags: Vec::new(),
            fail_policy: Default::default(),
        }],
    };
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), store.entity_stores(), secrets, security, Arc::new(ConfidentialityRules::default()), &AppConfig { component_defaults: defaults, ..Default::default() }, registry).unwrap());
    let channel = Channel {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        uses_workspace: true,
        id: ChannelId::new(),
        session_id: SessionId::new(),
        name: Name::new("main").unwrap(),
        components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
        participants: vec![
            Participant::User(alice.id),
            Participant::Agent(agent_id),
            Participant::Approver(caelumora_model::approval::Approver { id: caelumora_model::approval::ApproverId::new(), strategy: caelumora_model::approval::ApprovalStrategy::Any }),
        ],
        security_attributes: SecurityAttributes::default(),
        classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
        language: None,
        history_limits: Vec::new(),
        deleted: None,
        former: Vec::new(),
    };
    // Stored, so that leaving it can be stored too.
    runtime.entity_stores().channels.create(&SecurityContext::system(), &channel).await.unwrap();
    let executor = CommandExecutor::empty(Arc::clone(runtime.security_manager()));
    let session_id = channel.session_id;
    let channel = ChannelLoop::new(channel, ChannelRuntime::outside_a_session(runtime, Arc::new(executor), session_id)).await.unwrap();
    (channel, context, agent_id, alice.id)
}

/// A model that gives the answers it was given, one per call, and then
/// none.
#[derive(Debug)]
struct Scripted(std::sync::Mutex<std::collections::VecDeque<Message>>);

#[async_trait::async_trait]
impl LLMClient for Scripted {
    async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
        -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
    {
        match self.0.lock().unwrap().pop_front() {
            Some(answer) => Box::pin(futures::stream::iter([Ok(caelumora_model::message::MessageEvent::Complete(answer))])),
            None => Box::pin(futures::stream::pending()),
        }
    }

    fn profile(&self) -> caelumora_model::context::ModelProfile {
        Default::default()
    }
}

/// The states `agent` told the users of through `shown`, up to `until`.
async fn told_states(shown: &mut broadcast::Receiver<Envelope>, agent: AgentId, until: ActorState) -> Vec<ActorState> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut states = Vec::new();
        loop {
            let envelope = shown.recv().await.expect("the channel is running");
            if let (ActorEvent::StateChanged(state), true) = (envelope.event, envelope.context.sender_id == ActorId::Agent(agent)) {
                states.push(state);
                if state == until { break states }
            }
        }
    }).await.unwrap_or_else(|_| panic!("the agent tells of {until:?}"))
}

/// Why the agent `agent` gave its next turn up, as its `TurnGivenUp`
/// says: what its users are told of a failure, else the cause - with
/// the channel, if it was lowered.
async fn turn_given_up(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: AgentId) -> String {
    use caelumora_model::turn::GivenUpReason;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) { continue }
            if let ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                let cause = reason.cause();
                break match reason {
                    GivenUpReason::Failed { message } => message,
                    GivenUpReason::Lowered { channel } => format!("{cause} to {channel}"),
                    GivenUpReason::Stopped | GivenUpReason::Retried => cause.to_string(),
                };
            }
        }
    }).await.expect("the turn is given up")
}

/// Waits for `actor` to reach `wanted`.
pub(super) async fn settled(actor: &Arc<dyn Actor>, wanted: ActorState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while actor.state() != wanted { tokio::time::sleep(Duration::from_millis(10)).await }
    }).await.unwrap_or_else(|_| panic!("the agent does not reach {wanted:?}, it is {:?}", actor.state()));
}

/// Lets nothing through that mentions a secret.
#[derive(Debug)]
struct NoSecrets;

#[async_trait::async_trait]
impl crate::message::MessageProcessor for NoSecrets {
    async fn process_messages(&self, input: Vec<Message>, _security: &SecurityContext) -> Result<Vec<Message>> {
        Ok(input.into_iter().filter(|m| !m.content.as_deref().unwrap_or_default().contains("secret")).collect())
    }
}

impl crate::message::PerceiveInProcessor for NoSecrets {}

/// A prompt part that changes with every message the agent hears: how
/// many its history holds.
#[derive(Debug)]
struct Heard;

#[async_trait::async_trait]
impl crate::context::PromptProvider for Heard {
    async fn get_prompt(&self, work: &crate::agent::work::AgentWork<'_>) -> Result<crate::context::Prompt> {
        Ok(crate::context::Prompt::from(format!("[Heard]\n{}", work.agent_channel_state().history.messages().len())))
    }
}

/// Registers `NoSecrets` as the kind `test_no_secrets`.
/// A tool that asks the agent's owner one question before it runs - its
/// questions `sensitive` as its parameter says - and gives what it was
/// answered, or of sensitive answers only how many there were.
#[derive(Debug)]
struct Asking {
    sensitive: bool,
}

#[async_trait::async_trait]
impl crate::tool::ToolProvider for Asking {
    fn description(&self) -> String {
        "asks first".to_string()
    }
    fn tools(&self) -> Vec<caelumora_model::tool::Tool> {
        vec![caelumora_model::tool::Tool {
            name: "asking".to_string(),
            description: "asks first".to_string(),
            parameters: caelumora_model::tool::ToolParameters::object(Vec::<(String, caelumora_model::tool::ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    async fn execute(&self, _name: &str, _params: &serde_json::Value, answers: Option<&caelumora_model::question::Answers>, _turn: &mut crate::agent::turn::Turn) -> crate::tool::ToolResult<crate::tool::ToolOutput> {
        let answers = answers.ok_or_else(|| crate::tool::ToolError::ExecutionFailed("no answers".to_string()))?;
        if self.sensitive {
            return Ok(serde_json::json!({ "received": answers.responses.len() }).into());
        }
        Ok(serde_json::json!({ "answered_by": answers.answered_by_name, "responses": answers.responses }).into())
    }
    fn questions(&self, _name: &str, _params: &serde_json::Value) -> crate::tool::ToolResult<Option<caelumora_model::question::Questions>> {
        use caelumora_model::question::{Addressee, Question, Questions};
        Ok(Some(Questions {
            questions: vec![Question { header: "Secret".to_string(), question: "What is it?".to_string(), choices: Vec::new(), multi_select: false }],
            ask: vec![Addressee::Owner],
            sensitive: self.sensitive,
        }))
    }
}

/// A component whose agent carries out `/mark [text]` (issue 269): it
/// notes "marked <text>" and answers the issuer alone - with `fail`, it
/// notes and then fails, so that nothing of it may stay.
#[derive(Debug)]
struct Marking;

impl crate::command::ProvidesCommands for Marking {
    fn commands(&self) -> Vec<caelumora_model::command::Command> {
        vec![caelumora_model::command::Command { name: "mark".to_string(), description: "Marks.".to_string(), usage: Some("[text]".to_string()), owners_only: false }]
    }
}

#[async_trait::async_trait]
impl crate::command::AgentCommandProvider for Marking {
    async fn execute(&self, _name: &str, args: &str, turn: &mut crate::agent::turn::Turn<'_>) -> crate::command::CommandResult<caelumora_model::command::CommandOutput> {
        crate::agent::notes::AgentNotes::note(turn, crate::agent::notes::NoteKind::Public, format!("marked {args}"));
        if args == "fail" {
            return Err(crate::command::CommandError::Failed { command: "mark".to_string(), reason: "it was told to".to_string() });
        }
        Ok(caelumora_model::command::CommandOutput::private("Marked."))
    }
}

fn register_marking_command(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_marking").provides::<dyn crate::command::AgentCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::command::AgentCommandProvider>(Arc::new(Marking));
            Ok(facets)
        },
    );
}

/// What reaches `watch` of a command carried out (issues 269, 299):
/// that it began, if it was told so, the states the agent told of
/// meanwhile, and the answer.
async fn command_events(watch: &mut crate::actor::user::ChannelWatch) -> (Option<Envelope>, Vec<ActorState>, Envelope) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut started, mut states) = (None, Vec::new());
        loop {
            let envelope = watch.events.recv().await.expect("the channel is running");
            match envelope.event {
                ActorEvent::CommandStarted { .. } => started = Some(envelope),
                ActorEvent::StateChanged(state) => states.push(state),
                ActorEvent::CommandExecuted { .. } | ActorEvent::CommandFailed { .. } => break (started, states, envelope),
                _ => {}
            }
        }
    }).await.expect("the command is answered")
}

pub(super) fn says(text: &str) -> Message {
    Message {
        role: caelumora_model::message::MessageRole::Assistant,
        content: Some(text.to_string()),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    }
}

fn register_asking_tool(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_asking_tool").provides::<dyn crate::tool::ToolProvider>(),
        |descriptor, _context| {
            let sensitive = descriptor.params.get("sensitive").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::tool::ToolProvider>(Arc::new(Asking { sensitive }));
            Ok(facets)
        },
    );
}

fn register_no_secrets(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_no_secrets").provides::<dyn crate::message::PerceiveInProcessor>(),
        |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::message::PerceiveInProcessor>(Arc::new(NoSecrets));
            Ok(facets)
        },
    );
}

/// Runs `built` and has alice say "hello".
pub(super) async fn start((channel, context, agent, alice): (Arc<ChannelLoop>, SecurityContext, AgentId, UserId)) -> Stuck {
    let events = channel.observe();
    let running = tokio::spawn({ let channel = Arc::clone(&channel); async move { channel.run().await } });
    channel.user_actor(alice).unwrap().send_message("hello".to_string(), None).await.unwrap();
    let helper = channel.actor_by_id(ActorId::Agent(agent)).unwrap();
    Stuck { channel, context, agent, helper, events, running }
}

/// What `agent` answered next.
async fn answered(events: &mut tokio::sync::broadcast::Receiver<Envelope>, agent: AgentId) -> Vec<Message> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent) {
                if let ActorEvent::TurnFinished { messages, .. } = envelope.event { break messages }
            }
        }
    }).await.expect("the agent answers")
}

/// The user `context` acts for.
fn alice_of(context: &SecurityContext) -> UserId {
    match context.initiator_id {
        ActorId::User(user) => user,
        other => panic!("{other} is no user"),
    }
}
