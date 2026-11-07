//! An agent's memory through the whole kernel (issue 518): the plugin and the library joined to the
//! world of the security suite - the ownership policy, recording what it is asked, and alice, bob
//! and carol (`caelumora_kernel::testing`). The tests of an action that acts in someone's name
//! assert what it asks, as the suite does for the kernel's.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::testing::{agent_runtime, calling, register_scripted};
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::{AgentId, Personality};
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::ledger::Change;
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::removal::Removal;
use caelumora_domain::turn::{StagedChange, StagedChanges};
use caelumora_kernel::session::SessionLoop;
use caelumora_kernel::testing::{participant_runtime_in, World};
use caelumora_memory::component::AGENT_MEMORY_KIND;
use caelumora_memory::extension::MemoryWriter;
use caelumora_memory::library::MemoryLibrary;
use caelumora_memory::memory::{AgentMemory, AgentMemoryId, Memory, MemoryChange, MemoryId};
use caelumora_memory::plugin::MemoryPlugin;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

/// The world with an agent's memory, whose model, for the agent of the personality "remembering",
/// remembers `text` once it is asked, then says it noted it.
async fn world_remembering(text: &str) -> World {
    let remember = calling("remember", serde_json::json!({ "text": text }));
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("noted".to_string()),
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    World::with(&[&MemoryPlugin], &[&AgentsLibrary, &MemoryLibrary], |registry| {
        register_scripted(registry, "remembering", vec![remember, answer]);
    })
    .await
}

/// alice's agent `helper`, which remembers with its model "remembering", in a session alice shares
/// with bob and he opened.
async fn with_a_rememberer(world: &World) -> (Arc<SessionLoop>, AgentId) {
    let mut remembering = Personality {
        components: ComponentConfig {
            component_nodes: vec![
                ComponentDescriptor::new("model", "remembering"),
                ComponentDescriptor::new("memory", AGENT_MEMORY_KIND),
            ],
        },
        ..world.personality("remembering", None, &[]).await
    };
    world
        .kernel
        .entity_stores()
        .personalities
        .update(&World::system(), &mut remembering)
        .await
        .unwrap();
    let helper = world.agent("helper", world.alice, &remembering).await;
    let session = world.shared_session(&[helper], &[]).await;
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (session, helper)
}

/// What `agent` remembers as it is stored, read as the system.
async fn remembered(world: &World, agent: AgentId) -> Vec<Memory> {
    let memories = world.entity_store::<AgentMemory>();
    match memories.get(&World::system(), AgentMemoryId::of(agent)).await {
        Ok(memory) => memory.chain.current(),
        Err(_) => Vec::new(),
    }
}

