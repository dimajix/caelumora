//! The world of the tests of the agents' logic (issue 370): the world of the security suite
//! (`caelumora_kernel::testing::World`) with the agents' library and the models of a test, and what
//! those tests share - a turn to work in, the models, a call an agent makes in a workspace. Behind
//! the feature `test-support`, as the kernel's `testing` is.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use crate::library::{register_components, AgentsLibrary};
use crate::prompt::{NAMESPACE, PROMPTS};
use crate::tool::ToolProvider;
use crate::turn::Turn;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_api::component::{ComponentFacets, ComponentKind};
use caelumora_api::model::{ModelClient, ModelProfileProvider, ModelResult};
use caelumora_api::runtime::ActorRuntime;
use caelumora_kernel::agent::state::AgentStateHandle;
use caelumora_kernel::component::ComponentRegistry;
use caelumora_kernel::kernel::{AgentRuntime, ParticipantRuntime};
use caelumora_kernel::session::testing::detached_participant_in;
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::testing::TestKernel;
use caelumora_kernel::testing::{
    participant_runtime_in, test_registry as runtime_registry, with_unasked_model, World,
};
use caelumora_kernel::workspace::testing::WithWorkspace;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::channel::testing::bare_channel;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::context::ModelProfile;
use caelumora_model::message::{Message, MessageEvent, MessageId, MessageRole};
use caelumora_model::prompt_catalogue::{PromptCatalogue, PromptTexts, KERNEL, KERNEL_PROMPTS};
use caelumora_model::scope::Scope;
use caelumora_model::security::Grant;
use caelumora_model::session::testing::bare_session;
use caelumora_model::session::SessionId;
use caelumora_model::store::Persistence;
use caelumora_model::tool::{Tool, ToolCall, ToolVisibility};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::user::UserId;
use caelumora_model::work::Work;
use futures::stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::{sleep, timeout};

/// The world with the agents (`AgentsLibrary`) and nothing else.
pub async fn world() -> World {
    World::with(&[], &[&AgentsLibrary], |_| {}).await
}

/// A turn of `agent`, of `owner`, in the first channel of `session`,
/// begun in `state` - for what a component does in a turn.
pub fn turn<'a>(
    session: &Arc<SessionLoop>,
    agent: AgentId,
    owner: UserId,
    state: &'a mut AgentChannelState,
) -> Turn<'a> {
    let channel = &session.channels()[0];
    // On the owner's word; with the rights the kernel builds for the agent (issue 516).
    state.begin_turn(Work::new(
        EventContext::new(session.id(), channel.id(), ActorId::User(owner), String::new()),
        None,
        channel.classification(),
        Default::default(),
    ));
    Turn::of(state, agent_runtime(participant_runtime_in(channel, ActorId::Agent(agent))))
        .expect("a turn was begun")
}

/// What `provider` offers in a turn that reaches no channel (issue 290) -
/// for a provider whose offer does not depend on the place.
pub async fn offered_anywhere(provider: &dyn ToolProvider) -> Result<Vec<Tool>, String> {
    let mut state = AgentChannelState::default();
    state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default()));
    let Some(turn) = Turn::of(&mut state, detached_actor_runtime(AgentId::new())) else {
        return Err("no turn was begun".to_string());
    };
    provider.offered(&turn.situation()).await
}

