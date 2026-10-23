//! The tools of the workspace through the whole runtime (issues 251, 264,
//! 267, 290, 428): the plugin joined to the world of the security suite -
//! the ownership policy, recording what it is asked, and alice, bob and
//! carol (`caelumora_runtime::testing`) - with the agents
//! (`caelumora_agents::testing`). The tests of an action that acts in
//! someone's name assert what it asks, as the suite does for the core's.

use caelumora_agents::testing::{asked_as_a_call_on_the_workspace, asked_for_a_call, calling, register_once, TestAgents, ONCE_MODEL_KIND};
use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::agent::Personality;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::scope::Scope;
use caelumora_model::security::Grant;
use caelumora_model::tool::ToolResultStatus;
use caelumora_model::user::UserId;
use caelumora_runtime::testing::World;
use caelumora_runtime::view::Viewer;
use caelumora_workspace_tools::plugin::WorkspaceToolsPlugin;
use caelumora_workspace_tools::terminal_tool::TERMINAL_TOOL_KIND;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

/// What is asked when the agent of `owner`, asked by bob, makes `call` in the
/// session's workspace, where the files `present` lie already - one that
/// leaves `made`, a file of the workspace.
async fn asked_for_a_call_on_the_workspace(function: &str, arguments: Value, present: &[&str], made: &str, owner: fn(&World) -> UserId) -> Vec<String> {
    let world = World::with(&[&TestAgents, &WorkspaceToolsPlugin], |registry| register_once(registry, ONCE_MODEL_KIND, calling(function, arguments))).await;
    let kind = TERMINAL_TOOL_KIND;
    asked_for_a_call(&world, kind, present, |_, place| place.join(made).is_file(), owner(&world)).await
}

/// An agent's command (issue 251).
#[tokio::test]
async fn an_agents_command_is_called_for_its_owner_and_reaches_the_workspace_through_its_channel() {
    let asked = asked_for_a_call_on_the_workspace("run_command", serde_json::json!({ "command": "echo ran > ran.txt" }), &[], "ran.txt", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: WriteWorkspace(alice's) yes".to_string()), "a command may change anything: {asked:?}");
}

/// An agent's change of a file (issue 251, step 9) asks what a command asks.
#[tokio::test]
async fn an_agents_file_tool_asks_what_a_command_asks() {
    let asked = asked_for_a_call_on_the_workspace("write_file", serde_json::json!({ "path": "notes/a.md", "content": "written" }), &[], "notes/a.md", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: WriteWorkspace(alice's) yes".to_string()), "a change of its files: {asked:?}");
}

/// An agent's move of a file (issue 267) asks what any change of a file
/// asks.
#[tokio::test]
async fn an_agents_move_of_a_file_asks_what_a_change_asks() {
    let asked = asked_for_a_call_on_the_workspace("move_file", serde_json::json!({ "from": "notes/a.md", "to": "archive/a.md" }), &["notes/a.md"], "archive/a.md", |world| world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
}

/// An agent reaches the workspace of a session its owner does not take
/// part in (issue 264): carol's agent, in alice's session with bob, reaches
/// the terminal through the session it takes part in itself - read as
/// carol, by the agent's own taking part.
#[tokio::test]
async fn an_agent_reaches_the_workspace_of_a_session_its_owner_does_not_take_part_in() {
    let asked = asked_for_a_call_on_the_workspace("run_command", serde_json::json!({ "command": "echo ran > ran.txt" }), &[], "ran.txt", |world| world.carol).await;
    assert!(asked.contains(&"carol as helper, asked by bob: GetSession(alice's) yes".to_string()), "the session it takes part in: {asked:?}");
}

/// What an agent is offered of a workspace (issue 290): the tools its list
/// lets the agent call in its channel - in a turn, as its owner's - those
/// that only read files where it grants reading, a
/// call of another refused as unknown before anything is asked of it; every
/// one where it grants changing; none where the channel uses no workspace,
/// saying why. A call of one offered is checked again as it runs (the
/// tool's own tests, and the runtime's suite for what the list grants).
#[tokio::test]
async fn an_agent_is_offered_the_workspace_tools_its_list_lets_it_call_in_its_channel() {
    let write = calling("write_file", serde_json::json!({ "path": "notes/a.md", "content": "written" }));
    let world = World::with(&[&TestAgents, &WorkspaceToolsPlugin], |registry| register_once(registry, ONCE_MODEL_KIND, write)).await;
    let working = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", ONCE_MODEL_KIND),
            ComponentDescriptor { params: serde_json::json!({ "requires_approval": false }), ..ComponentDescriptor::new("workspace", TERMINAL_TOOL_KIND) },
        ] },
        ..world.personality("working", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &working).await.unwrap();
    let helper = world.agent("helper", world.bob, &working).await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "shell", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[helper], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Read)).await.unwrap();
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&bob, session.id()).await.expect("he opens it");
    let mut watch = main.user_actor(world.bob).expect("he takes part").watch();

    let (answered, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper write it".to_string(), None).await.expect("sent");
        time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::CompleteMessageGenerated(message) = watch.recv().await.expect("the channel runs").event {
                    if let Some(result) = message.tool_results.first() {
                        break (result.status, result.content.clone());
                    }
                }
            }
        }).await.expect("the call is answered")
    }).await;
    let (status, content) = answered;
    assert_eq!(status, ToolResultStatus::UnknownTool, "not offered where the list grants reading: {content}");
    assert!(content.contains("read_file") && !content.contains("run_command"), "it names what it was offered: {content}");
    assert!(asked.contains(&"bob as helper: WriteWorkspace(alice's) no".to_string()), "asked as the turn began, as its owner's: {asked:?}");
    assert!(!asked.iter().any(|line| line.contains("CallTool")), "refused before anything is asked of the call: {asked:?}");
    assert!(!world.files.path().join("shell").join(docs.id.to_string()).join("notes/a.md").exists(), "nothing written");

    let offered = || async {
        let view = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        let sets = view.participant(ActorId::Agent(helper)).await.unwrap().tool_sets().await.unwrap();
        let mut names: Vec<String> = sets[0].tools.iter().map(|tool| tool.name.clone()).collect();
        names.sort();
        (names, sets[0].unavailable.clone())
    };
    assert_eq!(offered().await, (["grep_files", "list_files", "publish_file", "read_file"].map(String::from).to_vec(), None), "what only reads");
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (changing, unavailable) = offered().await;
    assert_eq!((changing.len(), unavailable), (11, None), "every one where it grants changing: {changing:?}");
    main.update_uses_workspace(&alice, false).await.unwrap();
    assert_eq!(offered().await, (Vec::new(), Some("there is no workspace you can use in this channel".to_string())), "none where the channel uses none");
    world.manager.stop(&alice, session.id()).await.unwrap();
}
