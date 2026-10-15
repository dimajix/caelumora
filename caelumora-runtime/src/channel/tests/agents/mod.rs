//! The tests of the agents' logic in a running channel, and what only they
//! share: the models, tools, commands and prompt parts of a test, and a
//! channel of alice and an agent run here (`agent_channel`). They move with
//! the agents' logic into a crate of its own (issue 370); what they share
//! with the runtime's tests is `channel::testing`.

use super::*;
use crate::command::CommandError;
use crate::component::ComponentKind;
use caelumora_model::actor::{EventContext, RetryTarget};
use caelumora_model::confidentiality::Classification;
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::work::TurnId;
use tokio::sync::broadcast;
use crate::view::AgentContextView;
use crate::agent_command::AgentCommandProvider;
use crate::agent_plugin::AgentsPlugin;
use crate::agent::work::AgentWork;
use crate::prompt::{ContextError, Prompt, PromptProvider};
use crate::message::ProcessingError;
use crate::llm::LLMClient;

mod agent;
mod agent_commands;
mod external_agent;
mod questions;

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


pub(super) async fn stuck_in_a_model_call() -> Stuck {
    let stuck = with_an_agent(Arc::new(NeverAnswers), Default::default()).await;
    settled(&stuck.helper, ActorState::Planning).await;
    stuck
}

/// A running channel with alice and an agent that talks to `model` and
/// has `components`, after alice said "hello".
async fn with_an_agent(model: Arc<dyn LLMClient>, components: ComponentConfig) -> Stuck {
    with_an_agent_under(Arc::new(AllowAllPolicy::new()), model, components).await
}

/// `with_an_agent`, under `access_policy`.
async fn with_an_agent_under(access_policy: Arc<dyn AccessPolicy>, model: Arc<dyn LLMClient>, components: ComponentConfig) -> Stuck {
    start(agent_channel(access_policy, model, components).await).await
}

/// A channel of alice and an agent that talks to `model` and has
/// `components`, under `access_policy`, built but not running.
pub(super) async fn agent_channel(access_policy: Arc<dyn AccessPolicy>, model: Arc<dyn LLMClient>, components: ComponentConfig) -> (Arc<ChannelLoop>, SecurityContext, AgentId, UserId) {
    let store = Arc::new(MemoryStore::new());
    let (context, agent, alice) = alice_and_an_agent(&store, components).await;
    let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&access_policy)));
    // The model is a component (issue 130): the stand-in is registered as
    // a kind of its own and made every agent's default.
    let mut registry = test_registry(Arc::<MemoryStore>::clone(&store));
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
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig { component_defaults: defaults, ..Default::default() }, registry, &[&AgentsPlugin]).unwrap());
    (channel_of_alice_and(runtime, alice, agent).await, context, agent, alice)
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

/// Lets nothing through that mentions a secret.
#[derive(Debug)]
struct NoSecrets;

#[async_trait::async_trait]
impl crate::message::MessageProcessor for NoSecrets {
    async fn process_messages(&self, input: Vec<Message>, _security: &SecurityContext) -> Result<Vec<Message>, ProcessingError> {
        Ok(input.into_iter().filter(|m| !m.content.as_deref().unwrap_or_default().contains("secret")).collect())
    }
}

impl crate::message::PerceiveInProcessor for NoSecrets {}

/// A prompt part that changes with every message the agent hears: how
/// many its history holds.
#[derive(Debug)]
struct Heard;

#[async_trait::async_trait]
impl PromptProvider for Heard {
    async fn get_prompt(&self, work: &AgentWork<'_>) -> Result<Prompt, ContextError> {
        Ok(Prompt::from(format!("[Heard]\n{}", work.agent_channel_state().history.messages().len())))
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
impl AgentCommandProvider for Marking {
    async fn execute(&self, _name: &str, args: &str, turn: &mut crate::agent::turn::Turn<'_>) -> crate::command::CommandResult<caelumora_model::command::CommandOutput> {
        crate::agent::notes::AgentNotes::note(turn, crate::agent::notes::NoteKind::Public, format!("marked {args}"));
        if args == "fail" {
            return Err(CommandError::Failed { command: "mark".to_string(), reason: "it was told to".to_string() });
        }
        Ok(caelumora_model::command::CommandOutput::private("Marked."))
    }
}

fn register_marking_command(registry: &mut ComponentRegistry) {
    registry.register(
        ComponentKind::new("test_marking").provides::<dyn AgentCommandProvider>(),
        |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::new(Marking));
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
