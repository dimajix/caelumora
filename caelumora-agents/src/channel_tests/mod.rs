//! The tests of the agents' logic in a running channel, and what only they
//! share: the models, tools, commands and prompt parts of a test, and a
//! channel of alice and an agent run here (`agent_channel`). They move with
//! the agents' logic into a crate of its own (issue 370); what they share
//! with the runtime's tests is `channel::testing`.

use crate::activity::{ActivityError, ActivityProvider};
use crate::command::AgentCommandProvider;
use crate::llm::{LLMClient, LLMResult};
use crate::message::{MessageProcessor, PerceiveInProcessor, ProcessingError};
use crate::notes::{AgentNotes, NoteKind};
use crate::plugin::AgentsPlugin;
use crate::prompt::ModelProfileProvider;
use crate::prompt::{ContextError, Prompt, PromptProvider};
use crate::situation::Situation;
use crate::testing::Scripted;
use crate::tool::{ToolError, ToolOutput, ToolProvider, ToolResult};
use crate::turn::Turn;
use anyhow::Result;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::actor::{EventContext, RetryTarget};
use caelumora_model::agent::testing::make_personality;
use caelumora_model::agent::{AgentId, HearsDuringTurn, Personality};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, ComponentFailPolicy};
use caelumora_model::confidentiality::Classification;
use caelumora_model::config::AppConfig;
use caelumora_model::context::ModelProfile;
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageEvent, MessageId};
use caelumora_model::question::{Answers, Questions};
use caelumora_model::security::AllowAllPolicy;
use caelumora_model::security::{AccessPolicy, ActionType, SecurityContext};
use caelumora_model::store::Persistence;
use caelumora_model::tool::{Tool, ToolParameters};
use caelumora_model::user::UserId;
use caelumora_model::work::TurnId;
use caelumora_runtime::actor::user::ChannelWatch;
use caelumora_runtime::channel::testing::*;
use caelumora_runtime::channel::*;
use caelumora_runtime::command::help_command::HelpCommandProvider;
use caelumora_runtime::command::CommandError;
use caelumora_runtime::command::{CommandExecutor, CommandResult, ProvidesCommands};
use caelumora_runtime::component::ComponentKind;
use caelumora_runtime::component::{ComponentFacets, ComponentRegistry};
use caelumora_runtime::testing::TestRuntime;
use futures::stream;
use futures::stream::BoxStream;
use serde_json::Value;
use std::future;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Receiver;
use tokio::task::JoinHandle;
use tokio::time;

mod agent;
mod agent_commands;
mod external_agent;
mod questions;

/// What the channel shows next of `wanted`'s kind from `agent`.
async fn shown_by(events: &mut Receiver<Envelope>, agent: AgentId, wanted: &str) -> ActorEvent {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id == ActorId::Agent(agent)
                && envelope.event.kind() == wanted
            {
                break envelope.event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the agent sends {wanted}"))
}

/// Stops the actors of `channel` (issue 377) and waits: they end their
/// tasks by themselves, within the grace, and the channel with them - not
/// aborted.
async fn stop_and_wait(channel: &ChannelLoop, running: JoinHandle<Result<()>>) {
    let stopping = Instant::now();
    channel.stop_all_actors().await;
    assert!(stopping.elapsed() < STOP_GRACE, "the actors ended by themselves");
    time::timeout(Duration::from_secs(1), running)
        .await
        .expect("and with them the channel")
        .expect("the channel's task was not aborted")
        .expect("the channel ended without an error");
}

/// The channel of `stopped` built again from the stores and running, with
/// what it takes in from then on: as when its session runs again.
async fn run_again(
    stopped: &ChannelLoop,
) -> (Arc<ChannelLoop>, Receiver<Envelope>, JoinHandle<Result<()>>) {
    let restarted = ChannelLoop::new(stopped.restarted_runtime()).await.unwrap();
    let events = restarted.observe();
    let running = tokio::spawn({
        let restarted = Arc::clone(&restarted);
        async move { restarted.run().await }
    });
    (restarted, events, running)
}

/// A model whose answer never comes: what a cancel has to interrupt.
#[derive(Debug)]
pub(super) struct NeverAnswers;

#[async_trait::async_trait]
impl LLMClient for NeverAnswers {
    async fn chat_completion(
        &self,
        _: &[Message],
        _: &[Tool],
    ) -> BoxStream<'static, LLMResult<MessageEvent>> {
        Box::pin(stream::pending())
    }
}