/// bob asks `helper` in the main channel of `session` to remember, and waits until its turn
/// committed what it remembered.
async fn ask_to_remember(world: &World, session: &SessionLoop, helper: AgentId) {
    let main = &session.channels()[0];
    main.user_actor(world.bob)
        .expect("he takes part")
        .send_message(Some("@helper remember this".to_string()), None, Vec::new())
        .await
        .expect("sent");
    time::timeout(Duration::from_secs(5), async {
        while remembered(world, helper).await.is_empty() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("remembered once the turn completes");
}

/// What an agent remembers the plugin commits in the agent's name - with its owner's rights,
/// asked by whoever asked it - where the agent reads from: the agent's session reached as the agent
/// reaches it (`GetSession`), and where the entry was made derived by the plugin.
#[tokio::test]
async fn what_an_agent_remembers_is_committed_in_its_owners_name_where_it_reads() {
    let world = world_remembering("bob likes tea").await;
    let (session, helper) = with_a_rememberer(&world).await;

    let (_, asked) = world.asked(ask_to_remember(&world, &session, helper)).await;
    let remembered = remembered(&world, helper).await;
    assert_eq!(
        (remembered[0].text.as_str(), remembered[0].origin.channel),
        ("bob likes tea", session.channels()[0].id()),
        "made where the agent reads from"
    );
    let helpers: Vec<&String> = asked.iter().filter(|line| line.contains("as helper")).collect();
    assert!(
        helpers.iter().all(|line| line.starts_with("alice as helper, asked by bob:")),
        "with her rights, asked by him: {asked:?}"
    );
    assert!(
        helpers.contains(&&"alice as helper, asked by bob: GetSession(alice's) yes".to_string()),
        "its session reached as the agent reaches it: {asked:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent's memory is its owner's, kept apart from the agent (issue 518): it goes with the agent
/// when the agent is purged, and not before.
#[tokio::test]
async fn an_agents_memory_goes_with_it_when_it_is_purged() {
    let world = world_remembering("bob likes tea").await;
    let (session, helper) = with_a_rememberer(&world).await;
    ask_to_remember(&world, &session, helper).await;
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let alice = World::as_user(world.alice);
    let agents = world.kernel.agent_manager();
    assert!(agents.delete(&alice, helper).await.unwrap());
    assert_eq!(remembered(&world, helper).await.len(), 1, "kept while the agent is deleted");
    let memories = world.entity_store::<AgentMemory>();
    let removal = Removal::now(World::system().caller_id);
    let his = World::as_user(world.bob);
    let deleted = memories.delete(&his, AgentMemoryId::of(helper), &removal).await.unwrap();
    assert!(!deleted, "not his to see, nor to delete");
    assert_eq!(remembered(&world, helper).await.len(), 1);

    assert!(agents.purge(&World::system(), helper).await.unwrap());
    assert!(remembered(&world, helper).await.is_empty(), "purged with it");
    let system = World::system();
    assert!(
        memories.get_deleted(&system, AgentMemoryId::of(helper)).await.is_err(),
        "nothing left of it"
    );
}

/// What reclassifying an entry of an agent's memory asks (issues 133, 518): the memory's verb of
/// its own, `Reclassify`, of the memory - its owner's, so the agent's for them -, asked as the tool
/// asks it and again as the turn's change is committed, with the memory read and changed in the
/// agent's name.
#[tokio::test]
async fn reclassifying_an_entry_asks_the_memorys_owner_as_the_tool_asks_and_as_it_is_committed() {
    let world = world_remembering("bob likes tea").await;
    let (session, helper) = with_a_rememberer(&world).await;
    ask_to_remember(&world, &session, helper).await;
    let main = &session.channels()[0];
    let runtime = agent_runtime(participant_runtime_in(main, ActorId::Agent(helper)));
    let bob = ActorId::User(world.bob);
    let classification = main.classification();
    let public = Classification::PUBLIC;
    let mut staged = StagedChanges::default();
    let reclassify = Change::Reclassify { id: MemoryId(1), classification: public.clone() };
    staged.push(&MemoryChange(reclassify)).unwrap();

    let (_, asked) = world
        .asked(async {
            let writer = runtime.writer::<dyn MemoryWriter>(bob, classification.clone()).await;
            assert!(writer.expect("its writer").may_reclassify(&public).await, "hers to release");
            let after = MessageId::new();
            let kind = MemoryChange::KIND;
            runtime.commit_staged(bob, classification, &staged, kind, after).await.expect("done");
        })
        .await;
    let reclassifying = [
        "alice as helper, asked by bob: GetSession(alice's) yes",
        "alice as helper, asked by bob: GetMemory(alice's) yes",
        "alice as helper, asked by bob: ReclassifyMemory(alice's) yes",
        "alice as helper, asked by bob: Publish yes",
    ];
    let storing = [
        "alice as helper, asked by bob: GetMemory(alice's) yes",
        "alice as helper, asked by bob: UpdateMemory(alice's) yes",
    ];
    let expected: Vec<&str> =
        reclassifying.iter().chain(&reclassifying).chain(&storing).copied().collect();
    assert_eq!(asked, expected, "as the tool asks, then as it is committed and stored");
    assert_eq!(remembered(&world, helper).await[0].classification, public);
}
