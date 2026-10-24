//! Compaction through the whole runtime (issues 16, 284, 464): the plugin joined to the world of
//! the security suite - the ownership policy, recording what it is asked, and alice, bob and carol
//! (`caelumora_runtime::testing`) - with the agents' plugin. An agent compacts on its own as an
//! activity before a model call, and when its owner asks with `/compact`.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use caelumora_agents::testing::{about_agent_commands, activities_until_the_turn_ends, register_scripted, saying, TestAgents};
use caelumora_agents::tool::sleep_tool::SLEEP_TOOL_KIND;
use caelumora_compaction::component::COMPACTION_KIND;
use caelumora_compaction::plugin::CompactionPlugin;
use caelumora_model::actor::ActorEvent;
use caelumora_model::agent::{AgentId, Personality};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::Classification;
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::Message;
use caelumora_model::tool::{ToolCall, ToolVisibility};
use caelumora_runtime::channel::ChannelLoop;
use caelumora_runtime::command::CommandError;
use caelumora_runtime::session::SessionLoop;
use caelumora_runtime::testing::{shown, World};
use serde_json::json;
use std::sync::Arc;

/// The `compaction` component, as a personality lists it.
fn compaction() -> ComponentDescriptor {
    ComponentDescriptor::new("compaction", COMPACTION_KIND)
}

/// `message` as a call of a model whose context holds 1000 tokens answered it, given `prompt`
/// tokens.
fn measured(prompt: u64, message: Message) -> Message {
    Message { prompt_tokens: Some(prompt), completion_tokens: Some(10), context_size: Some(1000), ..message }
}

/// A world whose model, of the kind `scripted`, gives `answers`, one per call.
async fn world(answers: Vec<Message>) -> World {
    World::with(&[&TestAgents, &CompactionPlugin], |registry| register_scripted(registry, "scripted", answers)).await
}

/// The agent `helper` of alice in `world`, with the scripted model and `listed`, in a session she
/// shares with bob, and its channel.
async fn helper_with(world: &World, listed: Vec<ComponentDescriptor>) -> (Arc<SessionLoop>, Arc<ChannelLoop>, AgentId) {
    let mut components = vec![ComponentDescriptor::new("model", "scripted")];
    components.extend(listed);
    let personality = Personality {
        components: ComponentConfig { component_nodes: components },
        ..world.personality("helping", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &personality).await.unwrap();
    let helper = world.agent("helper", world.alice, &personality).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    (session, main, helper)
}

/// The agent `helper` of alice, with the sleep tool and `listed`, that answered her "@helper
/// hello" with "Hi." and then heard "@helper take a nap", which it answers by calling the sleep
/// tool, its context full once that call is in - 910 of 1000 -, and then by `afterwards`, one
/// answer a call; and what it told of its activities in the turn of the nap, and how that turn
/// ended. Mentioned, since nobody else asks it.
async fn napping_with(listed: Vec<ComponentDescriptor>, afterwards: Vec<Message>) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, AgentId, (Vec<String>, Result<Vec<Message>, String>)) {
    let nap = ToolCall { id: Some("1".to_string()), function_name: "sleep".to_string(), arguments: json!({ "seconds": 0 }), visibility: ToolVisibility::SHOWN, presentation: Default::default() };
    let mut script = vec![measured(100, saying("Hi.")), measured(900, Message { tool_calls: vec![nap], ..saying("") })];
    script.extend(afterwards);
    let world = world(script).await;
    let sleep = ComponentDescriptor { params: json!({ "requires_approval": false }), ..ComponentDescriptor::new("sleep", SLEEP_TOOL_KIND) };
    let (session, main, helper) = helper_with(&world, [vec![sleep], listed].concat()).await;
    let mut events = main.observe();
    let alice = main.user_actor(world.alice).unwrap();
    alice.send_message(Some("@helper hello".to_string()), None, Vec::new()).await.unwrap();
    activities_until_the_turn_ends(&mut events, helper).await.1.expect("answered");
    alice.send_message(Some("@helper take a nap".to_string()), None, Vec::new()).await.unwrap();
    let napped = activities_until_the_turn_ends(&mut events, helper).await;
    (world, session, main, helper, napped)
}