impl ModelProfileProvider for NeverAnswers {
    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

pub(super) async fn stuck_in_a_model_call() -> Stuck {
    let stuck = with_an_agent(Arc::new(NeverAnswers), Default::default()).await;
    settled(&stuck.helper, ActorState::Planning).await;
    stuck
}

/// A model of a test: its client and its profile from one object, as a
/// model's component gives both (issue 57).
pub(super) trait TestModel: LLMClient + ModelProfileProvider {}

impl<T: LLMClient + ModelProfileProvider> TestModel for T {}

/// A running channel with alice and an agent that talks to `model` and
/// has `components`, after alice said "hello".
async fn with_an_agent(model: Arc<dyn TestModel>, components: ComponentConfig) -> Stuck {
    with_an_agent_under(Arc::new(AllowAllPolicy::new()), model, components).await
}

/// `with_an_agent`, under `access_policy`.
async fn with_an_agent_under(
    access_policy: Arc<dyn AccessPolicy>,
    model: Arc<dyn TestModel>,
    components: ComponentConfig,
) -> Stuck {
    start(agent_channel(access_policy, model, components).await).await
}

/// A channel of alice and an agent that talks to `model` and has
/// `components`, under `access_policy`, built but not running.
pub(super) async fn agent_channel(
    access_policy: Arc<dyn AccessPolicy>,
    model: Arc<dyn TestModel>,
    components: ComponentConfig,
) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    agent_channel_hearing(HearsDuringTurn::default(), access_policy, model, components).await
}

/// A running channel with alice and an agent that talks to `model`, has
/// `components` and hears, between the steps of a turn, as `hears` says
/// (issue 431), after alice said "hello".
async fn with_an_agent_hearing(
    hears: HearsDuringTurn,
    model: Arc<dyn TestModel>,
    components: ComponentConfig,
) -> Stuck {
    start(agent_channel_hearing(hears, Arc::new(AllowAllPolicy::new()), model, components).await)
        .await
}

/// `agent_channel`, of an agent whose personality hears between the
/// steps of a turn as `hears` says (issue 431).
async fn agent_channel_hearing(
    hears: HearsDuringTurn,
    access_policy: Arc<dyn AccessPolicy>,
    model: Arc<dyn TestModel>,
    components: ComponentConfig,
) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    let store = Persistence::in_memory();
    let personality =
        Personality { hears_during_turn: hears, components, ..make_personality("plain") };
    let (context, agent, alice) = alice_and_an_agent(&store, "helper", personality).await;
    // The model is a component (issue 130): the stand-in is registered as
    // a kind of its own and made every agent's default.
    let components = move |registry: &mut ComponentRegistry| {
        registry.register(
            ComponentKind::new("test_model")
                .provides::<dyn LLMClient>()
                .provides::<dyn ModelProfileProvider>(),
            move |_descriptor, _context| {
                let mut facets = ComponentFacets::empty();
                let client: Arc<dyn LLMClient> = Arc::clone(&model) as _;
                let profile: Arc<dyn ModelProfileProvider> = Arc::clone(&model) as _;
                facets.insert::<dyn LLMClient>(client);
                facets.insert::<dyn ModelProfileProvider>(profile);
                Ok(facets)
            },
        );
        register_no_secrets(registry);
        register_asking_tool(registry);
        register_marking_command(registry);
        register_activities(registry);
    };
    let defaults = ComponentConfig {
        component_nodes: vec![ComponentDescriptor {
            id: "model".to_string(),
            kind: "test_model".to_string(),
            params: Value::Null,
            tags: Vec::new(),
            fail_policy: Default::default(),
        }],
    };
    let config = AppConfig { component_defaults: defaults, ..Default::default() };
    let runtime = TestRuntime {
        store,
        access_policy,
        config,
        components: Box::new(components),
        plugins: vec![&AgentsPlugin],
        ..Default::default()
    }
    .build();
    (channel_of_alice_and(runtime, alice, agent).await, context, agent, alice)
}

