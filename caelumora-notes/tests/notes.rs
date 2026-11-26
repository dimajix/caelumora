//! Agents' notes through the whole kernel (issue 607): the plugin and the library joined to the
//! world of the security suite - the ownership policy, recording what it is asked, and alice, bob
//! and carol (`caelumora_kernel::testing`). The tests of an action that acts in someone's name
//! assert what it asks, as the suite does for the kernel's.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code: what fails here fails the test that runs it"
)]

use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::testing::{about_agent_commands, agent_runtime, calling, register_scripted};
use caelumora_api::command::CommandError;
use caelumora_api::reader::ViewError;
use caelumora_domain::actor::{ActorEvent, ActorId};
use caelumora_domain::agent::{AgentId, Personality};
use caelumora_domain::channel::ChannelId;
use caelumora_domain::command::CommandVisibility;
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::config::AppConfig;
use caelumora_domain::event::EntityChange;
use caelumora_domain::ledger::Change;
use caelumora_domain::message::{Message, MessageId, MessageRole};
use caelumora_domain::removal::Removal;
use caelumora_domain::turn::{StagedChange, StagedChanges};
use caelumora_kernel::channel::testing::as_command_error;
use caelumora_kernel::session::{ChannelSpec, SessionLoop};
use caelumora_kernel::testing::{
    participant_runtime_in, quiet, shown, Script, ScriptedAgents, World,
};
use caelumora_kernel::view::Viewer;
use caelumora_notes::command::AGENT_NOTES_KIND;
use caelumora_notes::component::notes::NOTES_KIND;
use caelumora_notes::component::private_notes::PRIVATE_NOTES_KIND;
use caelumora_notes::extension::{NotesReader, NotesWriter};
use caelumora_notes::library::NotesLibrary;
use caelumora_notes::note::{AgentNotes, AgentNotesId, NoteChange, NoteId, NoteKind};
use caelumora_notes::plugin::NotesPlugin;
use caelumora_notes::view::{AgentNotesBoard, SessionAgentNotes};
use caelumora_notes::NOTES_CHANGED;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

