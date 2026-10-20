//! The world of the tests of the agents' logic (issue 370): the world of
//! the security suite (`caelumora_runtime::testing::World`) with the
//! agents' plugin and the models of a test, and what those tests share - a
//! turn to work in, the models, a call an agent makes in a workspace.
//! Behind the feature `test-support`, as the runtime's `testing` is.

use caelumora_runtime::agent::state::AgentStateHandle;
use crate::turn::Turn;
use crate::tool::ToolProvider;
use crate::plugin::{register_components, AgentsPlugin};
use caelumora_runtime::component::{ComponentFacets, ComponentKind, ComponentRegistry};
use crate::llm::{LLMClient, LLMError, LLMResult};
use caelumora_runtime::plugin::Plugin;
use caelumora_runtime::runtime::RuntimeBuilder;
use anyhow::Result;
use caelumora_runtime::session::SessionLoop;
use caelumora_runtime::session::testing::detached_actor_runtime;
use caelumora_runtime::workspace::testing::WithWorkspace;
use caelumora_runtime::testing::{actor_runtime_in, test_registry as runtime_registry, World, STAFF};
use async_trait::async_trait;
use caelumora_model::actor::{ActorId, EventContext};
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::context::ModelProfile;
use caelumora_model::message::{Message, MessageEvent, MessageRole};
use caelumora_model::scope::Scope;
use caelumora_model::security::{Grant, SecurityContext};
use caelumora_model::store::ComponentStore;
use caelumora_model::tool::{Tool, ToolCall, ToolVisibility};
use caelumora_model::user::UserId;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use caelumora_model::work::Work;

/// The agents in the world of a test, as a plugin (issue 373): the agents'
/// plugin (`AgentsPlugin`) and a model that is not to be asked
/// (`UNASKED_MODEL_KIND`) as the default. A test gives it to the runtime's
/// `World` with its own plugins. Naming the default model is the program's
/// (issue 329), not a plugin's: this one stands in for the program of the
/// tests.
#[derive(Debug, Default)]
pub struct TestAgents;

impl Plugin for TestAgents {
    fn name(&self) -> &'static str {
        AgentsPlugin.name()
    }

    fn register(&self, runtime: &mut RuntimeBuilder) -> Result<()> {
        AgentsPlugin.register(runtime)?;
        with_unasked_model(runtime.components());
        Ok(())
    }
}

/// The world with the agents (`TestAgents`) and nothing else.
pub async fn world() -> World {
    World::with(&[&TestAgents], |_| {}).await
}

/// A turn of `agent`, of `owner`, in the first channel of `session`,
/// begun in `state` - for what a component does in a turn.
pub fn turn<'a>(session: &Arc<SessionLoop>, agent: AgentId, owner: UserId, state: &'a mut AgentChannelState) -> Turn<'a> {
    let channel = &session.channels()[0];
    let context = SecurityContext::new(ActorId::User(owner), session.id(), channel.id()).next(ActorId::Agent(agent), ActorId::User(owner), ActorId::User(owner)).with_roles(vec![*STAFF], None);
    state.begin_turn(Work::new(EventContext::new(session.id(), channel.id(), ActorId::None, String::new()), None, context, channel.classification(), Default::default()));
    Turn::of(agent, agent.to_string(), state, AgentStateHandle::detached(), actor_runtime_in(channel, ActorId::Agent(agent))).expect("a turn was begun")
}

/// What `provider` offers in a turn that reaches no channel (issue 290) -
/// for a provider whose offer does not depend on the place.
pub async fn offered_anywhere(provider: &dyn ToolProvider) -> Result<Vec<Tool>, String> {
    let mut state = AgentChannelState::default();
    state.begin_turn(Work::new(Default::default(), None, Default::default(), Default::default(), Default::default()));
    let agent = AgentId::new();
    let Some(turn) = Turn::of(agent, agent.to_string(), &mut state, AgentStateHandle::detached(), detached_actor_runtime()) else {
        return Err("no turn was begun".to_string());
    };
    provider.offered(&turn.situation()).await
}

/// What is asked in `world` when the agent of `owner`, with a component of `kind`,
/// asked by bob, makes the call its model makes - the kind "once",
/// registered with the world (`register_once`, `calling`) - in the
/// session's workspace, where the files `present` lie already - until
/// `done` says the call arrived, given where the storages keep their
/// files (`files`) and where the workspace's lie.
pub async fn asked_for_a_call(world: &World, kind: &str, present: &[&str], done: impl Fn(&Path, &Path) -> bool, owner: UserId) -> Vec<String> {
    let working = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", ONCE_MODEL_KIND),
            ComponentDescriptor {
                params: json!({ "requires_approval": false }),
                ..ComponentDescriptor::new("workspace", kind)
            },
        ] },
        ..world.personality("working", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &working).await.unwrap();
    let helper = world.agent("helper", owner, &working).await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "shell", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[helper], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let main = Arc::clone(&session.channels()[0]);

    let place = world.files.path().join("shell").join(docs.id.to_string());
    for file in present {
        std::fs::create_dir_all(place.join(file).parent().unwrap()).unwrap();
        std::fs::write(place.join(file), "present").unwrap();
    }
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper do it".to_string(), None).await.expect("sent");
        timeout(Duration::from_secs(5), async {
            while !done(world.files.path(), &place) {
                sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the call reached the workspace");
    }).await;
    world.manager.stop(&alice, session.id()).await.unwrap();
    asked
}