/// The states `agent` told the users of through `shown`, up to `until`.
async fn told_states(
    shown: &mut ChannelWatch,
    agent: AgentId,
    until: ActorState,
) -> Vec<ActorState> {
    time::timeout(Duration::from_secs(5), async {
        let mut states = Vec::new();
        loop {
            let envelope = shown.recv().await.expect("the channel is running");
            if let (ActorEvent::StateChanged(state), true) =
                (envelope.event, envelope.context.sender_id == ActorId::Agent(agent))
            {
                states.push(state);
                if state == until {
                    break states;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the agent tells of {until:?}"))
}

/// Why the agent `agent` gave its next turn up, as its `TurnGivenUp`
/// says: what its users are told of a failure, else the cause - with
/// the channel, if it was lowered.
async fn turn_given_up(events: &mut Receiver<Envelope>, agent: AgentId) -> String {
    use caelumora_model::turn::GivenUpReason;
    time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            if let ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                let cause = reason.cause();
                break match reason {
                    GivenUpReason::Failed { message } => message,
                    GivenUpReason::Lowered { channel } => format!("{cause} to {channel}"),
                    GivenUpReason::Left
                    | GivenUpReason::Retried
                    | GivenUpReason::CallLimit { .. } => cause.to_string(),
                };
            }
        }
    })
    .await
    .expect("the turn is given up")
}

/// Lets nothing through that mentions a secret.
#[derive(Debug)]
struct NoSecrets;

#[async_trait::async_trait]
impl MessageProcessor for NoSecrets {
    async fn process_messages(
        &self,
        input: Vec<Message>,
        _security: &SecurityContext,
    ) -> Result<Vec<Message>, ProcessingError> {
        Ok(input
            .into_iter()
            .filter(|m| !m.content.as_deref().unwrap_or_default().contains("secret"))
            .collect())
    }
}

impl PerceiveInProcessor for NoSecrets {}

/// A prompt part that changes with every message the agent hears: how
/// many entries its transcript was given, on every branch - also an
/// external agent's, which begins anew with every call.
#[derive(Debug)]
struct HeardCount;

#[async_trait::async_trait]
impl PromptProvider for HeardCount {
    async fn get_prompt(&self, situation: &Situation<'_>) -> Result<Prompt, ContextError> {
        Ok(Prompt::from(format!("[Heard]\n{}", situation.agent_channel_state().transcript.end())))
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
impl ToolProvider for Asking {
    fn description(&self) -> String {
        "asks first".to_string()
    }
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "asking".to_string(),
            description: "asks first".to_string(),
            parameters: ToolParameters::object(
                Vec::<(String, ToolParameters)>::new(),
                Vec::<String>::new(),
            ),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    async fn execute(
        &self,
        _name: &str,
        _params: &Value,
        answers: Option<&Answers>,
        _turn: &mut Turn,
    ) -> ToolResult<ToolOutput> {
        let answers =
            answers.ok_or_else(|| ToolError::ExecutionFailed("no answers".to_string()))?;
        if self.sensitive {
            return Ok(serde_json::json!({ "received": answers.responses.len() }).into());
        }
        Ok(serde_json::json!({ "answered_by": answers.answered_by_name, "responses": answers.responses }).into())
    }
    fn questions(&self, _name: &str, _params: &Value) -> ToolResult<Option<Questions>> {
        use caelumora_model::question::{Addressee, Question};
        Ok(Some(Questions {
            questions: vec![Question {
                header: "Secret".to_string(),
                question: "What is it?".to_string(),
                choices: Vec::new(),
                multi_select: false,
            }],
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

impl ProvidesCommands for Marking {
    fn commands(&self) -> Vec<Command> {
        vec![Command {
            name: "mark".to_string(),
            description: "Marks.".to_string(),
            usage: Some("[text]".to_string()),
            owners_only: false,
        }]
    }
}

#[async_trait::async_trait]
impl AgentCommandProvider for Marking {
    async fn execute(
        &self,
        _name: &str,
        args: &str,
        turn: &mut Turn<'_>,
    ) -> CommandResult<CommandOutput> {
        AgentNotes::note(turn, NoteKind::Public, format!("marked {args}"));
        if args == "fail" {
            return Err(CommandError::Failed {
                command: "mark".to_string(),
                reason: "it was told to".to_string(),
            });
        }
        Ok(CommandOutput::private("Marked."))
    }
}

fn register_marking_command(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_marking").provides::<dyn AgentCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::new(Marking));
            Ok(facets)
        },
    );
}

/// An activity that is always due and changes nothing (issue 464), done
/// as `test_tidying`, named by its component's id.
#[derive(Debug)]
struct Tidying(String);

#[async_trait::async_trait]
impl ActivityProvider for Tidying {
    fn activity(&self) -> &str {
        &self.0
    }

    fn is_due(&self, _: &Situation<'_>) -> bool {
        true
    }

    async fn carry_out(&self, _: &mut Turn<'_>) -> Result<String, ActivityError> {
        Ok(format!("{} done", self.0))
    }
}

/// An activity that is always due and never done (issue 464), done as
/// `test_hanging`: what a stop has to interrupt.
#[derive(Debug)]
struct Hanging;

#[async_trait::async_trait]
impl ActivityProvider for Hanging {
    fn activity(&self) -> &str {
        "hanging"
    }

    fn is_due(&self, _: &Situation<'_>) -> bool {
        true
    }

    async fn carry_out(&self, _: &mut Turn<'_>) -> Result<String, ActivityError> {
        future::pending().await
    }
}

/// Registers `Tidying` as the kind `test_tidying` and `Hanging` as
/// `test_hanging`.
fn register_activities(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_tidying").provides::<dyn ActivityProvider>(),
        |descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ActivityProvider>(Arc::new(Tidying(descriptor.id.clone())));
            Ok(facets)
        },
    );
    registry.register(
        ComponentKind::new("test_hanging").provides::<dyn ActivityProvider>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ActivityProvider>(Arc::new(Hanging));
            Ok(facets)
        },
    );
}

/// What reaches `watch` of a command carried out (issues 269, 299):
/// that it began, if it was told so, the states the agent told of
/// meanwhile, and the answer.
async fn command_events(watch: &mut ChannelWatch) -> (Option<Envelope>, Vec<ActorState>, Envelope) {
    time::timeout(Duration::from_secs(5), async {
        let (mut started, mut states) = (None, Vec::new());
        loop {
            let envelope = watch.recv().await.expect("the channel is running");
            match envelope.event {
                ActorEvent::CommandStarted { .. } => started = Some(envelope),
                ActorEvent::StateChanged(state) => states.push(state),
                ActorEvent::CommandExecuted { .. } | ActorEvent::CommandFailed { .. } => {
                    break (started, states, envelope)
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the command is answered")
}

fn register_asking_tool(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_asking_tool").provides::<dyn ToolProvider>(),
        |descriptor, _context| {
            let sensitive =
                descriptor.params.get("sensitive").and_then(Value::as_bool).unwrap_or(false);
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(Asking { sensitive }));
            Ok(facets)
        },
    );
}

fn register_no_secrets(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_no_secrets").provides::<dyn PerceiveInProcessor>(),
        |_descriptor, _context| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn PerceiveInProcessor>(Arc::new(NoSecrets));
            Ok(facets)
        },
    );
}