/// The world with agents' notes, whose model "noting" writes the note `text` once it is asked, then
/// says it noted it; with `/channel notes` among the commands of every session.
async fn world_noting(text: &str) -> World {
    let note = calling("write_note", serde_json::json!({ "text": text }));
    let answer = Message {
        role: MessageRole::Assistant,
        content: Some("noted".to_string()),
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    World::configured(
        &[&NotesPlugin],
        &[&AgentsLibrary, &NotesLibrary],
        |registry| register_scripted(registry, "noting", vec![note, answer]),
        with_agent_notes,
    )
    .await
}

/// `config` with `/channel notes` among the commands of every session.
fn with_agent_notes(config: &mut AppConfig) {
    let agent_notes = ComponentDescriptor::new("agent-notes", AGENT_NOTES_KIND);
    config.commands.component_nodes.push(agent_notes);
}

/// The personality `name` of the world, with the model `model` and the components of `kinds`.
async fn personality_with(world: &World, name: &str, model: &str, kinds: &[&str]) -> Personality {
    let mut nodes = vec![ComponentDescriptor::new("model", model)];
    nodes.extend(kinds.iter().map(|kind| ComponentDescriptor::new(kind, kind)));
    let mut personality = Personality {
        components: ComponentConfig { component_nodes: nodes },
        ..world.personality(name, None, &[]).await
    };
    world
        .kernel
        .entity_stores()
        .personalities
        .update(&World::system(), &mut personality)
        .await
        .unwrap();
    personality
}

/// alice's agent `helper`, which notes with its model "noting", in a session alice shares with bob
/// and he opened.
async fn with_a_noter(world: &World) -> (Arc<SessionLoop>, AgentId) {
    let noting = personality_with(world, "noting", "noting", &[NOTES_KIND]).await;
    let helper = world.agent("helper", world.alice, &noting).await;
    let session = world.shared_session(&[helper], &[]).await;
    world.kernel.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (session, helper)
}

/// The notes of `agent` in `channel` as they are stored, read as the system; none stored, empty.
async fn stored(world: &World, agent: AgentId, channel: ChannelId) -> AgentNotes {
    let notes = world.entity_store::<AgentNotes>();
    match notes.get(&World::system(), AgentNotesId::of(agent, channel)).await {
        Ok(notes) => notes,
        Err(_) => AgentNotes::new(agent, channel, ActorId::System),
    }
}

/// Notes `texts` as `agent`'s of `kind` in `channel`, each numbered by its chain, at the channel's
/// default classification, as its turns would have committed them; owned by alice.
async fn noted(world: &World, agent: AgentId, channel: ChannelId, kind: NoteKind, texts: &[&str]) {
    let store = world.entity_store::<AgentNotes>();
    let system = World::system();
    let found = store.get(&system, AgentNotesId::of(agent, channel)).await.ok();
    let made = found.is_none();
    let owner = ActorId::User(world.alice);
    let mut notes = found.unwrap_or_else(|| AgentNotes::new(agent, channel, owner));
    for text in texts {
        let chain = notes.chain_mut(kind);
        let id = chain.next_id();
        let classification = Classification::CHANNEL_DEFAULT;
        chain.push(
            Change::Add { id, text: (*text).to_string(), classification },
            (),
            MessageId::new(),
        );
    }
    if made {
        store.create(&system, &mut notes).await.unwrap();
    } else {
        store.update(&system, &mut notes).await.unwrap();
    }
}

/// What the notes of `channel` of `session` read as `user` sees them: each agent with the texts of
/// its notes.
async fn board(
    world: &World,
    user: ActorId,
    session: &SessionLoop,
    channel: ChannelId,
) -> Vec<(ActorId, Vec<String>)> {
    let ActorId::User(user) = user else { unreachable!("a user") };
    let view = world
        .kernel
        .views(Viewer::new(World::as_user(user)))
        .channel(session.id(), channel)
        .await
        .unwrap();
    let published = view.notes().await.unwrap();
    published
        .into_iter()
        .map(|p| (p.agent, p.notes.into_iter().map(|n| n.text).collect()))
        .collect()
}

/// bob asks `helper` in the main channel of `session` to note, and waits until its turn committed
/// what it noted.
async fn ask_to_note(world: &World, session: &SessionLoop, helper: AgentId) {
    let main = &session.channels()[0];
    main.user_actor(world.bob)
        .expect("he takes part")
        .send_message(None, Some("@helper note this".to_string()), None, Vec::new())
        .await
        .expect("sent");
    time::timeout(Duration::from_secs(5), async {
        while stored(world, helper, main.id()).await.public.current().is_empty() {
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("noted once the turn completes");
}

/// What an agent notes the plugin commits when its turn completes - in the agent's name, with its
/// owner's rights, asked by whoever asked it -, numbered in its chain, and announces it to whoever
/// reads the channel, who reads it on its board.
#[tokio::test]
async fn what_an_agent_notes_is_committed_with_its_turn_and_read_on_the_channels_board() {
    let world = world_noting("bob likes tea").await;
    let (session, helper) = with_a_noter(&world).await;
    let main = session.channels()[0].id();
    let mut events = world.kernel.event_bus().subscribe_all();

    let (_, asked) = world.asked(ask_to_note(&world, &session, helper)).await;
    let notes = stored(&world, helper, main).await;
    let standing = notes.public.current();
    assert_eq!((standing[0].id, standing[0].text.as_str()), (NoteId(1), "bob likes tea"));
    assert!(notes.private.current().is_empty(), "published, not kept for itself");
    let helpers: Vec<&String> = asked.iter().filter(|line| line.contains("as helper")).collect();
    assert!(
        helpers.iter().all(|line| line.starts_with("alice as helper, asked by bob:")),
        "with her rights, asked by him: {asked:?}"
    );
    let announced = events.drain().await.into_iter().any(|event| {
        matches!(&event.change, EntityChange::Custom { name, .. } if *name == NOTES_CHANGED)
    });
    assert!(announced, "the channel's readers are told its notes changed");
    assert_eq!(
        board(&world, ActorId::User(world.bob), &session, main).await,
        [(ActorId::Agent(helper), vec!["bob likes tea".to_string()])],
        "read as the channel's, by bob"
    );
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The board holds the notes of the agents taking part
/// ([581](../../issues/581-the-notes-of-an-agent-that-left-fall-off-the-board.md)): one that left
/// takes its notes along, one deleted takes them along while it rests in the channel and brings
/// them back restored; an agent whose personality is gone still counts.
#[tokio::test]
async fn an_agent_that_no_longer_takes_part_takes_its_notes_off_the_board() {
    let world =
        World::with(&[&NotesPlugin], &[&ScriptedAgents(Script::Answers("ok".to_string()))], |_| {})
            .await;
    let plain = world.personality("plain", Some(world.alice), &[]).await;
    let leaver = world.agent("leaver", world.alice, &plain).await;
    let rester = world.agent("rester", world.alice, &plain).await;
    let session = world.shared_session(&[leaver, rester], &[]).await;
    let main = session.channels()[0].id();
    noted(&world, leaver, main, NoteKind::Public, &["noted before it left"]).await;
    noted(&world, rester, main, NoteKind::Public, &["noted by the rester"]).await;
    let alice = ActorId::User(world.alice);
    let agents = |board: Vec<(ActorId, Vec<String>)>| -> Vec<ActorId> {
        board.into_iter().filter(|(_, notes)| !notes.is_empty()).map(|(agent, _)| agent).collect()
    };
    let both = [ActorId::Agent(leaver), ActorId::Agent(rester)];
    assert_eq!(agents(board(&world, alice, &session, main).await), both);

    let system = World::system();
    let personalities = &world.kernel.entity_stores().personalities;
    assert!(personalities.delete(&system, plain.id, &Removal::now(alice)).await.unwrap());
    assert_eq!(agents(board(&world, alice, &session, main).await), both, "its personality gone");

    let hers = World::as_user(world.alice);
    session.channels()[0].remove_participant(&hers, ActorId::Agent(leaver)).await.unwrap();
    assert_eq!(
        agents(board(&world, alice, &session, main).await),
        [ActorId::Agent(rester)],
        "the leaver took its notes along"
    );
    assert_eq!(
        stored(&world, leaver, main).await.public.current()[0].text,
        "noted before it left",
        "kept for its return"
    );
    let stores = world.kernel.entity_stores();
    assert!(stores.agents.delete(&system, rester, &Removal::now(alice)).await.unwrap());
    assert!(
        agents(board(&world, alice, &session, main).await).is_empty(),
        "resting, the rester too"
    );
    stores.agents.restore(&system, rester).await.unwrap();
    assert_eq!(
        agents(board(&world, alice, &session, main).await),
        [ActorId::Agent(rester)],
        "and brings them back restored"
    );
    world.kernel.stop_session(&hers, session.id()).await.unwrap();
}

/// A channel's board read entry by entry - by an agent reading from a channel less classified -
/// holds the notes released to where it reads from; where none is, it answers as a channel that
/// does not exist, so that nothing can be probed.
#[tokio::test]
async fn a_board_read_entry_by_entry_holds_what_was_released_and_else_answers_as_absent() {
    let world = World::with(&[&NotesPlugin], &[&AgentsLibrary], |_| {}).await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let hers = World::as_user(world.alice);
    let main = session.channels()[0].id();
    let participants = vec![ActorId::User(world.alice), ActorId::Agent(helper)];
    let spec = |name: &str, classification| ChannelSpec {
        participants: participants.clone(),
        components: quiet(),
        classification,
        ..ChannelSpec::new(name)
    };
    let open = session.create_channel(&hers, spec("open", Classification::PUBLIC)).await.unwrap();
    let closed = session.create_channel(&hers, spec("closed", Classification::CHANNEL_DEFAULT));
    let closed = closed.await.unwrap().id();
    noted(&world, helper, main, NoteKind::Public, &["noted internally"]).await;
    released(&world, helper, main, "noted openly").await;
    noted(&world, helper, closed, NoteKind::Public, &["noted in closed"]).await;

    let runtime = agent_runtime(participant_runtime_in(&open, ActorId::Agent(helper)));
    let alice = ActorId::User(world.alice);
    let reader = runtime.reader::<dyn NotesReader>(alice, open.classification()).await.unwrap();
    let read = reader.published_in(main).await.expect("released there");
    let texts: Vec<&str> = read[0].notes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(texts, ["noted openly"], "what was released alone");
    assert!(
        matches!(reader.published_in(closed).await, Err(ViewError::NoChannelNamed(_))),
        "nothing released: as if it did not exist"
    );
    world.kernel.stop_session(&hers, session.id()).await.unwrap();
}

/// Adds `text`, released to everyone (`Classification::PUBLIC`), to the notes `agent` published in
/// `channel`, which it noted before.
async fn released(world: &World, agent: AgentId, channel: ChannelId, text: &str) {
    let store = world.entity_store::<AgentNotes>();
    let system = World::system();
    let mut notes = store.get(&system, AgentNotesId::of(agent, channel)).await.unwrap();
    let id = notes.public.next_id();
    let classification = Classification::PUBLIC;
    let added = Change::Add { id, text: text.to_string(), classification };
    notes.public.push(added, (), MessageId::new());
    store.update(&system, &mut notes).await.unwrap();
}

/// An agent's private notes across the channels of a session (issue 233) are read as what it saw:
/// by its owner, by the channels' names and without one noting nothing; not by another
/// participant.
#[tokio::test]
async fn an_agents_private_notes_are_read_from_every_channel_of_the_session_by_its_owner() {
    let world = World::with(&[&NotesPlugin], &[&AgentsLibrary], |_| {}).await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let hers = World::as_user(world.alice);
    let participants = vec![ActorId::User(world.alice), ActorId::Agent(helper)];
    let channel_of = async |name: &str| {
        let spec = ChannelSpec {
            participants: participants.clone(),
            components: quiet(),
            ..ChannelSpec::new(name)
        };
        session.create_channel(&hers, spec).await.expect("hers to make").id()
    };
    let zeta = channel_of("zeta").await;
    let _quiet = channel_of("quiet").await;
    let alpha = channel_of("alpha").await;
    noted(&world, helper, zeta, NoteKind::Private, &["noted in zeta"]).await;
    noted(&world, helper, alpha, NoteKind::Private, &["noted in alpha"]).await;
    let read = |user| {
        let world = &world;
        let session = session.id();
        async move {
            world
                .kernel
                .views(Viewer::new(World::as_user(user)))
                .session(session)
                .await
                .unwrap()
                .private_notes_of(helper)
                .await
        }
    };

    let found = read(world.alice).await.expect("alice made the agent");
    let found: Vec<_> = found
        .iter()
        .map(|found| (found.channel.name().as_str(), found.notes[0].text.as_str()))
        .collect();
    assert_eq!(found, [("alpha", "noted in alpha"), ("zeta", "noted in zeta")]);
    assert!(
        matches!(read(world.bob).await, Err(ViewError::Forbidden { .. })),
        "not by bob, who takes part"
    );
    world.kernel.stop_session(&hers, session.id()).await.unwrap();
}

/// An agent reads its private notes of the session's other channels through itself, as far as they
/// may be copied to where it reads from - also of a channel it left -, and its own chains of the
/// channel it reads from, with the numbers given (`NotesReader`).
#[tokio::test]
async fn an_agent_reads_its_own_notes_and_its_private_notes_of_other_channels_through_itself() {
    let world = World::with(&[&NotesPlugin], &[&AgentsLibrary], |_| {}).await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let hers = World::as_user(world.alice);
    let main = Arc::clone(&session.channels()[0]);
    let participants = vec![ActorId::User(world.alice), ActorId::Agent(helper)];
    let spec = ChannelSpec { participants, components: quiet(), ..ChannelSpec::new("side") };
    let side = session.create_channel(&hers, spec).await.expect("hers to make");
    noted(&world, helper, side.id(), NoteKind::Private, &["noted aside"]).await;
    noted(&world, helper, main.id(), NoteKind::Public, &["first", "second"]).await;
    side.remove_participant(&hers, ActorId::Agent(helper)).await.unwrap();

    let runtime = agent_runtime(participant_runtime_in(&main, ActorId::Agent(helper)));
    let alice = ActorId::User(world.alice);
    let reader = runtime.reader::<dyn NotesReader>(alice, main.classification()).await;
    let reader = reader.expect("its notes are read here");
    let elsewhere = reader.own_private_elsewhere().await;
    assert_eq!(
        elsewhere.iter().map(|c| (c.name.as_str(), c.notes[0].text.as_str())).collect::<Vec<_>>(),
        [("side", "noted aside")],
        "of a channel it left too"
    );
    let own = reader.own(NoteKind::Public);
    assert_eq!(own.current().len(), 2);
    assert_eq!(own.next_id(), NoteId(3), "the next number its own chain gives");
    world.kernel.stop_session(&hers, session.id()).await.unwrap();
}

/// What a turn staged of its notes the plugin checks as it commits it, all or none: a new note of a
/// number given or never reserved, or a change of a note that does not stand, is refused;
/// reclassifying asks the policy for the note's author, as the tool asks it.
#[tokio::test]
async fn a_change_of_notes_not_the_turns_to_make_is_refused_as_it_is_committed() {
    let world = world_noting("bob likes tea").await;
    let (session, helper) = with_a_noter(&world).await;
    ask_to_note(&world, &session, helper).await;
    let main = &session.channels()[0];
    let runtime = agent_runtime(participant_runtime_in(main, ActorId::Agent(helper)));
    let bob = ActorId::User(world.bob);
    let classification = main.classification();
    let commit = |change: Change<NoteId>| {
        let runtime = &runtime;
        let classification = classification.clone();
        async move {
            let mut staged = StagedChanges::default();
            staged.push(&NoteChange { kind: NoteKind::Public, change }).unwrap();
            let kind = NoteChange::KIND;
            runtime.commit_staged(bob, classification, &staged, kind, MessageId::new()).await
        }
    };
    let text = |id| Change::Add {
        id: NoteId(id),
        text: "again".to_string(),
        classification: Classification::CHANNEL_DEFAULT,
    };

    let writer = runtime.writer::<dyn NotesWriter>(bob, classification.clone()).await;
    let writer = writer.expect("its writer");
    let fresh = writer.reserve_id(NoteKind::Public).await.unwrap();
    assert_eq!(fresh, NoteId(2), "counted on from the notes as stored");
    commit(text(1)).await.expect_err("number 1 was given");
    commit(text(9)).await.expect_err("number 9 was never reserved");
    commit(Change::Remove { id: NoteId(7) }).await.expect_err("there is no note 7");
    let update = |id| Change::Update {
        id: NoteId(id),
        text: "changed".to_string(),
        classification: Classification::CHANNEL_DEFAULT,
    };
    commit(update(7)).await.expect_err("nor to update");
    let reclassify = Change::Reclassify { id: NoteId(7), classification: Classification::PUBLIC };
    commit(reclassify).await.expect_err("nor to reclassify");
    let above = Change::Add {
        id: fresh,
        text: "secret".to_string(),
        classification: Classification::general(Confidentiality::Confidential),
    };
    commit(above).await.expect_err("classified above the work");
    let mut both = StagedChanges::default();
    for change in [text(2), text(2)] {
        both.push(&NoteChange { kind: NoteKind::Public, change }).unwrap();
    }
    let twice = runtime.commit_staged(
        bob,
        classification.clone(),
        &both,
        NoteChange::KIND,
        MessageId::new(),
    );
    twice.await.expect_err("number 2 added twice, and the good first change not kept either");
    let standing = stored(&world, helper, main.id()).await.public.current();
    assert_eq!(standing.len(), 1, "nothing of it: all or none");
    assert_eq!(standing[0].text, "bob likes tea");
    commit(text(2)).await.expect("a number reserved and never given");
    let (allowed, asked) =
        world.asked(async { writer.may_reclassify(&Classification::PUBLIC).await }).await;
    assert!(allowed, "the author's to release");
    assert!(asked.iter().any(|line| line.contains("Reclassify")), "asked of the policy: {asked:?}");
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent's notes go with it when it is purged, and those noted in a channel with the channel
/// when its session is purged (`KeptBehind`).
#[tokio::test]
async fn notes_go_with_their_agent_and_with_their_channel_when_purged() {
    let world = World::with(&[&NotesPlugin], &[&AgentsLibrary], |_| {}).await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let other = world.agent("other", world.alice, &global).await;
    let session = world.shared_session(&[helper, other], &[]).await;
    let main = session.channels()[0].id();
    noted(&world, helper, main, NoteKind::Public, &["of the helper"]).await;
    noted(&world, other, main, NoteKind::Private, &["of the other"]).await;
    let hers = World::as_user(world.alice);
    world.kernel.stop_session(&hers, session.id()).await.unwrap();
    let agents = world.kernel.agent_manager();
    let system = World::system();

    assert!(agents.delete(&hers, helper).await.unwrap());
    assert!(!stored(&world, helper, main).await.public.current().is_empty(), "kept while deleted");
    assert!(agents.purge(&system, helper).await.unwrap());
    assert!(stored(&world, helper, main).await.public.current().is_empty(), "purged with it");

    assert!(world.kernel.delete_session(&hers, session.id()).await.unwrap());
    assert!(!stored(&world, other, main).await.private.current().is_empty(), "kept while deleted");
    assert!(world.kernel.purge_session(&system, session.id()).await.unwrap());
    assert!(stored(&world, other, main).await.private.current().is_empty(), "purged with it");
    let notes = world.entity_store::<AgentNotes>();
    assert!(notes.get_deleted(&system, AgentNotesId::of(other, main)).await.is_err());
}

/// `/channel notes` (issue 574) shows what the agents noted in a channel, read as the channel's, by
/// their authors, each note with its number, an agent that noted nothing left out; in a channel
/// where nothing is noted, that nothing is.
#[tokio::test]
async fn channel_notes_shows_what_the_agents_noted_by_their_authors() {
    // Agents that run, so that they take part.
    let answering = ScriptedAgents(Script::Answers("ok".to_string()));
    let world = World::configured(&[&NotesPlugin], &[&answering], |_| {}, with_agent_notes).await;
    let plain = world.personality("plain", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &plain).await;
    let scout = world.agent("scout", world.alice, &plain).await;
    let runner = world.agent("runner", world.alice, &plain).await;
    let session = world.shared_session(&[helper, scout, runner], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let notes = ["The build needs Rust 1.80.", "Tests run with nextest."];
    noted(&world, helper, main.id(), NoteKind::Public, &notes).await;
    noted(&world, runner, main.id(), NoteKind::Public, &["Deploys on Fridays are off."]).await;
    noted(&world, scout, main.id(), NoteKind::Private, &["Its own."]).await;
    let alice = main.user_actor(world.alice).unwrap();

    let reply = alice.execute_command(None, "channel notes").await.expect("hers to read");
    assert_eq!(reply.visibility, CommandVisibility::Private, "answered to her alone");
    let board = "Noted in main:\n\n\
                 helper:\n\
                 - #1 The build needs Rust 1.80.\n\
                 - #2 Tests run with nextest.\n\n\
                 runner:\n\
                 - #1 Deploys on Fridays are off.";
    assert_eq!(reply.output.as_deref(), Some(board), "scout, who published nothing, left out");
    let named = alice.execute_command(None, "channel notes main").await.unwrap().output.unwrap();
    assert_eq!(named, board, "the channel named, the same");
    let side = ChannelSpec { components: quiet(), ..ChannelSpec::new("side") };
    session.create_channel(&World::as_user(world.alice), side).await.expect("hers to make");
    let other = alice.execute_command(None, "channel notes side").await.unwrap().output.unwrap();
    assert_eq!(other, "Nothing is noted in side.", "another channel named, not this one");
    let nowhere = alice.execute_command(None, "channel notes nowhere").await.expect_err("none");
    assert!(
        matches!(as_command_error(&nowhere), CommandError::InvalidArguments { .. }),
        "a channel that names nothing: {nowhere}"
    );
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// `/channel notes` (issue 574) asks nothing reading the channel does not ask: bob, who reads
/// alice's channel, is shown what her agent noted in it, as the web UI shows it him; of a channel
/// he may not read, he is told what he is told of one that does not exist.
#[tokio::test]
async fn channel_notes_asks_what_reading_the_channel_asks() {
    let answering = ScriptedAgents(Script::Answers("ok".to_string()));
    let world = World::configured(&[&NotesPlugin], &[&answering], |_| {}, with_agent_notes).await;
    let hers = world.personality("hers", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &hers).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    noted(&world, helper, main.id(), NoteKind::Public, &["noted for all"]).await;
    world.kernel.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let private = ChannelSpec { components: quiet(), ..ChannelSpec::new("private") };
    session.create_channel(&World::as_user(world.alice), private).await.expect("hers to make");
    let bob = main.user_actor(world.bob).unwrap();

    for line in ["channel notes", "channel notes main"] {
        let (reply, asked) = world.asked(bob.execute_command(None, line)).await;
        let told = reply.expect("his to read").output.expect("answered to him");
        assert!(told.contains("helper:\n- #1 noted for all"), "/{line}: {told}");
        assert_eq!(
            asked,
            [
                "bob: ShowCommand(alice's) yes",
                "bob: ExecuteCommand(alice's) yes",
                // Reading the channel, as its view does; the notes read then ask nothing more.
                "bob: GetSession(alice's) yes",
                "bob: GetChannel(alice's) yes",
                "bob: Cleared(bob) yes",
            ],
            "/{line}: {asked:#?}"
        );
    }
    let refused = |name: &'static str| {
        let bob = &bob;
        async move {
            let refused = bob.execute_command(None, &format!("channel notes {name}")).await;
            match as_command_error(&refused.expect_err("unusable")) {
                CommandError::InvalidArguments { reason, .. } => reason.replace(name, "_"),
                other => panic!("refused as invalid, not {other:?}"),
            }
        }
    };
    assert_eq!(
        refused("private").await,
        refused("nowhere").await,
        "one he may not read, as one that does not exist"
    );
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// `/private-notes` (issue 574) shows an agent's private notes to its owner alone, as what it saw:
/// bob, who may write in the channel, neither sees nor asks for it; alice does, and the agent
/// answers her alone with what it noted for itself.
#[tokio::test]
async fn an_agents_private_notes_are_shown_by_command_to_its_owner_alone() {
    let world = World::with(&[&NotesPlugin], &[&AgentsLibrary, &NotesLibrary], |registry| {
        register_scripted(registry, "silent", vec![Message::new(Classification::PUBLIC)]);
    })
    .await;
    let noting = personality_with(&world, "noting", "silent", &[PRIVATE_NOTES_KIND]).await;
    let helper = world.agent("helper", world.alice, &noting).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    noted(&world, helper, main.id(), NoteKind::Private, &["secret"]).await;
    world.kernel.open_session(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let bob = main.user_actor(world.bob).unwrap();
    let (refused, asked) = world.asked(bob.execute_command(None, "private-notes @helper")).await;
    let refused = refused.expect_err("not his");
    assert!(
        matches!(as_command_error(&refused), CommandError::Unknown(name) if name == "private-notes"),
        "unknown to him: {refused}"
    );
    assert_eq!(
        about_agent_commands(&asked),
        ["bob: ShowAgentCommand(alice's) no"],
        "never granted to him"
    );

    let alice = main.user_actor(world.alice).unwrap();
    let mut hers = alice.watch();
    // It takes no arguments, which the agent refuses before it begins (`check_arguments`).
    alice.execute_command(None, "private-notes @helper now").await.expect("hers to ask for");
    let refused = shown(&mut hers, "CommandFailed").await;
    assert!(
        matches!(
            refused,
            ActorEvent::CommandFailed { ref reason, turn_id: None, .. }
                if reason.contains("it takes no arguments")
        ),
        "refused, never begun: {refused:?}"
    );
    alice.execute_command(None, "private-notes @helper").await.expect("hers, as its owner");
    let answer = shown(&mut hers, "CommandExecuted").await;
    assert!(
        matches!(
            answer,
            ActorEvent::CommandExecuted { ref output, ref to, .. }
                if output.ends_with("In this channel:\n- #1 secret")
                    && *to == Some(ActorId::User(world.alice))
        ),
        "carried out for her alone: {answer:?}"
    );
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}