#[tokio::test]
async fn an_agent_whose_context_fills_up_compacts_it_on_its_own_before_its_next_call() {
    let (world, session, main, helper, (told, answer)) =
        napping_with(vec![compaction()], vec![measured(800, saying("They greeted each other.")), measured(300, saying("Done."))]).await;

    assert_eq!(told, [
        "busy".to_string(),
        "started compacting its context".to_string(),
        "finished: helper compacted its context: 2 messages replaced, 3 kept as they were.".to_string(),
    ], "the users see it compact, and what came of it");
    let said: Vec<_> = answer.expect("answered").iter().filter_map(|m| m.content.clone()).filter(|c| !c.is_empty()).collect();
    assert_eq!(said, ["Done."], "the turn goes on, the summary no part of its answer");
    let recorded = main.history_entries().await.into_iter().any(|item| matches!(item, HistoryEntry::ActivityFinished { activity, .. } if activity == "compacting its context"));
    assert!(recorded, "the channel records it where it happened");
    let stored = main.stored_agent_channel_state(helper).await.unwrap();
    let now: Vec<_> = stored.transcript.messages().into_iter().filter_map(|m| m.content).filter(|c| !c.is_empty()).collect();
    assert!(now[0].contains("They greeted each other.") && !now.iter().any(|c| c == "@helper hello" || c == "Hi."), "the older messages replaced: {now:?}");
    assert_eq!(stored.used.map(|u| u.calls), Some(4), "the summary's call counts as used");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

#[tokio::test]
async fn a_compaction_that_fails_ends_the_turn() {
    // The model writes no summary.
    let (world, session, main, helper, (told, ended)) = napping_with(vec![compaction()], vec![saying("")]).await;

    assert_eq!(told, ["busy".to_string(), "started compacting its context".to_string()]);
    assert_eq!(ended.err().as_deref(), Some("helper could not compact its context: its model wrote no summary"), "given up: the call it was to make room for would not fit");
    let stored = main.stored_agent_channel_state(helper).await.unwrap();
    assert!(stored.transcript.messages().iter().any(|m| m.content.as_deref() == Some("Hi.")), "the transcript as it was");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

#[tokio::test]
async fn an_agent_whose_personality_does_not_list_compaction_does_not_compact() {
    let (world, session, main, helper, (told, answer)) = napping_with(Vec::new(), vec![measured(300, saying("Done."))]).await;

    assert!(told.is_empty(), "nothing done of its own accord: {told:?}");
    assert!(answer.is_ok(), "the call made with the context as it is");
    let stored = main.stored_agent_channel_state(helper).await.unwrap();
    assert!(stored.transcript.messages().iter().any(|m| m.content.as_deref() == Some("Hi.")), "nothing replaced");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Compacting a shared agent's context is its owners' (issue 16, `owners_only` as for approving
/// its calls): bob, who may write in the channel, neither sees nor asks for `/compact` of alice's
/// `helper`; alice does, and the agent carries it out for her - here refusing, since it heard
/// nothing yet.
#[tokio::test]
async fn only_an_agents_owner_compacts_its_context() {
    let world = world(vec![Message::new(Classification::PUBLIC)]).await;
    let (session, main, _) = helper_with(&world, vec![compaction()]).await;
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let (refused, asked) = world.asked(main.user_actor(world.bob).unwrap().execute_command("compact", "@helper")).await;
    let refused = refused.expect_err("not his");
    assert!(matches!(refused.downcast_ref::<CommandError>(), Some(CommandError::Unknown(name)) if name == "compact"), "unknown to him: {refused}");
    assert!(about_agent_commands(&asked).iter().all(|line| line.ends_with(" no")), "never granted to him: {asked:?}");

    let mut hers = main.user_actor(world.alice).unwrap().watch();
    main.user_actor(world.alice).unwrap().execute_command("compact", "@helper").await.expect("hers, as its owner");
    assert!(matches!(shown(&mut hers, "CommandFailed").await, ActorEvent::CommandFailed { ref reason, .. } if reason.contains("nothing to compact")),
        "carried out for her by the agent, which has nothing to compact yet");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