/// What is asked in `world` when the agent of `owner`, with a component of `kind`,
/// asked by bob, makes the call its model makes - the kind "once",
/// registered with the world (`register_scripted`, `calling`) - in the
/// session's workspace, where the files `present` lie already - until
/// `done` says the call arrived, given where the storages keep their
/// files (`files`) and where the workspace's lie.
pub async fn asked_for_a_call(
    world: &World,
    kind: &str,
    present: &[&str],
    done: impl Fn(&Path, &Path) -> bool,
    owner: UserId,
) -> Vec<String> {
    let mut working = Personality {
        components: ComponentConfig {
            component_nodes: vec![
                ComponentDescriptor::new("model", ONCE_MODEL_KIND),
                ComponentDescriptor {
                    params: json!({ "requires_approval": false }),
                    ..ComponentDescriptor::new("workspace", kind)
                },
            ],
        },
        ..world.personality("working", None, &[]).await
    };
    world
        .kernel
        .entity_stores()
        .personalities
        .update(&World::system(), &mut working)
        .await
        .unwrap();
    let helper = world.agent("helper", owner, &working).await;
    let alice = World::as_user(world.alice);
    let docs = world
        .kernel
        .workspace_manager()
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "shell",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let session = world.shared_session(&[helper], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let main = Arc::clone(&session.channels()[0]);

    let place = world.files.path().join("shell").join(docs.id.to_string());
    for file in present {
        fs::create_dir_all(place.join(file).parent().unwrap()).unwrap();
        fs::write(place.join(file), "present").unwrap();
    }
    let (_, asked) = world
        .asked(async {
            main.user_actor(world.bob)
                .unwrap()
                .send_message(Some("@helper do it".to_string()), None, Vec::new())
                .await
                .expect("sent");
            timeout(Duration::from_secs(5), async {
                while !done(world.files.path(), &place) {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the call reached the workspace");
        })
        .await;
    world.manager.stop(&alice, session.id()).await.unwrap();
    asked
}

/// The runtime of the agent `agent` in a channel of no running session, never attached, with a
/// state of its own (`agent_runtime`): of the participant `detached_participant` makes.
pub fn detached_actor_runtime(agent: AgentId) -> Arc<dyn ActorRuntime> {
    agent_runtime(detached_participant(agent))
}

/// The runtime of the participant `agent` in a channel of no running session, never attached, as
/// the kernel's (`caelumora_kernel::session::testing`), with the agents' library - and so the
/// agents' texts (issue 57).
pub fn detached_participant(agent: AgentId) -> ParticipantRuntime {
    let session = bare_session(SessionId::new());
    let channel = bare_channel(session.id);
    detached_participant_in(
        TestKernel { libraries: vec![&AgentsLibrary], ..Default::default() }.build(),
        session,
        channel,
        ActorId::Agent(agent),
    )
}

/// The runtime of the agent `runtime` is made for, with a state of its own
/// (`AgentStateHandle::detached`), as `AgentRuntime::in_test` makes it.
pub fn agent_runtime(runtime: ParticipantRuntime) -> Arc<dyn ActorRuntime> {
    Arc::new(AgentRuntime::in_test(runtime, AgentStateHandle::detached()))
}

/// The prompt catalogue with the kernel's texts and the agents', as the kernel and `AgentsLibrary`
/// add them - for what resolves a context in a test.
pub fn catalogue() -> Arc<PromptCatalogue> {
    catalogue_of(&[])
}

/// Its texts as a model of the default variant is given them - for a
/// provider a test makes itself.
pub fn prompts() -> PromptTexts {
    PromptTexts::new(catalogue(), None)
}

/// The prompt catalogue with the kernel's texts, the agents' and a library's `texts` under
/// `namespace`, as the kernel and the libraries add them - for what a library's test makes itself.
pub fn catalogue_with(namespace: &str, texts: &str) -> Arc<PromptCatalogue> {
    catalogue_of(&[(namespace, texts)])
}

/// The prompt catalogue with the kernel's texts, the agents' and `more`, each its namespace and its
/// texts.
fn catalogue_of(more: &[(&str, &str)]) -> Arc<PromptCatalogue> {
    let mut catalogue = PromptCatalogue::new();
    // The texts are compiled in; that they can be read is what this tests too.
    for (namespace, texts) in [(KERNEL, KERNEL_PROMPTS), (NAMESPACE, PROMPTS)].iter().chain(more) {
        catalogue
            .add(namespace, texts)
            .unwrap_or_else(|e| panic!("the texts of '{namespace}' cannot be read: {e}"));
    }
    Arc::new(catalogue)
}

/// Their texts as a model of the default variant is given them.
pub fn prompts_with(namespace: &str, texts: &str) -> PromptTexts {
    PromptTexts::new(catalogue_with(namespace, texts), None)
}

/// The component kinds a test of the agents builds components with that has no kernel, resolving
/// the bundles a `ref` names in `store`: the kernel's (`testing::test_registry`) and an agent's
/// (`with_agent_kinds`).
pub fn test_registry(store: &Persistence) -> ComponentRegistry {
    let mut registry = runtime_registry(store);
    with_agent_kinds(&mut registry);
    registry
}

/// Registers with `registry` the kinds of an agent's components (`register_components`) and a model
/// that is not to be asked as the default (`with_unasked_model`) - for tests that build components
/// without a kernel, with the model a test's kernel gives its agents.
pub fn with_agent_kinds(registry: &mut ComponentRegistry) {
    register_components(&mut registry.library_components());
    with_unasked_model(registry);
}

/// A model that gives the answers it was given, one per call, and then
/// none: a call after the last is never answered.
#[derive(Debug)]
pub struct Scripted(pub Mutex<VecDeque<Message>>);

#[async_trait]
impl ModelClient for Scripted {
    async fn chat_completion(
        &self,
        _: ActorId,
        _: &[Message],
        _: &[Tool],
    ) -> BoxStream<'static, ModelResult<MessageEvent>> {
        match self.0.lock().unwrap().pop_front() {
            Some(answer) => stream::iter([Ok(MessageEvent::Complete(answer))]).boxed(),
            None => stream::pending().boxed(),
        }
    }
}

impl ModelProfileProvider for Scripted {
    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// What the agent `agent` told of its activities on `events` - its state
/// `Busy`, an activity begun, one ended with what it said - until its turn
/// ended, and how it ended: its answer, or why it was given up - failed,
/// what its users were told.
pub async fn activities_until_the_turn_ends(
    events: &mut broadcast::Receiver<Envelope>,
    agent: AgentId,
) -> (Vec<String>, Result<Vec<Message>, String>) {
    let mut told = Vec::new();
    let ended = timeout(Duration::from_secs(5), async {
        loop {
            let envelope = events.recv().await.expect("the channel is running");
            if envelope.context.sender_id != ActorId::Agent(agent) {
                continue;
            }
            match envelope.event {
                ActorEvent::StateChanged(ActorState::Busy) => told.push("busy".to_string()),
                ActorEvent::ActivityStarted { activity, .. } => {
                    told.push(format!("started {activity}"))
                }
                ActorEvent::ActivityFinished { output, .. } => {
                    told.push(format!("finished: {output}"))
                }
                ActorEvent::TurnFinished { messages, .. } => break Ok(messages),
                ActorEvent::TurnGivenUp { reason: GivenUpReason::Failed { message }, .. } => {
                    break Err(message)
                }
                ActorEvent::TurnGivenUp { reason, .. } => break Err(reason.cause().to_string()),
                _ => {}
            }
        }
    })
    .await
    .expect("the turn ends");
    (told, ended)
}

/// The kind of the model `asked_for_a_call` gives its agent.
pub const ONCE_MODEL_KIND: &str = "once";

/// What a model answers to call `function` with `arguments`.
pub fn calling(function: &str, arguments: Value) -> Message {
    Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall {
            id: Some("1".to_string()),
            function_name: function.to_string(),
            arguments,
            visibility: ToolVisibility::SHOWN,
            presentation: Default::default(),
        }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    }
}

/// What a model answers to say `text`.
pub fn saying(text: &str) -> Message {
    Message {
        id: MessageId::new(),
        role: MessageRole::Assistant,
        content: Some(text.to_string()),
        ..Message::new(Classification::CHANNEL_DEFAULT)
    }
}

/// What of `asked` - the questions a test recorded - is about the commands
/// of agents (issue 269).
pub fn about_agent_commands(asked: &[String]) -> Vec<&str> {
    asked.iter().filter(|line| line.contains("AgentCommand")).map(String::as_str).collect()
}

/// That `asked` asked for an agent's call on the workspace: all of it in
/// one context (issue 255) - the call and the workspace with alice's
/// rights, its owner's, asked by bob - and the workspace through the
/// channel as the agent reads every other place, with its owner's
/// clearance (issue 142): its terminal reached through the session, its
/// access list asked (`GetWorkspace`, issue 23), its flow both ways, before
/// anything is sent to the storage.
pub fn asked_as_a_call_on_the_workspace(asked: &[String]) {
    let calling: Vec<&String> = asked.iter().filter(|line| line.contains("CallTool")).collect();
    assert!(
        !calling.is_empty()
            && calling.iter().all(|line| *line == "alice as helper, asked by bob: CallTool yes"),
        "the call as its owner, asked by bob: {asked:?}"
    );
    let agents: Vec<&String> = asked.iter().filter(|line| line.contains("as helper")).collect();
    assert!(
        agents.iter().all(
            |line| line.starts_with("alice as helper, asked by bob:") && line.ends_with(" yes")
        ),
        "one context for all the turn does: {asked:?}"
    );
    assert!(
        agents.contains(&&"alice as helper, asked by bob: GetSession(alice's) yes".to_string()),
        "its terminal through the session: {asked:?}"
    );
    let listed: Vec<&String> =
        agents.iter().copied().filter(|line| line.contains("Workspace")).collect();
    assert!(listed.iter().any(|line| line.contains("GetWorkspace")), "its list asked: {asked:?}");
    assert!(
        listed.iter().all(|line| line.ends_with("Workspace(alice's) yes")),
        "and only of the workspace attached: {asked:?}"
    );
    assert!(
        agents.iter().filter(|line| line.contains("Publish")).count() >= 4,
        "the flow both ways, for the workspace and its terminal: {asked:?}"
    );
}

/// A model component of the kind `kind` that gives `answers`, one per call
/// (`Scripted`), registered with `registry`.
pub fn register_scripted(registry: &mut ComponentRegistry, kind: &str, answers: Vec<Message>) {
    let model = Arc::new(Scripted(Mutex::new(answers.into())));
    registry.register(
        ComponentKind::new(kind)
            .provides::<dyn ModelClient>()
            .provides::<dyn ModelProfileProvider>(),
        move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ModelClient>(Arc::<Scripted>::clone(&model));
            facets.insert::<dyn ModelProfileProvider>(Arc::<Scripted>::clone(&model));
            Ok(facets)
        },
    );
}

/// What tests of other crates do with a turn, under names of their own -
/// the turn's API is not widened by them.
impl Turn<'_> {
    /// Records `message` as said in the turn (`Turn::push_message`), without
    /// telling the channel: a transcript for a component to work on.
    pub fn record_said(&mut self, message: Message) -> Message {
        self.push_message(message)
    }
}

/// A turn of the agent of `with` in its channel, begun in `state`: where a
/// test of a tool on the workspace calls it.
pub fn workspace_turn<'a>(with: &WithWorkspace, state: &'a mut AgentChannelState) -> Turn<'a> {
    let channel = &with.session.channels()[0];
    // On the owner's word; with the rights the kernel builds for the agent, a participant of the
    // channel (issue 516).
    let asked_by = with.context.initiator_id;
    state.begin_turn(Work::new(
        EventContext::new(with.session.id(), channel.id(), asked_by, String::new()),
        None,
        channel.classification(),
        Default::default(),
    ));
    Turn::of(state, agent_runtime(participant_runtime_in(channel, ActorId::Agent(with.agent))))
        .expect("a turn was begun")
}