/// The kind of the model the agents of a test talk to unless they name one
/// (`TestAgents`, `with_agent_kinds`).
pub const UNASKED_MODEL_KIND: &str = "unasked_model";

/// The model of a test that asks none: a call fails, as one to a model
/// that cannot be reached.
#[derive(Debug)]
struct Unasked;

#[async_trait]
impl LLMClient for Unasked {
    async fn chat_completion(&self, _: &[Message], _: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>> {
        let refused = LLMError::Connection { endpoint: UNASKED_MODEL_KIND.to_string(), reason: "the model of a test is not to be asked".to_string() };
        futures::stream::iter([Err(refused)]).boxed()
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// The component kinds the tests of the agents build with, resolving
/// references to preconfigured components in `component_store`: the
/// runtime's (`testing::test_registry`) and an agent's (`with_agent_kinds`).
pub fn test_registry(component_store: Arc<dyn ComponentStore>) -> ComponentRegistry {
    let mut registry = runtime_registry(component_store);
    with_agent_kinds(&mut registry);
    registry
}

/// Registers with `registry` the kinds of an agent's components
/// (`register_components`) and a model that is not to be asked as the
/// default (`with_unasked_model`) - for tests that build components without
/// a runtime; a runtime gets them from `TestAgents`.
pub fn with_agent_kinds(registry: &mut ComponentRegistry) {
    register_components(registry);
    with_unasked_model(registry);
}

/// Registers with `registry` a model that is not to be asked
/// (`UNASKED_MODEL_KIND`) as the default - the core names none, each
/// program names its own (issue 329), and so do the tests.
fn with_unasked_model(registry: &mut ComponentRegistry) {
    registry.register(ComponentKind::new(UNASKED_MODEL_KIND).provides::<dyn LLMClient>(), |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn LLMClient>(Arc::new(Unasked));
        Ok(facets)
    });
    registry.set_default_kind::<dyn LLMClient>(UNASKED_MODEL_KIND).expect("registered as providing it");
}

/// A model that answers its first call with `answer`, and no other.
#[derive(Debug)]
pub struct Once(pub Mutex<Option<Message>>);

#[async_trait]
impl LLMClient for Once {
    async fn chat_completion(&self, _: &[Message], _: &[Tool]) -> BoxStream<'static, LLMResult<MessageEvent>> {
        match self.0.lock().unwrap().take() {
            Some(answer) => futures::stream::iter([Ok(MessageEvent::Complete(answer))]).boxed(),
            None => futures::stream::pending().boxed(),
        }
    }

    fn profile(&self) -> ModelProfile {
        Default::default()
    }
}

/// The kind of the model `asked_for_a_call` gives its agent.
pub const ONCE_MODEL_KIND: &str = "once";

/// What a model answers to call `function` with `arguments`.
pub fn calling(function: &str, arguments: Value) -> Message {
    Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: function.to_string(), arguments, visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    }
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
    assert!(!calling.is_empty() && calling.iter().all(|line| *line == "alice as helper, asked by bob: CallTool yes"), "the call as its owner, asked by bob: {asked:?}");
    let agents: Vec<&String> = asked.iter().filter(|line| line.contains("as helper")).collect();
    assert!(agents.iter().all(|line| line.starts_with("alice as helper, asked by bob:") && line.ends_with(" yes")), "one context for all the turn does: {asked:?}");
    assert!(agents.contains(&&"alice as helper, asked by bob: GetSession(alice's) yes".to_string()), "its terminal through the session: {asked:?}");
    let listed: Vec<&String> = agents.iter().copied().filter(|line| line.contains("Workspace")).collect();
    assert!(listed.iter().any(|line| line.contains("GetWorkspace")), "its list asked: {asked:?}");
    assert!(listed.iter().all(|line| line.ends_with("Workspace(alice's) yes")), "and only of the workspace attached: {asked:?}");
    assert!(agents.iter().filter(|line| line.contains("Publish")).count() >= 4, "the flow both ways, for the workspace and its terminal: {asked:?}");
}

/// A model component of the kind `kind` that answers its first call with
/// `answer`, registered with `registry`.
pub fn register_once(registry: &mut ComponentRegistry, kind: &str, answer: Message) {
    let model = Arc::new(Once(Mutex::new(Some(answer))));
    registry.register(ComponentKind::new(kind).provides::<dyn LLMClient>(), move |_, _| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn LLMClient>(Arc::<Once>::clone(&model));
        Ok(facets)
    });
}

/// A turn of the agent of `with` in its channel, begun in `state`: where a
/// test of a tool on the workspace calls it.
pub fn workspace_turn<'a>(with: &WithWorkspace, state: &'a mut AgentChannelState) -> Turn<'a> {
    let channel = &with.session.channels()[0];
    state.begin_turn(Work::new(EventContext::new(with.session.id(), channel.id(), ActorId::None, String::new()), None, with.context.clone(), channel.classification(), Default::default()));
    Turn::of(with.agent, "Jonathan".to_string(), state, AgentStateHandle::detached(), actor_runtime_in(channel, ActorId::Agent(with.agent))).expect("a turn was begun")
}
