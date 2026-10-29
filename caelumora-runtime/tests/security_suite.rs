//! The security suite (issue 198): users with different permissions doing
//! different things, through the whole runtime - the stores checking with
//! the ownership policy, the `SessionManager`, running channels and the
//! views - as a frontend would. What the system starts from is put into the
//! store as the system, as a setup would; everything a user does goes
//! through the public API, in that user's name.
//!
//! The policy records every question it is asked (`Recorder`): a test of
//! an action asserts the questions asked in a user's name, in order - which
//! permissions the action needs. Each test names the scenario or action of
//! issue 198 it checks; the issue says what is expected and why.

use caelumora_model::removal::Removal;
use caelumora_model::store::query::Query;
use std::sync::Arc;
use std::time::Duration;

use caelumora_model::actor::{ActorId, Rejection};
use caelumora_model::agent::{Agent, AgentId, Personality};
use caelumora_model::approval::{ApprovalRule, Reach, RuleDecision, ToolMatcher};
use caelumora_model::channel::{Channel, HistoryAccess, LEFT};
use caelumora_model::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId, GroupUpdateEvent};
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::project::{Project, ProjectId, ProjectUpdateEvent};
use caelumora_model::role::RoleId;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, SecurityAttributes, SecurityContext, Subject};
use caelumora_model::session::testing::make_session;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::snapshot::Detail;
use caelumora_model::store::{EntityKind, StoreError, Stored};
use caelumora_model::user::testing::make_user;
use caelumora_model::user::{UserId, UserProfile};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::{Description, Introduction};
use caelumora_model::workspace::{CommandStatus, Workspace, WorkspaceId};
use caelumora_runtime::actor::Shown;
use caelumora_runtime::agent::AgentError;
use caelumora_runtime::channel::ChannelError;
use caelumora_runtime::component::{ComponentFacets, ComponentKind, BUNDLE_REF_KIND};
use caelumora_runtime::event::{EventSubscription, SubscriptionTopic};
use caelumora_runtime::group::GroupError;
use caelumora_runtime::moderator::broadcast_policy::BroadcastPolicy;
use caelumora_runtime::moderator::ResponderPolicy;
use caelumora_runtime::project::ProjectError;
use caelumora_runtime::scoped::ScopeNotEmpty;
use caelumora_runtime::session::testing::create;
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop, SessionSpec};
use caelumora_runtime::terminal::{CommandState, TerminalError, WorkspacePath};
use caelumora_runtime::testing::{
    attributes, checks, denied, file, not_found, quiet, read, shown, workspace, World, STAFF,
};
use caelumora_runtime::view::{ViewError, Viewer};
use caelumora_runtime::workspace::testing::{shell_in, terminal_in};
use caelumora_runtime::workspace::WorkspaceError;
use serde_json::json;
use tokio::time;

// --- Personalities and agents ---------------------------------------------

/// P1: a personality the user may not read makes no agent and no session.
#[tokio::test]
async fn a_session_cannot_be_made_with_a_personality_the_user_may_not_read() {
    let world = World::new().await;
    let bobs = world.personality("bobs-own", Some(world.bob), &[]).await;
    let template = world.template(Some(world.alice), &[], "helper", bobs.id).await;

    let refused = world
        .manager
        .create_from_template(&World::as_user(world.alice), template, SessionSpec::default())
        .await
        .expect_err("not hers to use");
    assert!(not_found(&refused), "answered as not there, so that it cannot be probed: {refused:#}");
    assert!(
        world
            .manager
            .views(Viewer::new(World::as_user(world.alice)))
            .sessions()
            .await
            .unwrap()
            .is_empty(),
        "no session was stored"
    );
    assert!(
        world
            .runtime
            .entity_stores()
            .agents
            .list(&World::system(), &Query::all())
            .await
            .map(|page| page.items)
            .unwrap()
            .is_empty(),
        "nor an agent"
    );
}

/// P4: such an agent cannot be added to a channel either.
#[tokio::test]
async fn an_agent_whose_personality_was_withdrawn_cannot_join_a_channel() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let main = Arc::clone(&session.channels()[0]);

    let mut withdrawn =
        Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world
        .runtime
        .entity_stores()
        .personalities
        .update(&World::as_user(world.bob), &mut withdrawn)
        .await
        .unwrap();

    let refused = main
        .add_participant(&World::as_user(world.alice), ActorId::Agent(helper), HistoryAccess::All)
        .await
        .expect_err("it may not run");
    assert!(denied(&refused), "{refused:#}");
    assert!(!main.participant_ids().contains(&ActorId::Agent(helper)), "the channel is as it was");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P5: a personality is changed and deleted by its owner only; who it is
/// shared with reads it, a stranger does not even find it.
#[tokio::test]
async fn a_personality_is_changed_by_its_owner_and_read_by_who_it_is_shared_with() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let alice = World::as_user(world.alice);
    let bob = World::as_user(world.bob);
    let carol = World::as_user(world.carol);

    assert!(
        world.runtime.entity_stores().personalities.get(&alice, shared.id).await.is_ok(),
        "shared with her"
    );
    let mut changed =
        Personality { description: Some(Description::new("hers now").unwrap()), ..shared.clone() };
    assert!(
        world.runtime.entity_stores().personalities.update(&alice, &mut changed).await.is_err(),
        "but not hers to change"
    );
    assert!(
        world
            .runtime
            .entity_stores()
            .personalities
            .delete(&alice, shared.id, &Removal::now(alice.caller_id))
            .await
            .is_err(),
        "nor to delete"
    );
    assert!(
        matches!(
            world.runtime.entity_stores().personalities.get(&carol, shared.id).await,
            Err(StoreError::NotFound { .. })
        ),
        "not there for a stranger"
    );
    assert!(
        world
            .runtime
            .entity_stores()
            .personalities
            .list(&carol, &Query::all())
            .await
            .map(|page| page.items)
            .unwrap()
            .iter()
            .all(|p| p.id != shared.id),
        "nor listed"
    );
    world
        .runtime
        .entity_stores()
        .personalities
        .update(&bob, &mut changed)
        .await
        .expect("its owner changes it");
}

// --- Sessions and channels --------------------------------------------------

/// S1: a stranger finds nothing of a session - not that it exists.
#[tokio::test]
async fn a_stranger_finds_nothing_of_a_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let carol = World::as_user(world.carol);

    assert!(not_found(
        &world.manager.open(&carol, session.id()).await.expect_err("not hers to open")
    ));
    assert!(not_found(&world.manager.stop(&carol, session.id()).await.expect_err("nor to stop")));
    assert!(not_found(
        &world.manager.delete(&carol, session.id()).await.expect_err("nor to delete")
    ));
    let views = world.manager.views(Viewer::new(carol));
    assert!(matches!(views.session(session.id()).await, Err(ViewError::SessionNotFound(_))));
    assert!(views.sessions().await.unwrap().is_empty(), "nor listed");
    assert!(
        world
            .manager
            .views(Viewer::new(World::as_user(world.alice)))
            .sessions()
            .await
            .unwrap()
            .len()
            == 1,
        "while it is there"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 23: a channel is read by who takes part in it now - wherever they
/// act -, not by a list copied when it was made: a participant removed is
/// told of it, and reads it no more.
#[tokio::test]
async fn a_channel_is_read_by_its_participants_wherever_they_act_and_no_more_once_removed() {
    let world = World::new().await;
    let session = world.session_of(&[world.bob, world.carol], &[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let elsewhere =
        SecurityContext::new(ActorId::User(world.carol), SessionId::new(), ChannelId::new())
            .with_roles(vec![*STAFF], None);
    assert!(
        session.channel_for(&elsewhere, main.id()).await.is_ok(),
        "a member, though she acts elsewhere"
    );

    let mut events = world
        .runtime
        .event_bus()
        .subscribe(World::as_user(world.carol), vec![SubscriptionTopic::All]);
    main.remove_participant(&World::as_user(world.alice), ActorId::User(world.carol))
        .await
        .expect("alice's to remove her");
    time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            if matches!(event.about::<Channel>(), Some(((_, channel), change)) if channel == main.id() && change.data::<ActorId>(LEFT) == Some(ActorId::User(world.carol))) {
                return;
            }
        }
    }).await.expect("she is told she was removed");
    assert!(
        session.channel_for(&World::as_user(world.carol), main.id()).await.is_err(),
        "and reads it no more"
    );
    assert!(
        session.channel_for(&World::as_user(world.bob), main.id()).await.is_ok(),
        "while the others still do"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// S2: a participant reads and takes part, but changes nothing of the
/// session or its channels.
#[tokio::test]
async fn a_participant_takes_part_but_changes_nothing() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let bob = World::as_user(world.bob);

    world.manager.open(&bob, session.id()).await.expect("he opens it");
    assert!(
        world
            .participants(world.bob, session.id(), main.id())
            .await
            .contains(&ActorId::User(world.alice)),
        "and sees who is in it"
    );
    main.user_actor(world.bob)
        .expect("he takes part")
        .send_message(Some("hello".to_string()), None, Vec::new())
        .await
        .expect("and writes into it");
    main.cancel(&bob).await.expect("and may cancel what runs in it");

    assert!(denied(&session.update_name(&bob, "his").await.expect_err("not his to rename")));
    assert!(
        session.update_privacy(&bob, PrivacyConfig::default()).await.is_err(),
        "nor to change its privacy"
    );
    assert!(
        session
            .create_channel(
                &bob,
                ChannelSpec {
                    name: "side".to_string(),
                    components: quiet(),
                    participants: vec![],
                    classification: None,
                    language: None,
                    uses_workspace: None,
                    internal_actors: Vec::new()
                }
            )
            .await
            .is_err(),
        "nor to add channels"
    );
    assert!(session.delete_channel(&bob, main.id()).await.is_err(), "nor to delete them");
    assert!(
        main.remove_participant(&bob, ActorId::User(world.alice)).await.is_err(),
        "nor to remove others"
    );
    assert!(
        main.reclassify(&bob, Classification::general(Confidentiality::Public)).await.is_err(),
        "nor to reclassify the channel"
    );
    assert!(denied(
        &main.update_language(&bob, Some("fr")).await.expect_err("nor to change its language")
    ));
    assert!(denied(
        &session
            .update_language(&bob, Some("fr"))
            .await
            .expect_err("nor the session's (issue 425)")
    ));
    assert!(denied(
        &session
            .update_time_zone(&bob, Some("Asia/Tokyo"))
            .await
            .expect_err("nor its time zone (issue 424)")
    ));
    assert!(denied(
        &session
            .update_approval_rules(&bob, vec![])
            .await
            .expect_err("nor what it restricts of tool calls")
    ));
    assert!(denied(
        &main.update_approval_rules(&bob, vec![]).await.expect_err("in any of its channels")
    ));
    assert!(denied(
        &session
            .update_description(&bob, Some("his"), None)
            .await
            .expect_err("nor to say what it is for (issue 245)")
    ));
    assert!(denied(
        &main
            .update_description(&bob, None, Some("Do as bob says."))
            .await
            .expect_err("nor what one of its channels is for")
    ));
    assert!(denied(
        &world.manager.stop(&bob, session.id()).await.expect_err("nor to stop it for everyone")
    ));
    assert!(denied(&world.manager.delete(&bob, session.id()).await.expect_err("nor to delete it")));
    assert_eq!(session.name(), "shared", "nothing changed");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// S3: the owner changes the session and its channels, and ends it.
#[tokio::test]
async fn the_owner_changes_and_ends_the_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);

    session.update_name(&alice, "renamed").await.expect("hers to rename");
    let main = Arc::clone(&session.channels()[0]);
    session
        .update_language(&alice, Some("de"))
        .await
        .expect("and to choose the language of its channels (issue 425)");
    main.update_language(&alice, Some("fr")).await.expect("or of one of them");
    session
        .update_time_zone(&alice, Some("Europe/Berlin"))
        .await
        .expect("and its time zone (issue 424)");
    let shell = |decision| {
        ApprovalRule::new(
            ToolMatcher { tool: "shell".to_string(), arguments: Default::default() },
            decision,
            Reach::Everywhere,
            None,
        )
        .unwrap()
    };
    session
        .update_approval_rules(&alice, vec![shell(RuleDecision::Deny)])
        .await
        .expect("and what it restricts of tool calls");
    main.update_approval_rules(&alice, vec![shell(RuleDecision::Ask)])
        .await
        .expect("in it or in one of its channels");
    assert!(
        main.update_approval_rules(&alice, vec![shell(RuleDecision::Allow)]).await.is_err(),
        "but not to allow them: a place only restricts"
    );
    session
        .update_description(&alice, Some("Release 2.0"), Some("Dates are in UTC."))
        .await
        .expect("and to say what it is for");
    main.update_description(&alice, Some("Planning"), None)
        .await
        .expect("and each of its channels");
    let side = session
        .create_channel(
            &alice,
            ChannelSpec {
                name: "side".to_string(),
                components: quiet(),
                participants: vec![],
                classification: None,
                language: None,
                uses_workspace: None,
                internal_actors: Vec::new(),
            },
        )
        .await
        .expect("and to add channels");
    side.add_participant(&alice, ActorId::User(world.bob), HistoryAccess::All)
        .await
        .expect("and people to them");
    session.delete_channel(&alice, side.id()).await.expect("and to delete them");
    assert!(world.manager.delete(&alice, session.id()).await.unwrap(), "and to delete it");
    assert!(not_found(&world.manager.open(&alice, session.id()).await.expect_err("gone")));
}

/// S4: a channel's history is read in pages, and a message of it by its id
/// (issue 218), as far as the reader's view of it goes: bob, taken into a
/// channel after something was said in it, reads from there on - in pages
/// as whole, and not the message before by its id -, beginning with what
/// alice gave him of the history, which only the two of them read (issue
/// 417); carol, a stranger, does not reach the channel. Asked as reaching
/// the channel is.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_history_is_read_in_pages_and_by_message_as_far_as_the_readers_view_goes() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::history::HistoryEntry;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let side = session
        .create_channel(
            &alice,
            ChannelSpec {
                name: "side".to_string(),
                components: quiet(),
                participants: vec![],
                classification: None,
                language: None,
                uses_workspace: None,
                internal_actors: Vec::new(),
            },
        )
        .await
        .unwrap();
    let hers = side.user_actor(world.alice).unwrap();
    let mut watch = hers.watch();
    let mut say = async |text: &str| {
        hers.send_message(Some(text.to_string()), None, Vec::new()).await.expect("sent");
        let ActorEvent::CompleteMessageGenerated(said) =
            shown(&mut watch, "CompleteMessageGenerated").await
        else {
            unreachable!()
        };
        // Shown as it is let in, recorded right after.
        time::timeout(Duration::from_secs(5), async {
            while !side
                .visible_messages(ActorId::User(world.alice))
                .await
                .iter()
                .any(|m| m.id == said.id)
            {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in the history");
        said.id
    };
    let before = say("before bob").await;
    side.add_participant(&alice, ActorId::User(world.bob), HistoryAccess::None)
        .await
        .expect("without what was said");
    let after = say("after bob").await;

    let reach = |user| {
        let manager = &world.manager;
        let (session, channel) = (session.id(), side.id());
        async move {
            manager
                .views(Viewer::new(World::as_user(user)))
                .session(session)
                .await?
                .channel(channel)
                .await
        }
    };
    let texts = |items: &[HistoryEntry]| {
        items
            .iter()
            .filter_map(HistoryEntry::said)
            .filter_map(|m| m.content.clone())
            .collect::<Vec<_>>()
    };
    let theirs = reach(world.alice).await.unwrap();
    let page = theirs.history_page(None, 1).await.unwrap();
    assert_eq!(texts(&page.entries().cloned().collect::<Vec<_>>()), ["after bob"]);
    let page = theirs.history_page(page.earlier, 1).await.unwrap();
    assert!(
        matches!(page.entries().collect::<Vec<_>>().as_slice(), [HistoryEntry::ParticipantJoined { participant, .. }] if *participant == ActorId::User(world.bob)),
        "where bob joined (issue 408): {:?}",
        page.items
    );
    // Given nothing before his join, nothing shared with him is recorded (issue 417).
    let page = theirs.history_page(page.earlier, 1).await.unwrap();
    assert_eq!(
        texts(&page.entries().cloned().collect::<Vec<_>>()),
        ["before bob"],
        "alice reads it all, page by page"
    );
    let page = theirs.history_page(page.earlier, 1).await.unwrap();
    assert!(
        matches!(page.entries().collect::<Vec<_>>().as_slice(), [HistoryEntry::ParticipantJoined { participant, .. }] if *participant == ActorId::User(world.alice)),
        "her own join as the channel's founder (issue 420): {:?}",
        page.items
    );
    assert_eq!(page.earlier, None, "and nothing older");

    let (his, asked) = world
        .asked(async {
            let view = reach(world.bob).await.expect("he takes part");
            (
                view.history_page(None, 10).await.unwrap(),
                view.message(before).await,
                view.message(after).await,
            )
        })
        .await;
    assert_eq!(
        asked,
        checks(&[
            "bob: GetSession(alice's) yes",
            "bob: GetChannel(alice's) yes",
            "bob: JoinChannel(bob) yes"
        ]),
        "reaching the channel, cleared for it; reading in it asks nothing more"
    );
    let (page, refused, read) = his;
    assert_eq!(
        (texts(&page.entries().cloned().collect::<Vec<_>>()), page.earlier),
        (vec!["after bob".to_string()], None),
        "bob from where he was taken in"
    );
    assert!(
        matches!(page.entries().collect::<Vec<_>>().as_slice(), [HistoryEntry::ParticipantJoined { participant, .. }, ..] if *participant == ActorId::User(world.bob)),
        "his own join first, given nothing before it: {:?}",
        page.items
    );
    assert!(matches!(refused, Err(ViewError::MessageNotFound { .. })), "not what was said before");
    assert_eq!(read.expect("what he sees").id, after);
    assert!(
        matches!(reach(world.carol).await, Err(ViewError::SessionNotFound(_))),
        "carol finds nothing of it"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// S5 (issue 404): a reader marks up to where they read a channel as they
/// reach it - the store asks to read it - and nothing more; the mark is
/// theirs alone, and a message before where their view begins is none they
/// read. Carol, who does not reach the channel, marks nothing.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn up_to_where_a_channel_was_read_is_marked_by_its_reader_alone() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::message::MessageId;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let side = session
        .create_channel(
            &alice,
            ChannelSpec {
                name: "side".to_string(),
                components: quiet(),
                participants: vec![],
                classification: None,
                language: None,
                uses_workspace: None,
                internal_actors: Vec::new(),
            },
        )
        .await
        .unwrap();
    let hers = side.user_actor(world.alice).unwrap();
    let mut watch = hers.watch();
    let mut say = async |text: &str| {
        hers.send_message(Some(text.to_string()), None, Vec::new()).await.expect("sent");
        let ActorEvent::CompleteMessageGenerated(said) =
            shown(&mut watch, "CompleteMessageGenerated").await
        else {
            unreachable!()
        };
        time::timeout(Duration::from_secs(5), async {
            while !side
                .visible_messages(ActorId::User(world.alice))
                .await
                .iter()
                .any(|m| m.id == said.id)
            {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("in the history");
        said.id
    };
    let before = say("before bob").await;
    side.add_participant(&alice, ActorId::User(world.bob), HistoryAccess::None)
        .await
        .expect("without what was said");
    let after = say("after bob").await;
    let mark = |user: UserId, message: MessageId| {
        let controllers = world.runtime.controller_manager();
        let (session, channel) = (session.id(), side.id());
        async move {
            controllers
                .user_controller(user)
                .await
                .unwrap()
                .mark_read(&World::as_user(user), session, channel, message)
                .await
        }
    };
    let unread = |user: UserId| {
        let (manager, session, channel) = (&world.manager, session.id(), side.id());
        async move {
            manager
                .views(Viewer::new(World::as_user(user)))
                .session(session)
                .await
                .unwrap()
                .channel(channel)
                .await
                .unwrap()
                .unread()
                .await
                .unwrap()
                .map(|unread| unread.messages)
        }
    };
    assert_eq!(unread(world.bob).await, Some(1), "what alice said since he was taken in");

    let (marked, asked) = world.asked(mark(world.bob, after)).await;
    marked.expect("his to mark");
    assert_eq!(
        asked,
        checks(&["bob: GetChannel(alice's) yes", "bob: JoinChannel(bob) yes"]),
        "reaching the channel, as the views do; marking what he read asks nothing more"
    );
    assert_eq!(unread(world.bob).await, Some(0));
    assert_eq!(
        unread(world.alice).await,
        Some(0),
        "her own words are nothing new to her, and his mark is not hers"
    );
    assert!(
        matches!(
            mark(world.bob, before).await,
            Err(StoreError::NotFound { entity: "message", .. })
        ),
        "not before where his view begins"
    );

    let (marked, asked) = world.asked(mark(world.carol, after)).await;
    assert!(
        matches!(marked, Err(StoreError::NotFound { entity: "channel", .. })),
        "carol does not reach it: {marked:?}"
    );
    assert_eq!(asked, checks(&["carol: GetChannel(alice's) no"]));
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// S5 (issue 386): a participant follows a channel through their own actor
/// alone, whether it runs or not - asked as reaching the channel is, and
/// nothing more; following a stopped channel does not start it. Carol does
/// not reach it.
#[tokio::test]
async fn a_participant_follows_a_channel_through_their_own_actor_whether_it_runs_or_not() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let reach = |user| {
        let manager = &world.manager;
        let (session, channel) = (session.id(), main.id());
        async move {
            manager
                .views(Viewer::new(World::as_user(user)))
                .session(session)
                .await?
                .channel(channel)
                .await
        }
    };

    let (watch, asked) =
        world.asked(async { reach(world.bob).await.expect("he takes part").user_follow() }).await;
    assert_eq!(
        asked,
        checks(&[
            "bob: GetSession(alice's) yes",
            "bob: GetChannel(alice's) yes",
            "bob: JoinChannel(bob) yes"
        ]),
        "reaching the channel; following asks nothing more"
    );
    let mut his = watch.expect("his actor's");
    main.user_actor(world.alice)
        .unwrap()
        .send_message(Some("hello bob".to_string()), None, Vec::new())
        .await
        .unwrap();
    let ActorEvent::CompleteMessageGenerated(said) =
        shown(&mut his, "CompleteMessageGenerated").await
    else {
        unreachable!()
    };
    assert_eq!(
        said.content.as_deref(),
        Some("hello bob"),
        "what alice said, routed to him - not to her own actor"
    );
    assert!(
        matches!(reach(world.carol).await, Err(ViewError::SessionNotFound(_))),
        "carol finds nothing of it"
    );

    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    let ended = time::timeout(Duration::from_secs(5), async {
        while !matches!(his.next().await, Ok(Shown::Ended)) {}
    })
    .await;
    assert!(ended.is_ok(), "he is told his actor ended");
    let stopped = reach(world.bob).await.expect("he still takes part");
    assert!(stopped.user_follow().is_some(), "and follows the channel while it does not run");
    assert!(stopped.running().is_none(), "without starting it");
}

// --- Taking someone into a session (issue 221) ------------------------------

/// Whoever may change the session takes a user into it and into a channel
/// at once - asked before anything of the one taken in; who may not learns
/// nothing of them. Who is in the session already is added to a channel by
/// whoever may change the channel alone.
#[tokio::test]
async fn taking_someone_in_is_for_whoever_may_change_the_session_and_the_channel() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let carol = ActorId::User(world.carol);

    let (refused, asked) = world
        .asked(session.add_participant(&bob, carol, Some((main.id(), HistoryAccess::All))))
        .await;
    assert!(denied(&refused.expect_err("not his session")));
    assert_eq!(asked, checks(&["bob: UpdateSession(alice's) no"]), "nothing of carol is asked");

    let (added, asked) = world
        .asked(session.add_participant(&alice, carol, Some((main.id(), HistoryAccess::All))))
        .await;
    added.expect("alice's to take her in");
    assert_eq!(asked, checks(&[
        "alice: UpdateSession(alice's) yes",
        "alice: GetChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: UpdateChannel(alice's) yes",
        "alice: GetUser(system's) yes",
        "alice: JoinChannel(carol) yes",
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
        "alice: UpdateChannel(alice's) yes",
        "alice: JoinChannel(carol) yes",
        "alice: GetChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: UpdateChannel(alice's) yes",
    ]), "changing the session and the channel, reading carol and whether she is cleared - then the store and the channel ask again as they store");
    assert!(
        world.manager.open(&World::as_user(world.carol), session.id()).await.is_ok(),
        "carol reads the session now"
    );
    assert!(
        world.participants(world.carol, session.id(), main.id()).await.contains(&carol),
        "and takes part in the channel"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// An agent is taken in only by who may read it (issue 14): nobody brings
/// in an agent whose tools, and the credentials behind them, they may not
/// use - and only one that may run, whose personality is its to read.
#[tokio::test]
async fn an_agent_is_taken_in_only_by_who_may_read_it_and_only_if_it_may_run() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice, world.carol]).await;
    let bobs_helper = world.agent("bobs-helper", world.bob, &shared).await;
    let alices_helper = world.agent("alices-helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let alice = World::as_user(world.alice);

    let (refused, asked) = world
        .asked(session.add_participant(
            &alice,
            ActorId::Agent(bobs_helper),
            Some((main.id(), HistoryAccess::All)),
        ))
        .await;
    assert!(not_found(&refused.expect_err("not hers to read")), "answered as not there");
    assert_eq!(
        asked.last().map(String::as_str),
        Some("alice: GetAgent(bob's) no"),
        "asked after what is hers to ask: {asked:?}"
    );

    let mut withdrawn =
        Personality { security_attributes: attributes(Some(world.bob), &[world.carol]), ..shared };
    world
        .runtime
        .entity_stores()
        .personalities
        .update(&World::as_user(world.bob), &mut withdrawn)
        .await
        .unwrap();
    let (refused, asked) = world
        .asked(session.add_participant(
            &alice,
            ActorId::Agent(alices_helper),
            Some((main.id(), HistoryAccess::All)),
        ))
        .await;
    assert!(denied(&refused.expect_err("it may not run")));
    assert_eq!(
        asked.last().map(String::as_str),
        Some("alice as alices-helper: GetPersonality(bob's) no"),
        "its personality, in its own name: {asked:?}"
    );
    assert!(
        !session
            .session()
            .participants
            .iter()
            .any(|p| p.actor_id() == ActorId::Agent(alices_helper)),
        "nothing was stored"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Who is not cleared for the channel is not taken in - into the channel,
/// nor into the session: everything is asked before anything is stored. An
/// agent is cleared by its owner and, if its personality limits it, by
/// those roles, too.
#[tokio::test]
async fn who_is_not_cleared_for_the_channel_is_not_taken_in() {
    let world = World::new().await;
    let dave = make_user("dave");
    world.runtime.entity_stores().users.create(&World::system(), &dave).await.unwrap();
    let mut limited = Personality {
        roles: Some(vec![]),
        ..world.personality("limited", Some(world.alice), &[]).await
    };
    world
        .runtime
        .entity_stores()
        .personalities
        .update(&World::as_user(world.alice), &mut limited)
        .await
        .unwrap();
    let helper = world.agent("limited-helper", world.alice, &limited).await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let alice = World::as_user(world.alice);

    for (who, what) in [
        (ActorId::User(dave.id), "a user without a role"),
        (ActorId::Agent(helper), "an agent its personality limits"),
    ] {
        let refused = session
            .add_participant(&alice, who, Some((main.id(), HistoryAccess::All)))
            .await
            .expect_err(what);
        assert!(
            matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::NotCleared { .. })),
            "{what}: {refused:#}"
        );
        assert!(
            !session.session().participants.iter().any(|p| p.actor_id() == who),
            "{what}: not in the session either"
        );
    }
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Searching whom to take in is for whoever may change the session: the
/// search tells who exists. It shows who is found with whether they take
/// part, and why they cannot be taken in - coarsely, never which clearance
/// is missing.
#[tokio::test]
async fn searching_whom_to_take_in_is_for_whoever_may_change_the_session() {
    use caelumora_model::snapshot::CandidateRefusal;
    let world = World::new().await;
    let dave = make_user("dave");
    world.runtime.entity_stores().users.create(&World::system(), &dave).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    let views = |user| world.manager.views(Viewer::new(World::as_user(user)));

    let his = views(world.bob).session(session.id()).await.unwrap();
    assert!(
        matches!(his.candidates("ca", Some(main)).await, Err(ViewError::NotPermitted(_))),
        "bob may not change the session"
    );

    let hers = views(world.alice).session(session.id()).await.unwrap();
    assert!(
        hers.candidates("c", Some(main)).await.unwrap().is_empty(),
        "a single character finds nobody"
    );
    let found = |prefix: &'static str| {
        let hers = hers.clone();
        async move { hers.candidates(prefix, Some(main)).await.unwrap() }
    };
    let carol = found("CA").await;
    assert!(
        matches!(carol.as_slice(), [c] if c.id == ActorId::User(world.carol) && !c.in_session && c.refusal.is_none()),
        "carol, to be taken in: {carol:?}"
    );
    let bob = found("bo").await;
    assert!(
        matches!(bob.as_slice(), [b] if b.in_session && b.in_channel),
        "bob takes part: {bob:?}"
    );
    let dave_found = found("da").await;
    assert!(
        matches!(dave_found.as_slice(), [d] if d.id == ActorId::User(dave.id) && d.refusal == Some(CandidateRefusal::NotCleared)),
        "dave exists, but is not cleared: {dave_found:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Messages ----------------------------------------------------------------

/// M0: a message enters the channel as its sender classified it, if the
/// flow lets it in (issue 133) - decided by the channel alone, in order, as
/// it admits the message (issue 304): her actor asks nothing. What is not
/// let in, its sender is told, and why (`MessageRejected`); nobody else
/// hears of it.
#[tokio::test]
async fn a_message_is_let_in_by_the_channel_and_its_sender_told_if_not() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let (mut hers, mut his) = (alice.watch(), bob.watch());

    let (refused, asked) = world
        .asked(async {
            alice
                .send_message(
                    Some("secret".to_string()),
                    Some(Classification::general(Confidentiality::Confidential)),
                    Vec::new(),
                )
                .await
                .expect("sent: the channel decides");
            shown(&mut hers, "MessageRejected").await
        })
        .await;
    let ActorEvent::MessageRejected { rejected, message, reason } = refused else { unreachable!() };
    assert_eq!(
        (rejected.as_str(), reason),
        ("CompleteMessageGenerated", Rejection::ClassificationRefused),
        "above the channel"
    );
    assert!(message.is_some(), "naming the message");
    assert_eq!(asked, checks(&["alice: Publish no"]), "asked by the channel alone");

    alice.send_message(Some("hello".to_string()), None, Vec::new()).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) =
        shown(&mut his, "CompleteMessageGenerated").await
    else {
        unreachable!()
    };
    assert_eq!(
        (said.content.as_deref(), Some(said.id) != message),
        (Some("hello"), true),
        "he hears what was let in, and nothing of what was not"
    );
    while let Ok(envelope) = his.try_recv() {
        assert_ne!(envelope.event.kind(), "MessageRejected", "nor of the refusal");
    }
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M1: a message is its author's to modify and to take back (issue 234) -
/// not another participant's. Asked by the channel alone, as it admits the
/// change (issue 304), with whether the modified message may enter the
/// channel as classified; who is refused is told why (`MessageRejected`).
/// Another's message modified would speak as its author, which nobody's
/// actor can send (issue 228): it is refused by his actor, which asks
/// nothing.
#[tokio::test]
async fn a_message_is_modified_and_taken_back_by_its_author_alone() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let mut watch = bob.watch();
    alice.send_message(Some("helo".to_string()), None, Vec::new()).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) =
        shown(&mut watch, "CompleteMessageGenerated").await
    else {
        unreachable!()
    };
    let said = said.id;

    let (refused, asked) =
        world.asked(bob.modify_message(said, Some("bob's".to_string()), None, Vec::new())).await;
    assert!(denied(&refused.expect_err("not his")));
    assert!(asked.is_empty(), "it cannot be sent in his name: {asked:?}");
    let (refused, asked) = world
        .asked(async {
            bob.revoke_message(said).await.expect("sent: the channel decides");
            shown(&mut watch, "MessageRejected").await
        })
        .await;
    assert!(
        matches!(refused, ActorEvent::MessageRejected { reason: Rejection::Forbidden, message: Some(m), .. } if m == said),
        "not his to take back, and he is told"
    );
    assert_eq!(asked, checks(&["bob: RevokeMessage(alice's) no"]), "asked by the channel alone");

    let (_, asked) = world
        .asked(async {
            alice
                .modify_message(said, Some("hello".to_string()), None, Vec::new())
                .await
                .expect("hers");
            shown(&mut watch, "MessageModified").await;
        })
        .await;
    assert_eq!(
        asked,
        checks(&["alice: ModifyMessage(alice's) yes", "alice: Publish yes"]),
        "the channel, as it admits it: her permission, then the flow"
    );
    let (_, asked) = world
        .asked(async {
            alice.revoke_message(said).await.expect("hers");
            shown(&mut watch, "MessageRevoked").await;
        })
        .await;
    assert_eq!(asked, checks(&["alice: RevokeMessage(alice's) yes"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M3: a message is reclassified (issues 133, 239) by its author, or by the
/// channel's owner - not by another participant. Asked as `Reclassify` of
/// the message's author, then the flow of the new classification, by the
/// channel alone as it admits it (issue 304); nothing else, since nothing of
/// the message's content enters the channel again. Who is refused is told
/// why (`MessageRejected`).
#[tokio::test]
async fn a_message_is_reclassified_by_its_author_or_the_channels_owner() {
    use caelumora_model::actor::ActorEvent;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let (mut hers, mut his) = (alice.watch(), bob.watch());
    alice.send_message(Some("from alice".to_string()), None, Vec::new()).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) =
        shown(&mut his, "CompleteMessageGenerated").await
    else {
        unreachable!()
    };
    let alices = said.id;
    bob.send_message(Some("from bob".to_string()), None, Vec::new()).await.expect("sent");
    // Her actor shows her what she says, too; his message is what the
    // channel routes to her.
    let bobs = loop {
        let ActorEvent::CompleteMessageGenerated(said) =
            shown(&mut hers, "CompleteMessageGenerated").await
        else {
            unreachable!()
        };
        if said.sender_id == ActorId::User(world.bob) {
            break said.id;
        }
    };
    let public = Classification::general(Confidentiality::Public);

    let (refused, asked) = world
        .asked(async {
            bob.reclassify_message(alices, public.clone())
                .await
                .expect("sent: the channel decides");
            shown(&mut his, "MessageRejected").await
        })
        .await;
    assert!(
        matches!(refused, ActorEvent::MessageRejected { reason: Rejection::Forbidden, message: Some(m), .. } if m == alices),
        "not his, and he is told"
    );
    assert_eq!(
        asked,
        checks(&["bob: Reclassify(alice's message) no"]),
        "refused before the flow is asked"
    );

    let (_, asked) = world
        .asked(async {
            bob.reclassify_message(bobs, public.clone()).await.expect("his own");
            shown(&mut hers, "MessageReclassified").await;
        })
        .await;
    assert_eq!(
        asked,
        checks(&["bob: Reclassify(bob's message) yes", "bob: Publish yes"]),
        "the channel: his permission, then the flow (issue 304)"
    );

    let internal = Classification::general(Confidentiality::Internal);
    let (_, asked) = world.asked(async {
        alice.reclassify_message(bobs, internal.clone()).await.expect("her channel");
        // His actor showed him his own before.
        while !matches!(shown(&mut his, "MessageReclassified").await, ActorEvent::MessageReclassified { classification, .. } if classification == internal) {}
    }).await;
    assert_eq!(
        asked,
        checks(&["alice: Reclassify(bob's message) yes", "alice: Publish yes"]),
        "the owner, another's"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M4: a file is attached (issue 282) by a participant of the channel: uploaded as it may flow into
/// the channel and the storage it is kept in, asked of the flow alone, and named by the message
/// that carries it - the channel asks the flow of each again as it lets the message in. Another
/// participant's message cannot carry it. Its content is read through the message by whoever reads
/// it, asked what reading the message asks; a stranger reaches neither. What a tool's result showed
/// of it is read the same way through the message of the result, where the result is shown.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_file_is_attached_by_a_participant_and_read_with_its_message() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::message::{Message, MessageId, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolCallResult, ToolVisibility};
    use caelumora_runtime::attachment::AttachmentError;
    use futures::StreamExt;
    use serde_json::Value;
    use std::slice;
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let attachments = world.runtime.attachment_manager();
    let (manager, session_id, channel_id) = (&world.manager, session.id(), main.id());
    let channel_of = |user: UserId| async move {
        manager
            .views(Viewer::new(World::as_user(user)))
            .session(session_id)
            .await?
            .channel(channel_id)
            .await
    };

    let (uploaded, asked) = world
        .asked(attachments.create(
            &World::as_user(world.alice),
            &main.channel(),
            "notes.txt",
            None,
            (5, file("notes")),
        ))
        .await;
    let uploaded = uploaded.expect("hers to upload");
    assert_eq!(
        asked,
        checks(&["alice: Publish yes"]),
        "the flow into the channel, the storage bounding nothing"
    );
    assert!(channel_of(world.carol).await.is_err(), "a stranger reaches no channel to upload into");

    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let (refused, asked) = world.asked(bob.send_message(None, None, vec![uploaded.id])).await;
    assert!(
        matches!(
            refused.expect_err("not his").downcast::<AttachmentError>(),
            Ok(AttachmentError::NotFound(_))
        ),
        "not one of his uploads"
    );
    assert!(asked.is_empty(), "asking nothing: {asked:?}");
    let mut watch = bob.watch();
    let (said, asked) = world
        .asked(async {
            alice
                .send_message(Some("my notes".to_string()), None, vec![uploaded.id])
                .await
                .expect("sent");
            shown(&mut watch, "CompleteMessageGenerated").await
        })
        .await;
    let ActorEvent::CompleteMessageGenerated(said) = said else { unreachable!() };
    assert_eq!(said.attachments, slice::from_ref(&uploaded), "he hears it with the message");
    assert_eq!(
        asked,
        checks(&["alice: Publish yes", "alice: Publish yes"]),
        "the channel, as it admits it: the message, then its attachment"
    );

    let (read, asked) = world
        .asked(async {
            let (_, _, content) = channel_of(world.bob)
                .await
                .unwrap()
                .attachment_content(said.id, uploaded.id)
                .await
                .expect("he reads it with the message");
            content
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .flat_map(|chunk| chunk.unwrap().to_vec())
                .collect::<Vec<u8>>()
        })
        .await;
    assert_eq!(read, b"notes");
    assert_eq!(
        asked,
        checks(&[
            "bob: GetSession(alice's) yes",
            "bob: GetChannel(alice's) yes",
            "bob: JoinChannel(bob) yes"
        ]),
        "reaching the channel, as reading the message asks; its attachment asks nothing more"
    );

    let showing = |visibility: ToolVisibility| {
        let call = ToolCall {
            id: Some("1".to_string()),
            function_name: "view_attachment".to_string(),
            arguments: Value::Null,
            visibility,
            presentation: Default::default(),
        };
        let result = ToolCallResult {
            shown: vec![uploaded.clone()],
            ..ToolCallResult::of(
                &call,
                Default::default(),
                "shown".to_string(),
                main.classification(),
            )
        };
        Message {
            id: MessageId::new(),
            sender_id: ActorId::Agent(AgentId::new()),
            role: MessageRole::Tool,
            tool_results: vec![result],
            ..Message::new(main.classification())
        }
    };
    let shown = showing(ToolVisibility::SHOWN);
    let withheld = showing(ToolVisibility::WITHOUT_RESULT);
    main.record_said(vec![shown.clone(), withheld.clone()]).await;
    let (read, asked) = world
        .asked(async {
            channel_of(world.bob).await.unwrap().attachment(shown.id, uploaded.id).await
        })
        .await;
    assert_eq!(read.expect("what a result shown to him showed"), uploaded);
    assert_eq!(
        asked,
        checks(&[
            "bob: GetSession(alice's) yes",
            "bob: GetChannel(alice's) yes",
            "bob: JoinChannel(bob) yes"
        ]),
        "as reading the message of the result asks"
    );
    let hidden = channel_of(world.bob).await.unwrap().attachment(withheld.id, uploaded.id).await;
    assert!(
        matches!(hidden, Err(ViewError::Attachment(AttachmentError::NotFound(_)))),
        "not what a result withheld from him showed: {hidden:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Users and their profiles -------------------------------------------------

/// U1: a user's record is the system's, their profile their own; what they
/// say of themselves reaches everyone in their session.
#[tokio::test]
async fn a_profile_is_written_by_its_user_alone_and_read_by_who_shares_a_session() {
    let world = World::new().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let profile =
        UserProfile::new(world.alice, Some(Introduction::new("I run the team.").unwrap()));
    world
        .runtime
        .entity_stores()
        .user_profiles
        .create(&alice, &profile)
        .await
        .expect("she writes her own");
    let mut forged = UserProfile::new(world.alice, Some(Introduction::new("I quit.").unwrap()));
    assert!(
        world.runtime.entity_stores().user_profiles.update(&bob, &mut forged).await.is_err(),
        "bob does not write hers"
    );
    let carols = UserProfile::new(world.carol, Some(Introduction::new("hello").unwrap()));
    assert!(
        world.runtime.entity_stores().user_profiles.create(&bob, &carols).await.is_err(),
        "nor anyone else's"
    );

    let mut promoted = world
        .runtime
        .entity_stores()
        .users
        .get(&alice, world.alice)
        .await
        .expect("everyone reads a user");
    promoted.roles.push(RoleId::new());
    assert!(
        world.runtime.entity_stores().users.update(&alice, &mut promoted).await.is_err(),
        "nobody grants themselves roles"
    );

    let session = world.shared_session(&[], &[]).await;
    let channel = session.channels()[0].id();
    let view = world
        .manager
        .views(Viewer::new(bob))
        .session(session.id())
        .await
        .unwrap()
        .channel(channel)
        .await
        .unwrap();
    let participants = view.participants().await;
    let hers = participants.iter().find(|m| m.id() == ActorId::User(world.alice)).unwrap();
    assert_eq!(
        hers.introduction().await.unwrap().as_ref().map(|introduction| introduction.as_str()),
        Some("I run the team."),
        "bob reads how she introduces herself"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

// --- The permissions each action needs --------------------------------------
//
// What each action asks of the policy, in whose name, of whose entity, and
// what it is answered - for its owner, for someone taking part, for a
// stranger: the record of which permissions an action needs (issue 198).
// The runtime's own reads in its own name are left out (`World::asked`).

/// Making an agent of a personality (issue 227): creating it, in the
/// maker's name and owned by them, before the personality is read in their
/// name, and the store creating it again. One of a personality they may not
/// read is not made, and answered as not there.
#[tokio::test]
async fn making_an_agent_needs_creating_it_then_its_personality() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let bobs = world.personality("bobs", Some(world.bob), &[]).await;
    let agents = world.runtime.agent_manager();
    let alice = World::as_user(world.alice);
    let hers = Scope::UserScope { user_id: world.alice };

    let (made, asked) = world.asked(agents.create(&alice, "helper", hers.clone(), shared.id)).await;
    let made = made.expect("hers to make");
    assert_eq!(
        (made.personality_id, made.security_attributes.owner_id),
        (shared.id, ActorId::User(world.alice))
    );
    assert_eq!(
        asked,
        checks(&[
            "alice: CreateAgent(alice's) yes",
            "alice: GetPersonality(bob's) yes",
            "alice: CreateAgent(alice's) yes",
        ])
    );

    let (refused, asked) = world.asked(agents.create(&alice, "other", hers, bobs.id)).await;
    assert!(
        matches!(refused, Err(AgentError::Store(StoreError::NotFound { .. }))),
        "answered as not there: {refused:?}"
    );
    assert_eq!(
        asked,
        checks(&["alice: CreateAgent(alice's) yes", "alice: GetPersonality(bob's) no"]),
        "nothing is made"
    );
    assert_eq!(
        world
            .runtime
            .entity_stores()
            .agents
            .list(&World::system(), &Query::all())
            .await
            .map(|page| page.items)
            .unwrap()
            .len(),
        1,
        "only hers"
    );
}

/// No agent is made in the system's name (issue 477): it would act with all
/// the system's rights. The system itself may make anything, so the store
/// refuses it.
#[tokio::test]
async fn no_agent_is_made_in_the_systems_name() {
    let world = World::new().await;
    let plain = world.personality("plain", None, &[]).await;
    let agents = world.runtime.agent_manager();
    let made = agents.create(&World::system(), "helper", Scope::GlobalScope, plain.id).await;
    assert!(matches!(made, Err(AgentError::Store(StoreError::Validation(_)))), "{made:?}");
}

/// Making an agent in a session's or a channel's scope (issue 227): who may
/// not read the session puts none there - it would keep the session from
/// being deleted -, asked after making it and before the personality; who
/// takes part does. A channel is asked about with the session the scope
/// names: one of another session is none.
#[tokio::test]
async fn making_an_agent_in_a_session_needs_reading_it() {
    let world = World::new().await;
    let plain = world.personality("plain", None, &[]).await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    let other = world.session_of(&[], &[], &[]).await;
    let agents = world.runtime.agent_manager();
    let in_session = Scope::SessionScope { session_id: session.id() };

    let (refused, asked) = world
        .asked(agents.create(&World::as_user(world.carol), "spy", in_session.clone(), plain.id))
        .await;
    assert!(
        matches!(refused, Err(AgentError::Store(StoreError::NotFound { entity: "session", .. }))),
        "not hers to read: {refused:?}"
    );
    assert_eq!(
        asked,
        checks(&["carol: CreateAgent(carol's) yes", "carol: GetSession(alice's) no"]),
        "nothing else is asked"
    );

    let (made, asked) = world
        .asked(agents.create(&World::as_user(world.bob), "helper", in_session, plain.id))
        .await;
    made.expect("he takes part");
    assert_eq!(
        asked,
        checks(&[
            "bob: CreateAgent(bob's) yes",
            "bob: GetSession(alice's) yes",
            "bob: GetPersonality(system's) yes",
            "bob: CreateAgent(bob's) yes",
        ])
    );

    let in_main = Scope::ChannelScope { session_id: session.id(), channel_id: main };
    let (made, asked) = world
        .asked(agents.create(&World::as_user(world.bob), "minder", in_main.clone(), plain.id))
        .await;
    let minder = made.expect("he is in the channel");
    assert_eq!(asked[1], "bob: GetChannel(alice's) yes");
    let forged = Scope::ChannelScope { session_id: other.id(), channel_id: main };
    let (refused, _) =
        world.asked(agents.create(&World::as_user(world.bob), "forged", forged, plain.id)).await;
    assert!(
        matches!(refused, Err(AgentError::Store(StoreError::NotFound { entity: "channel", .. }))),
        "a channel of another session: {refused:?}"
    );
    let made = world
        .runtime
        .entity_stores()
        .agents
        .list(&World::system(), &Query::all())
        .await
        .map(|page| page.items)
        .unwrap();
    assert_eq!(made.len(), 2, "only his two");

    // A channel is not marked with its session (issue 458): its session is asked too, in S,
    // after the channel - its owner reads the channel still, and learns that the session is gone.
    let alice = World::as_user(world.alice);
    world.manager.delete(&World::system(), session.id()).await.expect("with his agents");
    let (refused, asked) = world.asked(agents.create(&alice, "late", in_main, plain.id)).await;
    assert!(
        matches!(refused, Err(AgentError::Store(StoreError::NotFound { entity: "session", .. }))),
        "a channel of a deleted session: {refused:?}"
    );
    assert_eq!(asked[1], "alice: GetChannel(alice's) yes");
    // Nor is one restored into it: it stays deleted with the session, as it was.
    let bob = World::as_user(world.bob);
    let refused = agents.restore(&bob, minder.id).await;
    assert!(matches!(refused, Err(AgentError::NotFound(_))), "its session is gone: {refused:?}");
    let stores = world.runtime.entity_stores();
    let session_mark = stores.sessions.get_deleted(&World::system(), session.id()).await.unwrap();
    let minder_mark = stores.agents.get_deleted(&World::system(), minder.id).await.unwrap();
    assert_eq!(minder_mark.removal, session_mark.removal, "with the session's mark still");
}

/// Issue 458: nothing is put into the scope of a deleted session - a workspace neither made there
/// nor restored into it.
#[tokio::test]
async fn a_workspace_is_not_put_into_a_deleted_session() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let session = world.session_of(&[], &[], &[]).await;
    let in_session = Scope::SessionScope { session_id: session.id() };
    let workspaces = world.runtime.workspace_manager();
    let internal = Classification::general(Confidentiality::Internal);
    let docs = workspaces
        .create(&alice, "docs", in_session.clone(), "main", None, internal.clone())
        .await
        .expect("hers, in her session");
    assert!(workspaces.delete(&alice, docs.id).await.unwrap(), "deleted alone first");
    world.manager.stop(&alice, session.id()).await.unwrap();
    world.manager.delete(&alice, session.id()).await.unwrap();

    let refused = workspaces.restore(&alice, docs.id).await;
    assert!(matches!(refused, Err(WorkspaceError::NotFound(_))), "not restored: {refused:?}");
    let refused = workspaces.create(&alice, "late", in_session, "main", None, internal).await;
    assert!(
        matches!(refused, Err(WorkspaceError::Store(StoreError::NotFound { .. }))),
        "not made: {refused:?}"
    );
}

/// Renaming an agent (issue 227), through the controller this instance
/// shares: the store asks reading it and changing it - its owner's alone.
/// Who may not read it finds none; who reads it is refused, and the agent
/// keeps its name.
#[tokio::test]
async fn renaming_an_agent_needs_changing_it() {
    let world = World::new().await;
    let personality = world.personality("plain", None, &[]).await;
    let helper = world.agent("helper", world.alice, &personality).await;
    let agent = world.runtime.controller_manager().agent_controller(helper).await.unwrap();

    let (renamed, asked) = world.asked(agent.update_name(&World::as_user(world.bob), "his")).await;
    assert!(matches!(renamed, Err(StoreError::NotFound { .. })), "not his to read: {renamed:?}");
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) no"]));

    let mut shared =
        world.runtime.entity_stores().agents.get(&World::system(), helper).await.unwrap();
    shared.security_attributes =
        shared.security_attributes.granting(Subject::Actor(ActorId::User(world.bob)), Grant::Read);
    world.runtime.entity_stores().agents.update(&World::system(), &mut shared).await.unwrap();
    let (renamed, asked) = world.asked(agent.update_name(&World::as_user(world.bob), "his")).await;
    assert!(
        matches!(renamed, Err(StoreError::PermissionDenied(_))),
        "his to read, not to change: {renamed:?}"
    );
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) yes", "bob: UpdateAgent(alice's) no"]));
    assert_eq!(agent.name(), "helper", "nothing changed");

    let (renamed, asked) =
        world.asked(agent.update_name(&World::as_user(world.alice), "assistant")).await;
    renamed.expect("hers to rename");
    // Shared past its controller, whose copy was outdated by that: asked, refused as outdated,
    // read again and asked again (issue 492).
    assert_eq!(
        asked,
        checks(&[
            "alice: GetAgent(alice's) yes",
            "alice: UpdateAgent(alice's) yes",
            "alice: GetAgent(alice's) yes",
            "alice: UpdateAgent(alice's) yes",
        ])
    );
    assert_eq!(
        world
            .runtime
            .entity_stores()
            .agents
            .get(&World::system(), helper)
            .await
            .unwrap()
            .name
            .as_str(),
        "assistant"
    );
}

/// Opening (starting) a session: reading it.
#[tokio::test]
async fn opening_a_session_needs_reading_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    for (user, answer) in [(world.alice, "yes"), (world.bob, "yes"), (world.carol, "no")] {
        let (_, asked) = world.asked(world.manager.open(&World::as_user(user), session.id())).await;
        let name = world.access_policy.name(ActorId::User(user));
        assert_eq!(asked, checks(&[&format!("{name}: GetSession(alice's) {answer}")]));
    }
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Stopping a session for everyone in it: reading it, and stopping it -
/// its owner's alone.
#[tokio::test]
async fn stopping_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let (_, asked) =
        world.asked(world.manager.stop(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: GetSession(alice's) no"]), "a stranger: not even found");
    let (_, asked) =
        world.asked(world.manager.stop(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: GetSession(alice's) yes", "bob: StopSession(alice's) no"]));
    let (stopped, asked) =
        world.asked(world.manager.stop(&World::as_user(world.alice), session.id())).await;
    assert!(stopped.unwrap());
    assert_eq!(
        asked,
        checks(&["alice: GetSession(alice's) yes", "alice: StopSession(alice's) yes"])
    );
}

/// Deleting a session: reading it and deleting it - its owner's alone. It
/// is marked and kept, with its channels (issue 164), and what lies in its
/// scope goes with it in her name (issue 320): an agent of somebody else's
/// she may not delete keeps the session, and she is told what is left.
#[tokio::test]
async fn deleting_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let personality = world.personality("plain", None, &[]).await;
    let bobs = Agent::new(
        ScopedName::new("helper", Scope::SessionScope { session_id: session.id() }).unwrap(),
        personality.id,
        ActorId::User(world.bob),
    );
    world.runtime.entity_stores().agents.create(&World::system(), &bobs).await.unwrap();
    let (_, asked) =
        world.asked(world.manager.delete(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: GetSession(alice's) no"]));
    let (_, asked) =
        world.asked(world.manager.delete(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: GetSession(alice's) yes", "bob: DeleteSession(alice's) no"]));
    let (refused, asked) = world.asked(world.manager.delete(&alice, session.id())).await;
    let refused = refused.expect_err("bob's agent is left");
    assert!(
        matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ScopeNotEmpty(ScopeNotEmpty { left, .. })) if *left == [(Agent::KIND, 1)]),
        "{refused}"
    );
    assert_eq!(asked, checks(&[
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
        "alice: GetAgent(bob's) no",
    ]), "asked by the session manager before it stops anything, again by the store, and for what lies in its scope");
    assert!(
        world.runtime.entity_stores().sessions.get(&alice, session.id()).await.is_ok(),
        "the session is kept"
    );
    let own = session.session().project;
    assert!(
        world.runtime.entity_stores().projects.get(&alice, own).await.is_ok(),
        "and its own project, marked with it, is unmarked with it (issue 458)"
    );
    assert!(
        world.runtime.entity_stores().agents.get(&World::system(), bobs.id).await.is_ok(),
        "and bob's agent"
    );

    world
        .runtime
        .agent_manager()
        .delete(&World::as_user(world.bob), bobs.id)
        .await
        .expect("his to delete");
    let (deleted, _) = world.asked(world.manager.delete(&alice, session.id())).await;
    assert!(deleted.unwrap(), "nothing of anybody else's is left");
    world.manager.restore(&alice, session.id()).await.unwrap();
    assert!(
        world.runtime.entity_stores().agents.get(&World::system(), bobs.id).await.is_err(),
        "deleted before the session, bob's agent stays deleted"
    );
}

/// A session's own project (issue 467) is read by who reads the session:
/// its owner and its participants, nobody else. What lies in its scope goes
/// with the session as what lies in the session's does, in the deleter's
/// name: a bundle of bob's she may not delete keeps the session, the
/// project and their groups, and she is told what is left of the session.
#[tokio::test]
async fn a_session_s_own_project_is_read_by_its_participants_and_goes_with_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let project = session.session().project;
    let stores = world.runtime.entity_stores();
    for (user, reads) in [(world.alice, true), (world.bob, true), (world.carol, false)] {
        assert_eq!(
            stores.projects.get(&World::as_user(user), project).await.is_ok(),
            reads,
            "{user}"
        );
    }
    let holder = Scope::ProjectScope { project_id: project };
    let bobs = ComponentBundle::new(
        ScopedName::new("tools", holder).unwrap(),
        quiet(),
        attributes(Some(world.bob), &[]),
    );
    stores.component_bundles.create(&World::system(), &bobs).await.unwrap();

    let (refused, asked) = world.asked(world.manager.delete(&alice, session.id())).await;
    let refused = refused.expect_err("bob's bundle is left");
    let session_scope = Scope::SessionScope { session_id: session.id() };
    let left = [(ComponentBundle::KIND, 1)];
    assert!(
        matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ScopeNotEmpty(ScopeNotEmpty { holder, left: l })) if *holder == session_scope && *l == left),
        "{refused}"
    );
    assert!(
        asked.contains(&"alice: GetComponentBundle(bob's) no".to_string()),
        "asked in her name: {asked:?}"
    );
    assert!(stores.projects.get(&alice, project).await.is_ok(), "the project is kept");
    assert!(
        stores.groups.get(&World::system(), GroupId::of_project(project)).await.is_ok(),
        "with its group"
    );

    stores
        .component_bundles
        .delete(&World::as_user(world.bob), bobs.id, &Removal::now(ActorId::User(world.bob)))
        .await
        .expect("his to delete");
    assert!(
        world.manager.delete(&alice, session.id()).await.unwrap(),
        "nothing of anybody else's is left"
    );
    assert!(
        stores.projects.get(&World::system(), project).await.is_err(),
        "the project goes with it"
    );
}

/// A shared project (issue 467) is its owner's: made in her own scope or the global one, never
/// in another's or a session's, and changed by her alone - who does not read it does not find
/// it, who reads it may change nothing. Its members read it through their group, which follows
/// them; who leaves reads it no longer, and hears so.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_project_is_its_owners_and_read_by_its_members() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let bob = World::as_user(world.bob);
    let carol = World::as_user(world.carol);
    let projects = &world.projects;
    let mine = Scope::UserScope { user_id: world.alice };
    let project = projects
        .create(&alice, "report", mine.clone(), Some("The yearly report"))
        .await
        .expect("in her own scope");
    projects.create(&alice, "handbook", Scope::GlobalScope, None).await.expect("or everywhere");
    let refused =
        projects.create(&alice, "theirs", Scope::UserScope { user_id: world.bob }, None).await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::Validation(_)))),
        "not in another's scope: {refused:?}"
    );
    let refused = projects.create(&alice, Project::OWN, mine.clone(), None).await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::Validation(_)))),
        "not under a session's own name: {refused:?}"
    );
    let refused = projects
        .create(&alice, "results", Scope::SessionScope { session_id: SessionId::new() }, None)
        .await;
    assert!(
        matches!(refused, Err(ProjectError::InvalidScope)),
        "a session's own is made with the session: {refused:?}"
    );

    let controller =
        world.runtime.controller_manager().project_controller(project.id).await.unwrap();
    controller.update_name(&alice, "annual report").await.expect("hers to rename");
    controller
        .update_description(&alice, Some("The annual report"))
        .await
        .expect("and to describe");
    let refused = controller.update_name(&bob, "mine").await;
    assert!(
        matches!(refused, Err(ProjectError::NotFound(_))),
        "who does not read it does not find it: {refused:?}"
    );
    let (made, asked) = world.asked(controller.update_members(&alice, vec![world.bob])).await;
    made.expect("she names its members");
    assert_eq!(asked, checks(&["alice: GetProject(alice's) yes", "alice: UpdateProject(alice's) yes", "alice: GetProject(alice's) yes", "alice: UpdateProject(alice's) yes"]),
        "reached, changed - before whether its members exist is asked (issue 312) -, then changed as stored");
    assert!(
        world.runtime.entity_stores().projects.get(&bob, project.id).await.is_ok(),
        "a member reads it"
    );
    assert!(
        world.runtime.entity_stores().projects.get(&carol, project.id).await.is_err(),
        "a stranger does not"
    );
    let (refused, asked) =
        world.asked(controller.update_members(&bob, vec![world.bob, world.carol])).await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::PermissionDenied(_)))),
        "a member changes nothing: {refused:?}"
    );
    assert_eq!(
        asked,
        checks(&["bob: GetProject(alice's) yes", "bob: UpdateProject(alice's) no"]),
        "refused before he learns whether carol exists"
    );
    let refused = controller.update_members(&alice, vec![UserId::new()]).await;
    assert!(matches!(refused, Err(ProjectError::UnknownMember(_))), "{refused:?}");

    let mut bobs = world.runtime.event_bus().subscribe(bob.clone(), vec![SubscriptionTopic::All]);
    controller.update_members(&alice, Vec::new()).await.unwrap();
    assert!(
        world.runtime.entity_stores().projects.get(&bob, project.id).await.is_err(),
        "who left reads it no longer"
    );
    hears(
        &mut bobs,
        [GlobalEvent::updated::<Project>(
            &project.id,
            &ProjectUpdateEvent::MembersChanged(Vec::new()),
        )],
    )
    .await;

    let to_carol =
        vec![AclEntry { subject: Subject::Actor(ActorId::User(world.carol)), grant: Grant::Read }];
    let refused = controller.update_access(&bob, to_carol.clone()).await;
    assert!(
        matches!(refused, Err(ProjectError::NotFound(_))),
        "who does not read it shares nothing: {refused:?}"
    );
    controller.update_access(&alice, to_carol).await.expect("hers to share");
    assert!(
        world.runtime.entity_stores().projects.get(&carol, project.id).await.is_ok(),
        "whom its list names reads it"
    );
    let refused = controller.update_name(&carol, "mine").await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::PermissionDenied(_)))),
        "and changes nothing: {refused:?}"
    );

    assert!(world.projects.delete(&alice, project.id).await.unwrap());
    let refused = world.projects.restore(&carol, project.id).await;
    assert!(
        matches!(refused, Err(ProjectError::NotFound(_))),
        "deleted, it is hers alone to restore: {refused:?}"
    );
    assert!(!world.projects.purge(&carol, project.id).await.unwrap(), "or to purge");
    world.projects.restore(&alice, project.id).await.expect("she restores it");
}

/// The views show a project to who reads it (issue 467): the shared ones in the list of
/// projects, a session's own with the session. Its members are named; its access list only its
/// owner sees, the group of an own project's members named by its session, as every own is
/// called alike.
#[tokio::test]
async fn a_project_is_seen_by_who_reads_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let bob = World::as_user(world.bob);
    let carol = World::as_user(world.carol);
    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));
    let project = world
        .projects
        .create(&alice, "report", Scope::UserScope { user_id: world.alice }, None)
        .await
        .unwrap();
    world
        .runtime
        .controller_manager()
        .project_controller(project.id)
        .await
        .unwrap()
        .update_members(&alice, vec![world.bob])
        .await
        .unwrap();

    let listed: Vec<ProjectId> =
        views(&bob).projects().await.unwrap().iter().map(|p| p.id()).collect();
    assert_eq!(
        listed,
        [project.id],
        "the shared ones he works on, not the own of a session he reads"
    );
    let his = views(&bob).project(project.id).await.unwrap().snapshot().await;
    assert_eq!(
        (his.members, his.can_change, his.access.len()),
        (vec![(world.bob, Some("bob".to_string()))], false, 0),
        "its members named, its list not his to see"
    );
    let hers = views(&alice).project(project.id).await.unwrap().snapshot().await;
    assert!(
        hers.can_change && hers.access.iter().any(|e| e.name.as_deref() == Some("report")),
        "she sees its list, its members by its name: {:?}",
        hers.access
    );
    assert!(
        matches!(views(&carol).project(project.id).await, Err(ViewError::ProjectNotFound(_))),
        "a stranger does not find it"
    );

    let hers = world.session_of(&[], &[], &[]).await;
    for lying_there in [&session, &hers] {
        lying_there.update_project(&alice, Some(project.id)).await.unwrap();
    }
    let in_it = |ctx: SecurityContext| {
        let views = views(&ctx);
        async move {
            let mut ids: Vec<SessionId> = views
                .project(project.id)
                .await
                .unwrap()
                .sessions()
                .await
                .unwrap()
                .iter()
                .map(|s| s.id())
                .collect();
            ids.sort();
            ids
        }
    };
    let mut both = vec![session.id(), hers.id()];
    both.sort();
    assert_eq!(in_it(alice.clone()).await, both, "its owner reads both sessions in it");
    assert_eq!(in_it(bob.clone()).await, [session.id()], "a member those he reads alone");
    for lying_there in [&session, &hers] {
        lying_there.update_project(&alice, None).await.unwrap();
    }

    let shown = views(&bob)
        .session(session.id())
        .await
        .unwrap()
        .snapshot(Detail::empty())
        .await
        .project
        .expect("a participant sees the session's project");
    assert_eq!(shown.id, session.session().project);
    let own = views(&alice).project(shown.id).await.unwrap().snapshot().await;
    let names: Vec<Option<&str>> = own.access.iter().map(|e| e.name.as_deref()).collect();
    assert_eq!(
        names,
        [Some("shared"), Some("shared")],
        "its members and its participants, both called by the session"
    );

    world.projects.delete(&alice, project.id).await.unwrap();
    world.manager.delete(&alice, session.id()).await.unwrap();
    let deleted: Vec<ProjectId> =
        views(&alice).deleted_projects().await.unwrap().iter().map(|d| d.id).collect();
    assert_eq!(
        deleted,
        [project.id],
        "hers to restore; a session's own comes back with its session"
    );
}

/// A session is put into a shared project by who may change the session and read the project
/// (issue 467), and taken out into its own again: the project it lies in is shared with its
/// participants, the one it leaves no longer. Nothing moves; another session's own takes no other
/// session in.
#[tokio::test]
async fn a_session_is_put_into_a_project_by_who_may_change_it_and_read_the_project() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let own = session.session().project;
    let project = world
        .projects
        .create(&alice, "report", Scope::UserScope { user_id: world.alice }, None)
        .await
        .unwrap();
    let carols = world
        .projects
        .create(
            &World::as_user(world.carol),
            "hers",
            Scope::UserScope { user_id: world.carol },
            None,
        )
        .await
        .unwrap();
    let projects = &world.runtime.entity_stores().projects;

    let refused = session
        .update_project(&bob, Some(project.id))
        .await
        .expect_err("bob may not change the session");
    assert!(denied(&refused), "{refused}");
    let refused = session
        .update_project(&alice, Some(carols.id))
        .await
        .expect_err("alice does not read carol's");
    assert!(not_found(&refused), "{refused}");
    let other = world.shared_session(&[], &[]).await;
    let refused = session
        .update_project(&alice, Some(other.session().project))
        .await
        .expect_err("another session's own");
    assert!(
        matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ProjectOutOfScope(_))),
        "{refused}"
    );

    let (put, asked) = world.asked(session.update_project(&alice, Some(project.id))).await;
    put.expect("hers to put it there");
    assert_eq!(asked, checks(&["alice: UpdateSession(alice's) yes", "alice: GetProject(alice's) yes", "alice: GetSession(alice's) yes", "alice: UpdateSession(alice's) yes"]),
        "changing the session, reaching the project, then storing the session; the share is the runtime's, the session's change asked");
    assert_eq!(session.session().project, project.id, "its results go there now");
    assert!(projects.get(&bob, project.id).await.is_ok(), "shared with its participants");
    assert!(
        projects.get(&World::system(), own).await.is_ok(),
        "its own stays, with what lies in it"
    );

    session.update_project(&alice, None).await.expect("and to take it out");
    assert_eq!(session.session().project, own, "into its own again");
    assert!(
        projects.get(&bob, project.id).await.is_err(),
        "the project it left is no longer shared with them"
    );
}

/// Lifting a session's own project (issue 467) makes it a shared project of its owner's, with the
/// session in it: its participants read it still. It is named anew - `own` is a session's own's
/// alone -, lifted once, and only by its owner. Taken out of it again, the session is made a new
/// project of its own, and the lifted one is no longer shared with its participants.
#[tokio::test]
async fn lifting_a_sessions_own_project_makes_it_shared_with_the_session_in_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let own = session.session().project;
    let controller = world.runtime.controller_manager().project_controller(own).await.unwrap();
    let mine = Scope::UserScope { user_id: world.alice };
    let projects = &world.runtime.entity_stores().projects;

    let refused =
        controller.lift(&World::as_user(world.carol), "results", Scope::GlobalScope).await;
    assert!(
        matches!(refused, Err(ProjectError::NotFound(_))),
        "who does not read it learns nothing of it: {refused:?}"
    );
    let refused = controller.lift(&bob, "results", Scope::UserScope { user_id: world.bob }).await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::PermissionDenied(_)))),
        "bob reads it, but it is not his: {refused:?}"
    );
    let refused = controller.lift(&alice, Project::OWN, mine.clone()).await;
    assert!(
        matches!(refused, Err(ProjectError::Store(StoreError::Validation(_)))),
        "named anew: {refused:?}"
    );
    let refused = controller
        .lift(&alice, "results", Scope::SessionScope { session_id: SessionId::new() })
        .await;
    assert!(
        matches!(refused, Err(ProjectError::InvalidScope)),
        "not into another session's scope: {refused:?}"
    );
    controller.lift(&alice, "results", mine.clone()).await.expect("hers to lift");
    let lifted = projects.get(&bob, own).await.expect("its participants read it still");
    assert_eq!(lifted.name.scope, mine, "in her scope");
    assert_eq!(session.session().project, own, "the session lies in it");
    let refused = controller.lift(&alice, "again", Scope::GlobalScope).await;
    assert!(matches!(refused, Err(ProjectError::InvalidScope)), "lifted once: {refused:?}");

    session.update_project(&alice, None).await.expect("taken out");
    let anew = session.session().project;
    assert_ne!(anew, own, "into a new project of its own");
    assert_eq!(
        projects.get(&bob, anew).await.unwrap().name,
        Project::own_name(session.id()),
        "read by its participants"
    );
    assert!(projects.get(&bob, own).await.is_err(), "the lifted one is no longer shared with them");
}

/// Deleting a project (issue 467) deletes the sessions that lie in it and what lies in its scope,
/// in the deleter's name: a bundle of bob's she may not delete keeps the project - what she could
/// delete stays deleted, to be restored by hand -, and she is told what is left. Restored, what
/// went with it comes back; purged, it goes for good. A session's own goes with its session alone.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn deleting_a_project_deletes_its_sessions_and_what_lies_in_it_as_far_as_the_deleter_may() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let own = session.session().project;
    assert!(
        matches!(world.projects.delete(&alice, own).await, Err(ProjectError::InvalidScope)),
        "a session's own goes with its session"
    );
    let project = world
        .projects
        .create(&alice, "report", Scope::UserScope { user_id: world.alice }, None)
        .await
        .unwrap();
    session.update_project(&alice, Some(project.id)).await.unwrap();
    let bobs = ComponentBundle::new(
        ScopedName::new("tools", project.holds()).unwrap(),
        quiet(),
        attributes(Some(world.bob), &[]),
    );
    let stores = world.runtime.entity_stores();
    stores.component_bundles.create(&World::system(), &bobs).await.unwrap();

    let refused =
        world.projects.delete(&alice, project.id).await.expect_err("bob's bundle is left");
    assert!(
        matches!(&refused, ProjectError::ScopeNotEmpty(ScopeNotEmpty { holder, left }) if *holder == project.holds() && *left == [(ComponentBundle::KIND, 1)]),
        "{refused:?}"
    );
    assert!(stores.projects.get(&alice, project.id).await.is_ok(), "the project is kept");
    assert!(
        stores.sessions.get(&World::system(), session.id()).await.is_err(),
        "what she could delete stays deleted"
    );
    world.manager.restore(&alice, session.id()).await.expect("restored by hand");

    stores
        .component_bundles
        .delete(&World::as_user(world.bob), bobs.id, &Removal::now(ActorId::User(world.bob)))
        .await
        .expect("his to delete");
    let bob = World::as_user(world.bob);
    world
        .runtime
        .controller_manager()
        .project_controller(project.id)
        .await
        .unwrap()
        .update_members(&alice, vec![world.bob])
        .await
        .unwrap();
    let his =
        create(&world.manager, &bob, make_session("his", world.bob)).await.expect("bob's session");
    his.update_project(&bob, Some(project.id)).await.expect("a member puts his session into it");
    let refused =
        world.projects.delete(&alice, project.id).await.expect_err("bob's session is left");
    assert!(
        matches!(&refused, ProjectError::ScopeNotEmpty(ScopeNotEmpty { left, .. }) if *left == [(Session::KIND, 1)]),
        "{refused:?}"
    );
    world.manager.restore(&alice, session.id()).await.expect("hers restored by hand");
    his.update_project(&bob, None).await.expect("bob takes his out");
    assert!(
        world.projects.delete(&alice, project.id).await.unwrap(),
        "nothing of anybody else's is left"
    );
    assert!(
        stores.sessions.get(&World::system(), session.id()).await.is_err(),
        "its session goes with it"
    );
    let mark = stores.projects.get_deleted(&World::system(), project.id).await.unwrap().removal;
    let session_mark =
        stores.sessions.get_deleted(&World::system(), session.id()).await.unwrap().removal;
    let own_mark = stores.projects.get_deleted(&World::system(), own).await.unwrap().removal;
    assert_eq!(
        (&session_mark, &own_mark),
        (&mark, &mark),
        "with the project's mark, as what goes with the session (issue 469)"
    );
    let refused = world.manager.restore(&alice, session.id()).await.expect_err("not alone");
    assert!(
        matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ProjectDeleted(_))),
        "it comes back with its project: {refused}"
    );
    assert!(
        stores.projects.get(&World::system(), own).await.is_err(),
        "with the session's own project"
    );
    world.projects.restore(&alice, project.id).await.expect("restored");
    assert!(stores.sessions.get(&World::system(), session.id()).await.is_ok(), "with its session");
    assert!(
        stores.projects.get(&World::system(), own).await.is_ok(),
        "and the session's own project"
    );
    assert!(world.projects.delete(&alice, project.id).await.unwrap());
    assert!(world.projects.purge(&World::system(), project.id).await.unwrap(), "purged");
    assert!(
        stores.sessions.get_deleted(&World::system(), session.id()).await.is_err(),
        "with its session"
    );
    assert!(
        stores.groups.get_deleted(&World::system(), GroupId::of_project(project.id)).await.is_err(),
        "and the group of its members"
    );
}

/// Deleting a channel is refused as deleting its session is (issue 320):
/// an agent of somebody else's in its scope, which she may not delete,
/// keeps the channel, running, and she is told what is left of which
/// channel.
#[tokio::test]
async fn a_channel_is_kept_with_what_of_its_scope_is_not_the_deleters() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let side = session
        .create_channel(
            &alice,
            ChannelSpec {
                name: "side".to_string(),
                components: quiet(),
                participants: vec![],
                classification: None,
                language: None,
                uses_workspace: None,
                internal_actors: Vec::new(),
            },
        )
        .await
        .unwrap()
        .id();
    let holder = Scope::ChannelScope { session_id: session.id(), channel_id: side };
    let personality = world.personality("plain", None, &[]).await;
    let bobs = Agent::new(
        ScopedName::new("helper", holder.clone()).unwrap(),
        personality.id,
        ActorId::User(world.bob),
    );
    world.runtime.entity_stores().agents.create(&World::system(), &bobs).await.unwrap();

    let refused = session.delete_channel(&alice, side).await.expect_err("bob's agent is left");
    let left_in = |scope: &Scope, left: &[(EntityKind, usize)]| {
        *scope == holder && left == [(Agent::KIND, 1)]
    };
    assert!(
        matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ScopeNotEmpty(ScopeNotEmpty { holder, left })) if left_in(holder, left)),
        "{refused}"
    );
    assert!(
        refused.to_string().contains(&format!("channel {side} of session {}", session.id())),
        "names the channel: {refused}"
    );
    assert!(session.channel(side).is_some(), "the channel is kept and runs on");
    assert!(
        world.runtime.entity_stores().agents.get(&World::system(), bobs.id).await.is_ok(),
        "and bob's agent"
    );
}

/// Issue 320: what went with a session comes back with it in the name of
/// whoever restores it, as far as they may; the session comes back all the
/// same, and what they may not restore - an agent of bob's in its scope, a
/// bundle of his in its own project's (issue 467) - stays deleted for its
/// owner to restore.
#[tokio::test]
async fn a_session_comes_back_with_what_lies_in_it_as_far_as_whoever_restores_it_may() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let personality = world.personality("plain", None, &[]).await;
    let bobs = Agent::new(
        ScopedName::new("helper", Scope::SessionScope { session_id: session.id() }).unwrap(),
        personality.id,
        ActorId::User(world.bob),
    );
    world.runtime.entity_stores().agents.create(&World::system(), &bobs).await.unwrap();
    let project = Scope::ProjectScope { project_id: session.session().project };
    let bundle = ComponentBundle::new(
        ScopedName::new("tools", project).unwrap(),
        quiet(),
        attributes(Some(world.bob), &[]),
    );
    world
        .runtime
        .entity_stores()
        .component_bundles
        .create(&World::system(), &bundle)
        .await
        .unwrap();
    world
        .manager
        .delete(&World::system(), session.id())
        .await
        .expect("the system's to delete, all of it");

    let (restored, asked) = world.asked(world.manager.restore(&alice, session.id())).await;
    restored.expect("restored all the same");
    assert!(
        asked.contains(&"alice: ReadDeletedAgent(bob's) no".to_string()),
        "asked in her name: {asked:?}"
    );
    assert!(
        asked.contains(&"alice: ReadDeletedComponentBundle(bob's) no".to_string()),
        "for its project's too: {asked:?}"
    );
    assert!(
        world.runtime.entity_stores().agents.get_deleted(&World::system(), bobs.id).await.is_ok(),
        "bob's agent stays deleted: not hers to restore"
    );
    assert!(
        world
            .runtime
            .entity_stores()
            .component_bundles
            .get_deleted(&World::system(), bundle.id)
            .await
            .is_ok(),
        "and his bundle"
    );
    world
        .runtime
        .agent_manager()
        .restore(&World::as_user(world.bob), bobs.id)
        .await
        .expect("bob restores his");
}

/// Creating a channel: the classification it is given, creating it - as
/// its session's owner -, then everyone's clearance for it (issue 312), and
/// the store creating it again and changing the session that lists it.
/// Then its founders join by the one way in (issue 420), each cleared again
/// as they join - and nothing more: naming them was creating the channel,
/// so changing it (`UpdateChannel`) is not asked.
#[tokio::test]
async fn creating_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let spec = |participants| ChannelSpec {
        name: "side".to_string(),
        components: quiet(),
        participants,
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };
    let (created, asked) = world
        .asked(
            session
                .create_channel(&World::as_user(world.alice), spec(vec![ActorId::User(world.bob)])),
        )
        .await;
    assert!(created.is_ok());
    assert_eq!(
        asked,
        checks(&[
            "alice: CreateChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: JoinChannel(bob) yes",
            "alice: CreateChannel(alice's) yes",
            "alice: GetSession(alice's) yes",
            "alice: UpdateSession(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: JoinChannel(bob) yes",
        ])
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A founder who is not cleared for a new channel keeps it from being made
/// (issue 420): asked before anything is stored, so that there is no channel
/// left behind, nor one announced.
#[tokio::test]
async fn a_founder_who_is_not_cleared_leaves_no_channel() {
    let world = World::new().await;
    let dave = make_user("dave");
    world.runtime.entity_stores().users.create(&World::system(), &dave).await.unwrap();
    world.access_policy.names.lock().unwrap().insert(ActorId::User(dave.id), "dave".to_string());
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    session
        .add_participant(&alice, ActorId::User(dave.id), None)
        .await
        .expect("dave takes part in the session");
    let spec = ChannelSpec {
        name: "side".to_string(),
        components: quiet(),
        participants: vec![ActorId::User(dave.id)],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };

    let (refused, asked) = world.asked(session.create_channel(&alice, spec)).await;
    assert!(
        matches!(
            refused.err().and_then(|e| e.downcast::<ChannelError>().ok()),
            Some(ChannelError::NotCleared { .. })
        ),
        "dave has no role"
    );
    assert_eq!(
        asked,
        checks(&[
            "alice: CreateChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: JoinChannel(dave) no"
        ]),
        "nothing stored"
    );
    assert!(session.channel_named("side").is_none(), "no channel runs");
    assert_eq!(session.session().channels.len(), 1, "nor does the session list one");
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Who may not create a channel is refused before anything is asked of
/// the others in it (issue 198, F2).
#[tokio::test]
async fn who_may_not_create_a_channel_is_refused_before_the_others_are_looked_at() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let spec = ChannelSpec {
        name: "his".to_string(),
        components: quiet(),
        participants: vec![ActorId::User(world.alice)],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };
    let (refused, asked) =
        world.asked(session.create_channel(&World::as_user(world.bob), spec)).await;
    assert!(refused.is_err());
    assert_eq!(asked, checks(&["bob: CreateChannel(alice's) no"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The component bundles a new channel names are read in the name of
/// whoever creates it, before creating it is asked (issue 321): the
/// channel builds them in its owner's, but whoever names them names only
/// what they may read. Once the share is withdrawn, the channel is refused
/// before anything else is asked or stored.
#[tokio::test]
async fn the_components_a_new_channel_names_are_read_in_its_creators_name() {
    let world = World::new().await;
    let policy = ComponentBundle::new(
        ScopedName::new("bobs-policy", Scope::GlobalScope).unwrap(),
        ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("policy", "broadcast_policy")],
        },
        attributes(Some(world.bob), &[world.alice]),
    );
    world
        .store
        .entity_stores()
        .unwrap()
        .component_bundles
        .create(&World::as_user(world.bob), &policy)
        .await
        .expect("bob keeps his policy");
    let session = world.shared_session(&[], &[]).await;
    let spec = |name: &str| ChannelSpec {
        name: name.to_string(),
        components: ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params: json!({ "name": "bobs-policy" }),
                ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
            }],
        },
        participants: vec![],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };

    let (created, asked) =
        world.asked(session.create_channel(&World::as_user(world.alice), spec("side"))).await;
    assert!(created.is_ok(), "shared with her");
    assert_eq!(asked, checks(&[
        "alice: List(component_bundle) yes",
        "alice: GetComponentBundle(bob's) yes",
        "alice: CreateChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: CreateChannel(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
        "alice: List(component_bundle) yes",
        "alice: GetComponentBundle(bob's) yes",
        "alice: JoinChannel(alice) yes",
    ]), "read as she names it, and again as the channel is built in its owner's name - hers; then its founder joins");

    let mut withdrawn =
        ComponentBundle { security_attributes: attributes(Some(world.bob), &[]), ..policy };
    world
        .store
        .entity_stores()
        .unwrap()
        .component_bundles
        .update(&World::as_user(world.bob), &mut withdrawn)
        .await
        .expect("bob owns it");
    let (refused, asked) =
        world.asked(session.create_channel(&World::as_user(world.alice), spec("other"))).await;
    assert!(matches!(
        refused.err().and_then(|e| e.downcast::<SessionError>().ok()),
        Some(SessionError::InvalidComponents(_))
    ));
    assert_eq!(
        asked,
        checks(&["alice: List(component_bundle) yes", "alice: GetComponentBundle(bob's) no"])
    );
    assert!(session.channel_named("other").is_none(), "nothing is created");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A channel whose bundle is withdrawn after it was made is left out as its
/// session opens, and the session opens with its other channels (issue
/// 463): the bundle's owner - a third party - cannot keep it closed. The
/// channel is listed and shown as unavailable, what is sent to it is
/// refused with why, its owner may delete it, and it runs again once the
/// session is opened after the share came back.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_channel_whose_bundle_is_withdrawn_keeps_its_session_from_nothing() {
    let world = World::new().await;
    let shared = ComponentBundle::new(
        ScopedName::new("bobs-policy", Scope::GlobalScope).unwrap(),
        ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("policy", "broadcast_policy")],
        },
        attributes(Some(world.bob), &[world.alice]),
    );
    let bundles = world.store.entity_stores().unwrap().component_bundles;
    bundles.create(&World::as_user(world.bob), &shared).await.expect("bob keeps his policy");
    let session = world.shared_session(&[], &[]).await;
    let alice = World::as_user(world.alice);
    let spec = ChannelSpec {
        name: "side".to_string(),
        components: ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params: json!({ "name": "bobs-policy" }),
                ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
            }],
        },
        participants: vec![],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };
    let side = session.create_channel(&alice, spec).await.expect("shared with her").id();
    let main = session.channel_named("main").expect("runs").id();
    let mut withdrawn =
        ComponentBundle { security_attributes: attributes(Some(world.bob), &[]), ..shared.clone() };
    bundles.update(&World::as_user(world.bob), &mut withdrawn).await.expect("bob owns it");
    world.manager.stop(&alice, session.id()).await.unwrap();

    let (opened, asked) = world.asked(world.manager.open(&alice, session.id())).await;
    let session = opened.expect("opens without the channel");
    assert!(
        asked.contains(&"alice: GetComponentBundle(bob's) no".to_string()),
        "read in its owner's name: {asked:?}"
    );
    assert!(session.channel(main).is_some(), "the other channel runs");
    assert!(session.channel(side).is_none(), "this one does not");
    let refused = session.channel_for(&alice, side).await.expect_err("does not run");
    assert!(
        matches!(
            refused.downcast_ref::<SessionError>(),
            Some(SessionError::ChannelUnavailable { .. })
        ),
        "{refused}"
    );
    assert!(refused.to_string().contains("bobs-policy"), "names what cannot be read: {refused}");
    let bob = World::as_user(world.bob);
    let joining = world
        .manager
        .join(&bob, session.id(), ActorId::User(world.bob), Some(side))
        .await
        .expect_err("does not run");
    assert!(
        matches!(
            joining.downcast_ref::<SessionError>(),
            Some(SessionError::ChannelUnavailable { .. })
        ),
        "nor is it joined: {joining}"
    );
    let carol = World::as_user(UserId::new());
    let hidden = session.channel_for(&carol, side).await.expect_err("not hers to read");
    assert!(
        matches!(hidden.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })),
        "nor that it is unavailable: {hidden}"
    );
    let views =
        world.manager.views(Viewer::new(alice.clone())).session(session.id()).await.unwrap();
    let listed = views.channel(side).await.expect("listed").snapshot(Detail::empty()).await;
    assert!(
        listed.unavailable.is_some_and(|why| why.contains("bobs-policy")),
        "shown as unavailable, with why"
    );
    assert_eq!(
        views.channel(main).await.unwrap().snapshot(Detail::empty()).await.unavailable,
        None
    );

    // Written in place of what was written since (issue 492).
    let mut shared = ComponentBundle { revision: withdrawn.revision, ..shared };
    bundles.update(&World::as_user(world.bob), &mut shared).await.expect("shared again");
    let session = world.manager.open(&alice, session.id()).await.unwrap();
    assert!(session.channel(side).is_some(), "built again as the running session is opened");
    session.channel_for(&alice, side).await.expect("and reached");

    withdrawn.revision = shared.revision;
    bundles.update(&World::as_user(world.bob), &mut withdrawn).await.unwrap();
    world.manager.stop(&alice, session.id()).await.unwrap();
    let session = world.manager.open(&alice, session.id()).await.unwrap();
    session
        .delete_channel(&alice, side)
        .await
        .expect("its owner deletes it though it does not run");
    assert!(
        world
            .manager
            .views(Viewer::new(alice.clone()))
            .session(session.id())
            .await
            .unwrap()
            .channel(side)
            .await
            .is_err(),
        "gone"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Components a channel would not take - a tool, which it never calls - are
/// refused before creating it is asked (issue 337): only what naming them
/// asks is asked, reading the list that brings them; nothing is built in the
/// owner's name, so no secret of theirs is read for it.
#[tokio::test]
async fn a_channel_is_refused_a_tool_before_anything_is_built() {
    let world = World::new().await;
    let tools = ComponentBundle::new(
        ScopedName::new("bobs-tools", Scope::GlobalScope).unwrap(),
        ComponentConfig { component_nodes: vec![ComponentDescriptor::new("echo", "echo_tool")] },
        attributes(Some(world.bob), &[world.alice]),
    );
    world
        .store
        .entity_stores()
        .unwrap()
        .component_bundles
        .create(&World::as_user(world.bob), &tools)
        .await
        .expect("bob keeps his tools");
    let session = world.shared_session(&[], &[]).await;
    let spec = ChannelSpec {
        name: "tooled".to_string(),
        components: ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params: json!({ "name": "bobs-tools" }),
                ..ComponentDescriptor::new("tools", BUNDLE_REF_KIND)
            }],
        },
        participants: vec![],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };

    let (refused, asked) =
        world.asked(session.create_channel(&World::as_user(world.alice), spec)).await;
    let refused = refused.err().and_then(|e| e.downcast::<SessionError>().ok());
    assert!(matches!(&refused, Some(SessionError::InvalidComponents(_))), "{refused:?}");
    assert!(
        refused.is_some_and(|e| e.to_string().contains("'tools'")),
        "named by the ref that brings it"
    );
    assert_eq!(
        asked,
        checks(&["alice: List(component_bundle) yes", "alice: GetComponentBundle(bob's) yes"]),
        "only the bundle read as she names it"
    );
    assert!(session.channel_named("tooled").is_none(), "nothing is created");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A component of a kind only an operator configures (issue 403) - whose
/// params say where what it is given goes - is refused in a request that
/// writes it, before anything is asked, and taken where the request names it
/// through a `ref` to a component bundle the creator may read.
#[tokio::test]
async fn a_new_channel_names_a_component_only_an_operator_configures_through_a_ref() {
    let world = World::with(&[], |registry| {
        registry.register(
            ComponentKind::new("operated_policy").provides::<dyn ResponderPolicy>().operator_only(),
            |_, _| {
                let mut facets = ComponentFacets::empty();
                facets.insert::<dyn ResponderPolicy>(Arc::new(BroadcastPolicy));
                Ok(facets)
            },
        );
    })
    .await;
    let operated = ComponentBundle::new(
        ScopedName::new("operated", Scope::GlobalScope).unwrap(),
        ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("policy", "operated_policy")],
        },
        attributes(None, &[world.alice]),
    );
    world
        .store
        .entity_stores()
        .unwrap()
        .component_bundles
        .create(&World::system(), &operated)
        .await
        .expect("the operator's");
    let session = world.shared_session(&[], &[]).await;
    let spec = |name: &str, policy: ComponentDescriptor| ChannelSpec {
        name: name.to_string(),
        components: ComponentConfig { component_nodes: vec![policy] },
        participants: vec![],
        classification: None,
        language: None,
        uses_workspace: None,
        internal_actors: Vec::new(),
    };

    let (refused, asked) = world
        .asked(session.create_channel(
            &World::as_user(world.alice),
            spec("written", ComponentDescriptor::new("policy", "operated_policy")),
        ))
        .await;
    let refused = refused.err().and_then(|e| e.downcast::<SessionError>().ok());
    assert!(matches!(&refused, Some(SessionError::InvalidComponents(_))), "{refused:?}");
    assert!(asked.is_empty(), "nothing asked: {asked:?}");
    assert!(session.channel_named("written").is_none(), "nothing is created");

    let named = ComponentDescriptor {
        params: json!({ "name": "operated" }),
        ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
    };
    session
        .create_channel(&World::as_user(world.alice), spec("named", named))
        .await
        .expect("named through a ref");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Deleting a channel, in the order of issue 304: reaching the channel as
/// the views do - reading it and being cleared for it (issue 312) -,
/// deleting it - its owner's alone -, then, as the store takes it out of
/// its session and marks it, changing the session and deleting it again
/// (rule 2: the operation asks before it acts, the store as it stores).
/// The group of its participants goes with it in the runtime's name (issue
/// 348): nothing is asked of her for it.
#[tokio::test]
async fn deleting_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    let (refused, asked) =
        world.asked(session.delete_channel(&World::as_user(world.bob), main)).await;
    assert!(denied(&refused.expect_err("not his")));
    assert_eq!(
        asked,
        checks(&[
            "bob: GetChannel(alice's) yes",
            "bob: JoinChannel(bob) yes",
            "bob: DeleteChannel(alice's) no"
        ]),
        "refused before the session is touched"
    );
    assert!(session.channel(main).is_some(), "and the channel runs on");
    let (deleted, asked) =
        world.asked(session.delete_channel(&World::as_user(world.alice), main)).await;
    assert!(deleted.is_ok());
    assert_eq!(
        asked,
        checks(&[
            "alice: GetChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: DeleteChannel(alice's) yes",
            "alice: GetSession(alice's) yes",
            "alice: UpdateSession(alice's) yes",
            "alice: GetChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: DeleteChannel(alice's) yes",
        ])
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Restoring a deleted session: reading it as deleted and restoring it -
/// its owner's alone, not a contributor's; for whoever may not read what
/// was deleted, there is nothing to restore (issue 164). The manager reads
/// it first, as whoever restores it (issue 304), and the store asks again as
/// it restores it (rule 2) - and returns it, so that it is not read back
/// (issue 318). The groups of its participants and of its channel's come
/// back with it in the runtime's name (issue 348): nothing is asked of her
/// for them.
#[tokio::test]
async fn restoring_a_session_needs_owning_it() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    world.manager.delete(&World::as_user(world.alice), session.id()).await.unwrap();
    let (_, asked) =
        world.asked(world.manager.restore(&World::as_user(world.carol), session.id())).await;
    assert_eq!(asked, checks(&["carol: ReadDeletedSession(alice's) no"]));
    let (_, asked) =
        world.asked(world.manager.restore(&World::as_user(world.bob), session.id())).await;
    assert_eq!(
        asked,
        checks(&["bob: ReadDeletedSession(alice's) no"]),
        "a contributor does not see what was deleted"
    );
    let (restored, asked) =
        world.asked(world.manager.restore(&World::as_user(world.alice), session.id())).await;
    assert!(restored.is_ok());
    assert_eq!(
        asked,
        checks(&[
            "alice: ReadDeletedSession(alice's) yes",
            "alice: ReadDeletedSession(alice's) yes",
            "alice: RestoreSession(alice's) yes",
        ]),
        "and does not read it back"
    );
}

/// Purging a deleted session: the system's alone until there is an
/// administrator (issue 125) - its owner may read it as deleted, but not
/// remove it for good (issue 164).
#[tokio::test]
async fn purging_a_session_is_not_its_owners() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    world.manager.delete(&World::as_user(world.alice), session.id()).await.unwrap();
    let (_, asked) =
        world.asked(world.manager.purge(&World::as_user(world.bob), session.id())).await;
    assert_eq!(asked, checks(&["bob: ReadDeletedSession(alice's) no"]));
    let (refused, asked) =
        world.asked(world.manager.purge(&World::as_user(world.alice), session.id())).await;
    assert!(refused.is_err());
    assert_eq!(
        asked,
        checks(&[
            "alice: ReadDeletedSession(alice's) yes",
            "alice: ReadDeletedSession(alice's) yes",
            "alice: PurgeSession(alice's) no",
        ]),
        "only what was deleted, asked by the agent manager and again by the store"
    );
}

/// The deleted channels of a session (issues 164, 472): listed to whoever may read them as deleted,
/// each with whether they may restore it - only with changing the session, which restoring it
/// does - and none purged alone. A channel's owner who may not change the session sees it, and may
/// not restore it.
#[tokio::test]
async fn deleted_channels_are_restored_only_by_who_may_change_their_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    let stores = world.runtime.entity_stores();
    let mut his = stores.channels.get(&World::system(), main).await.unwrap();
    his.security_attributes.owner_id = ActorId::User(world.bob);
    stores.channels.update(&World::system(), &mut his).await.unwrap();
    session.delete_channel(&World::system(), main).await.unwrap();
    let views = |user| world.manager.views(Viewer::new(World::as_user(user)));

    let (listed, asked) = world
        .asked(async {
            views(world.bob).session(session.id()).await.unwrap().deleted_channels().await.unwrap()
        })
        .await;
    assert_eq!(
        listed
            .iter()
            .map(|entry| (entry.id, entry.can_restore, entry.can_purge))
            .collect::<Vec<_>>(),
        [(main, false, false)],
        "his, not his to restore"
    );
    assert_eq!(
        asked,
        checks(&[
            "bob: GetSession(alice's) yes",
            "bob: List(channel) yes",
            "bob: ReadDeletedChannel(bob's) yes",
            "bob: JoinChannel(bob) yes",
            "bob: RestoreChannel(bob's) yes",
            "bob: PurgeChannel(bob's) no",
            "bob: UpdateSession(alice's) no",
        ]),
        "each channel asked as every deleted entry is, the session once"
    );
    let (listed, asked) = world
        .asked(async {
            views(world.alice)
                .session(session.id())
                .await
                .unwrap()
                .deleted_channels()
                .await
                .unwrap()
        })
        .await;
    assert!(
        listed.is_empty(),
        "not shown to who may not read it as deleted, though she may change the session"
    );
    assert_eq!(
        asked,
        checks(&[
            "alice: GetSession(alice's) yes",
            "alice: List(channel) yes",
            "alice: UpdateSession(alice's) yes"
        ])
    );
}

/// Restoring a deleted channel (issue 164), in the order of issue 304:
/// reading it as deleted, as the session lists its deleted channels,
/// restoring it - its owner's alone - before everyone's clearance is asked
/// (issue 312),
/// and the store restoring the channel again and listing it in its
/// session. The group of its participants comes back with it in the
/// runtime's name (issue 348): nothing is asked of her for it.
#[tokio::test]
async fn restoring_a_channel_needs_owning_its_session() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = session.channels()[0].id();
    session.delete_channel(&World::as_user(world.alice), main).await.unwrap();
    let (refused, asked) =
        world.asked(session.restore_channel(&World::as_user(world.carol), main)).await;
    assert!(
        matches!(
            refused.unwrap_err().downcast_ref::<SessionError>(),
            Some(SessionError::ChannelNotFound { .. })
        ),
        "a stranger finds nothing"
    );
    assert_eq!(asked, checks(&["carol: ReadDeletedChannel(alice's) no"]));
    let (refused, asked) =
        world.asked(session.restore_channel(&World::as_user(world.bob), main)).await;
    assert!(
        matches!(
            refused.unwrap_err().downcast_ref::<SessionError>(),
            Some(SessionError::ChannelNotFound { .. })
        ),
        "a participant does not see what was deleted"
    );
    assert_eq!(asked, checks(&["bob: ReadDeletedChannel(alice's) no"]));
    let (restored, asked) =
        world.asked(session.restore_channel(&World::as_user(world.alice), main)).await;
    assert!(restored.is_ok());
    assert_eq!(
        asked,
        checks(&[
            "alice: ReadDeletedChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: RestoreChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: JoinChannel(bob) yes",
            "alice: ReadDeletedChannel(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: RestoreChannel(alice's) yes",
            "alice: GetSession(alice's) yes",
            "alice: UpdateSession(alice's) yes",
        ]),
        "everyone's clearance again, as when it was created, and listing it in the session"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Deleting an agent: reading it and deleting it - its owner's alone, not
/// the owner's of its personality; for whoever may not read it, there is
/// nothing to delete.
#[tokio::test]
async fn deleting_an_agent_needs_owning_it() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let (deleted, asked) = world
        .asked(world.runtime.entity_stores().agents.delete(
            &World::as_user(world.bob),
            helper,
            &Removal::now(ActorId::User(world.bob)),
        ))
        .await;
    assert!(!deleted.unwrap(), "not there for bob, though the personality is his");
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) no"]));
    let (deleted, asked) = world
        .asked(world.runtime.entity_stores().agents.delete(
            &World::as_user(world.alice),
            helper,
            &Removal::now(ActorId::User(world.alice)),
        ))
        .await;
    assert!(deleted.unwrap());
    assert_eq!(asked, checks(&["alice: GetAgent(alice's) yes", "alice: DeleteAgent(alice's) yes"]));
}

/// Deleting an agent through the agent manager (issues 164, 314): reading
/// it and deleting it - its owner's alone - before anything happens; the
/// channels it rests in are changed by nobody. Restoring it is its owner's
/// too, asked by the store alone (issue 318), purging it not.
#[tokio::test]
async fn deleting_restoring_and_purging_an_agent_is_its_owners_but_purging() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let agents = world.runtime.agent_manager();
    let (deleted, asked) = world.asked(agents.delete(&World::as_user(world.bob), helper)).await;
    assert!(!deleted.unwrap(), "not there for bob, though the personality is his");
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) no"]));
    let (deleted, asked) = world.asked(agents.delete(&World::as_user(world.alice), helper)).await;
    assert!(deleted.unwrap());
    assert_eq!(
        asked,
        checks(&[
            "alice: GetAgent(alice's) yes",
            "alice: DeleteAgent(alice's) yes",
            "alice: GetAgent(alice's) yes",
            "alice: DeleteAgent(alice's) yes",
        ]),
        "asked by the agent manager before anything happens, and again by the store"
    );
    let (refused, asked) = world.asked(agents.purge(&World::as_user(world.alice), helper)).await;
    assert!(refused.is_err());
    assert_eq!(
        asked,
        checks(&["alice: ReadDeletedAgent(alice's) yes", "alice: PurgeAgent(alice's) no"])
    );
    let (restored, asked) = world.asked(agents.restore(&World::as_user(world.alice), helper)).await;
    assert!(restored.is_ok());
    assert_eq!(
        asked,
        checks(&["alice: ReadDeletedAgent(alice's) yes", "alice: RestoreAgent(alice's) yes"]),
        "only what was deleted, asked by the store"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Who may not change a channel is refused before anything is asked of
/// the agent they would add (issue 198, F2).
#[tokio::test]
async fn who_may_not_add_to_a_channel_is_refused_before_the_agent_is_looked_at() {
    let world = World::new().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[], &[helper]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (refused, asked) = world
        .asked(main.add_participant(
            &World::as_user(world.bob),
            ActorId::Agent(helper),
            HistoryAccess::All,
        ))
        .await;
    assert!(denied(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["bob: UpdateChannel(alice's) no"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Workspaces (issues 251, 253) ---

/// Ends command `id` as the REST server does (issue 294): through the
/// shell of channel `channel` of `session`, where it was started, as `ctx`
/// reaches it there (`WorkspaceController::shell_in`).
async fn kill(
    session: &SessionLoop,
    channel: ChannelId,
    ctx: &SecurityContext,
    id: &str,
) -> Result<CommandState, WorkspaceError> {
    let (_, shell) = shell_in(session, channel, ctx).await?;
    let shell = shell.ok_or_else(|| TerminalError::UnknownCommand(id.to_string()))?;
    Ok(shell.kill(id, 0).await?)
}

/// A workspace is its owner's alone (issue 251, decision 12): made by a
/// user cleared for its classification, read, changed, deleted and restored by them, by nobody
/// else, each reached as the views reach it - and purged by the system,
/// its files with it.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn a_workspace_is_its_owners_alone() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);

    let (made, asked) = world
        .asked(workspaces.create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            Some("Drafts"),
            internal.clone(),
        ))
        .await;
    let docs = made.expect("alice makes one");
    assert_eq!(
        asked,
        checks(&["alice: JoinChannel(alice) yes", "alice: CreateWorkspace(alice's) yes"]),
        "her own flow, then creating it as it is stored (issue 312)"
    );
    assert!(world.files.path().join(docs.id.to_string()).is_dir(), "with its place");
    let above = workspaces
        .create(
            &alice,
            "secrets",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Confidential),
        )
        .await;
    assert!(
        matches!(above, Err(WorkspaceError::NotCleared)),
        "not above what she is cleared for: {above:?}"
    );
    let elsewhere = workspaces
        .create(
            &alice,
            "other",
            Scope::UserScope { user_id: world.alice },
            "nowhere",
            None,
            internal.clone(),
        )
        .await;
    assert!(
        matches!(elsewhere, Err(WorkspaceError::UnknownStorage(_))),
        "nor in a storage that is not configured: {elsewhere:?}"
    );

    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));
    assert_eq!(views(&alice).workspaces().await.unwrap().len(), 1, "hers");
    let (listed, asked) = world.asked(views(&bob).workspaces()).await;
    assert!(listed.unwrap().is_empty(), "not his");
    assert_eq!(
        asked,
        checks(&["bob: List(workspace) yes"]),
        "nor asked about: the list is narrowed to what he is admitted to (issue 332)"
    );
    let (hidden, asked) = world.asked(views(&bob).workspace(docs.id)).await;
    assert!(matches!(hidden, Err(ViewError::WorkspaceNotFound(_))));
    assert_eq!(asked, checks(&["bob: GetWorkspace(alice's) no"]));

    let renamed = workspace(&world, docs.id).await.update_name(&bob, "mine").await;
    assert!(
        matches!(renamed, Err(WorkspaceError::NotFound(_))),
        "bob does not rename it: {renamed:?}"
    );
    let (renamed, asked) =
        world.asked(workspace(&world, docs.id).await.update_name(&alice, "notes")).await;
    renamed.expect("alice does");
    assert_eq!(workspace(&world, docs.id).await.load().name.name, "notes");
    assert_eq!(
        asked,
        checks(&[
            "alice: GetWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: GetWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: UpdateWorkspace(alice's) yes"
        ]),
        "reached, then reached again and changed by the store (issue 322)"
    );

    assert!(!workspaces.delete(&bob, docs.id).await.unwrap(), "nothing bob could see to delete");
    assert!(workspaces.delete(&alice, docs.id).await.unwrap());
    assert!(
        matches!(workspaces.restore(&bob, docs.id).await, Err(WorkspaceError::NotFound(_))),
        "nor restore"
    );
    let (restored, asked) = world.asked(workspaces.restore(&alice, docs.id)).await;
    assert_eq!(restored.expect("alice restores it").id, docs.id);
    assert_eq!(
        asked,
        checks(&[
            "alice: ReadDeletedWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: RestoreWorkspace(alice's) yes"
        ]),
        "reached by the store (issue 322), and not read back (issue 318)"
    );
    assert!(workspaces.delete(&alice, docs.id).await.unwrap());
    let (purged, asked) = world.asked(workspaces.purge(&alice, docs.id)).await;
    assert!(
        matches!(purged, Err(WorkspaceError::Store(StoreError::PermissionDenied(_)))),
        "the owner does not purge: {purged:?}"
    );
    assert_eq!(
        asked,
        checks(&[
            "alice: ReadDeletedWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: PurgeWorkspace(alice's) no"
        ])
    );
    assert!(world.files.path().join(docs.id.to_string()).is_dir(), "and the files stay");
    assert!(workspaces.purge(&World::system(), docs.id).await.unwrap(), "the system does");
    assert!(!world.files.path().join(docs.id.to_string()).exists(), "with the files");
}

/// A workspace is reached as the views reach it (issue 304): read and
/// cleared for, else it is not found - before anything else is asked, so
/// that whoever is not cleared for it learns nothing of it, not even that
/// they may not change its files; by its id as by navigating to it (issue
/// 312), and once deleted - all of it decided by the store (issue 322).
#[tokio::test]
async fn a_workspace_one_is_not_cleared_for_is_not_found() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let alice = World::as_user(world.alice);
    let docs = workspaces
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .expect("hers");

    // Alice without the role that clears her for internal.
    let uncleared =
        SecurityContext::new(ActorId::User(world.alice), Default::default(), Default::default());
    let (reached, asked) =
        world.asked(workspace(&world, docs.id).await.terminal(&uncleared, Grant::Write)).await;
    assert!(
        matches!(reached, Err(WorkspaceError::NotFound(id)) if id == docs.id),
        "not found, not refused: {:?}",
        reached.err()
    );
    assert_eq!(
        asked,
        checks(&["alice: GetWorkspace(alice's) yes", "alice: JoinChannel(alice) no"]),
        "read, not cleared - and not asked whether she may change it"
    );
    let (reached, asked) =
        world.asked(workspace(&world, docs.id).await.terminal(&alice, Grant::Write)).await;
    assert!(reached.is_ok(), "cleared, she reaches it");
    assert_eq!(
        asked,
        checks(&[
            "alice: GetWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: WriteWorkspace(alice's) yes"
        ]),
        "read, cleared, then the verb"
    );

    // Named by its id, it is reached alike (issue 312): its owner, no
    // longer cleared, neither renames, shares nor deletes it.
    let (renamed, asked) =
        world.asked(workspace(&world, docs.id).await.update_name(&uncleared, "mine")).await;
    assert!(matches!(renamed, Err(WorkspaceError::NotFound(_))), "{renamed:?}");
    assert_eq!(
        asked,
        checks(&["alice: GetWorkspace(alice's) yes", "alice: JoinChannel(alice) no"])
    );
    let shared = workspace(&world, docs.id)
        .await
        .share(&uncleared, Subject::Actor(ActorId::User(world.bob)), Grant::Read)
        .await;
    assert!(matches!(shared, Err(WorkspaceError::NotFound(_))), "{shared:?}");
    assert!(
        !workspaces.delete(&uncleared, docs.id).await.unwrap(),
        "nothing she reaches to delete"
    );

    // Deleted, it is reached by the store alike (issue 322): the list of
    // what was deleted and restoring agree.
    workspaces.delete(&alice, docs.id).await.unwrap();
    let listed =
        world.manager.views(Viewer::new(uncleared.clone())).deleted_workspaces().await.unwrap();
    assert!(listed.is_empty(), "not among what she deleted");
    let (restored, asked) = world.asked(workspaces.restore(&uncleared, docs.id)).await;
    assert!(matches!(restored, Err(WorkspaceError::NotFound(_))), "nor to restore");
    assert_eq!(
        asked,
        checks(&["alice: ReadDeletedWorkspace(alice's) yes", "alice: JoinChannel(alice) no"])
    );
    assert_eq!(
        world.manager.views(Viewer::new(alice.clone())).deleted_workspaces().await.unwrap().len(),
        1,
        "where it is, cleared"
    );
}

/// A storage takes what may flow to where it puts it (issue 251, part B2):
/// a workspace classified above its destination is not made there, nor
/// reclassified above it - asked as publishing there, before the
/// clearance is.
#[tokio::test]
async fn a_storage_holds_no_workspace_above_its_destination() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let alice = World::as_user(world.alice);
    let mine = Scope::UserScope { user_id: world.alice };

    let (above, asked) = world
        .asked(workspaces.create(
            &alice,
            "secrets",
            mine.clone(),
            "shell",
            None,
            Classification::general(Confidentiality::Confidential),
        ))
        .await;
    assert!(
        matches!(&above, Err(WorkspaceError::StorageNotCleared(storage)) if storage == "shell"),
        "{above:?}"
    );
    assert_eq!(asked, checks(&["alice: Publish no"]));

    let (made, asked) = world
        .asked(workspaces.create(
            &alice,
            "docs",
            mine,
            "shell",
            None,
            Classification::general(Confidentiality::Internal),
        ))
        .await;
    let docs = made.expect("up to it");
    assert_eq!(
        asked,
        checks(&[
            "alice: Publish yes",
            "alice: JoinChannel(alice) yes",
            "alice: CreateWorkspace(alice's) yes"
        ])
    );
    let raised = workspace(&world, docs.id)
        .await
        .update_classification(&alice, Classification::general(Confidentiality::Confidential))
        .await;
    assert!(matches!(raised, Err(WorkspaceError::StorageNotCleared(_))), "{raised:?}");
}

/// Attaching a workspace is its owner's act, into a session they may change
/// (issue 251, decision 12); it shares the workspace with the session's
/// participants only where asked to, by an entry of its access list (issue 23).
/// Taking it away needs changing the session alone, and leaves the list.
#[tokio::test]
#[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
async fn only_its_owner_attaches_a_workspace_to_a_session_they_may_change() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);
    let hers = workspaces
        .create(
            &alice,
            "hers",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            internal.clone(),
        )
        .await
        .unwrap();
    let his = workspaces
        .create(&bob, "his", Scope::UserScope { user_id: world.bob }, "main", None, internal)
        .await
        .unwrap();
    let session = world.shared_session(&[], &[]).await;

    let (refused, asked) = world.asked(session.update_workspace(&bob, Some(his.id), None)).await;
    assert!(denied(&refused.unwrap_err()), "bob's own, into alice's session");
    assert_eq!(
        asked,
        checks(&["bob: UpdateSession(alice's) no"]),
        "refused before his workspace is asked about"
    );
    let (refused, asked) = world.asked(session.update_workspace(&alice, Some(his.id), None)).await;
    assert!(not_found(&refused.unwrap_err()), "bob's, by alice");
    assert_eq!(
        asked,
        checks(&["alice: UpdateSession(alice's) yes", "alice: GetWorkspace(bob's) no"])
    );
    let stores = world.runtime.entity_stores();
    let mut lent = stores.workspaces.get(&World::system(), his.id).await.unwrap();
    lent.security_attributes =
        lent.security_attributes.granting(Subject::Actor(ActorId::User(world.alice)), Grant::Read);
    stores.workspaces.update(&World::system(), &mut lent).await.unwrap();
    let (refused, asked) = world.asked(session.update_workspace(&alice, Some(his.id), None)).await;
    assert!(denied(&refused.unwrap_err()), "bob's, which alice reads but may not attach");
    assert_eq!(
        asked,
        checks(&[
            "alice: UpdateSession(alice's) yes",
            "alice: GetWorkspace(bob's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: UpdateWorkspace(bob's) no"
        ]),
        "refused as its owner's act, before the session is stored"
    );

    let (attached, asked) =
        world.asked(session.update_workspace(&alice, Some(hers.id), None)).await;
    attached.expect("hers, into her session");
    assert_eq!(
        asked,
        checks(&[
            "alice: UpdateSession(alice's) yes",
            "alice: GetWorkspace(alice's) yes",
            "alice: JoinChannel(alice) yes",
            "alice: UpdateWorkspace(alice's) yes",
            "alice: GetSession(alice's) yes",
            "alice: UpdateSession(alice's) yes"
        ])
    );
    assert_eq!(session.session().workspace, Some(hers.id));
    let members = Subject::Group(GroupId::of_session(session.id()));
    assert!(
        world
            .runtime
            .entity_stores()
            .workspaces
            .get(&alice, hers.id)
            .await
            .unwrap()
            .security_attributes
            .acl
            .is_empty(),
        "attaching alone shares nothing"
    );

    session
        .update_workspace(&alice, Some(hers.id), Some(Grant::Read))
        .await
        .expect("shared with its members to read");
    let acl = world
        .runtime
        .entity_stores()
        .workspaces
        .get(&alice, hers.id)
        .await
        .unwrap()
        .security_attributes
        .acl;
    assert_eq!(acl, [AclEntry { subject: members, grant: Grant::Read }]);
    session
        .update_workspace(&alice, Some(hers.id), Some(Grant::Write))
        .await
        .expect("and widened to write");
    let acl = world
        .runtime
        .entity_stores()
        .workspaces
        .get(&alice, hers.id)
        .await
        .unwrap()
        .security_attributes
        .acl;
    assert_eq!(acl, [AclEntry { subject: members, grant: Grant::Write }], "one entry, widened");

    assert!(
        denied(&session.update_workspace(&bob, None, None).await.unwrap_err()),
        "bob does not take it away"
    );
    session.update_workspace(&alice, None, None).await.expect("alice does");
    assert_eq!(session.session().workspace, None);
    assert_eq!(
        world
            .runtime
            .entity_stores()
            .workspaces
            .get(&alice, hers.id)
            .await
            .unwrap()
            .security_attributes
            .acl
            .len(),
        1,
        "the list is the workspace's, and stays"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// In a session, a workspace is used through the channels (issue 251,
/// decisions 10, 11): by a current participant - not its owner alone - whom its
/// access list lets (issue 23), where the channel uses it and it and the
/// channel flow both ways; nobody else, and nothing says why. What the list
/// grants is reading, or changing the files too.
#[tokio::test]
async fn a_workspace_is_used_in_a_channel_by_its_participants_as_its_list_lets_them_where_both_flow(
) {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = workspaces
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), None).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let in_main = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.session(session_id).await.unwrap().channel(main).await.unwrap() }
    };

    assert!(
        in_main(&bob).await.workspace().await.unwrap().is_none(),
        "not shared: not his to use, though attached"
    );
    workspace(&world, docs.id)
        .await
        .share(&alice, Subject::Group(GroupId::of_session(session_id)), Grant::Read)
        .await
        .unwrap();
    let (usable, asked) =
        world.asked(async { in_main(&bob).await.workspace().await.unwrap() }).await;
    let bobs = usable.expect("bob, a member of the channel, uses alice's workspace there");
    assert!(
        asked.ends_with(&checks(&[
            "bob: GetWorkspace(alice's) yes",
            "bob: Publish yes",
            "bob: Publish yes"
        ])),
        "its list, then both ways: {asked:?}"
    );
    assert_eq!(bobs.id(), docs.id);
    let his = |need| terminal_in(&session, main, &bob, need);
    assert!(
        matches!(his(Grant::Write).await, Err(WorkspaceError::ReadOnly { .. })),
        "reading, not changing, where it grants reading"
    );
    workspace(&world, docs.id)
        .await
        .share(&alice, Subject::Group(GroupId::of_session(session_id)), Grant::Write)
        .await
        .unwrap();
    let (_, terminal) = his(Grant::Write).await.expect("and reaches its files");
    terminal
        .write(&WorkspacePath::parse("notes/a.md").unwrap(), file("from bob"), 100)
        .await
        .unwrap();
    let owned = workspace(&world, docs.id).await.terminal(&alice, Grant::Read).await.unwrap();
    assert_eq!(
        read(owned.as_ref(), "notes/a.md").await,
        "from bob",
        "the same files as its owner's"
    );

    let channel = Arc::clone(&session.channels()[0]);
    assert!(
        denied(&channel.update_uses_workspace(&bob, false).await.unwrap_err()),
        "a member does not switch it off"
    );
    channel.update_uses_workspace(&alice, false).await.expect("the channel's owner does");
    assert!(
        in_main(&bob).await.workspace().await.unwrap().is_none(),
        "not where the channel does not use it"
    );
    assert!(
        matches!(his(Grant::Read).await, Err(WorkspaceError::NotUsableInChannel)),
        "asked again when used"
    );
    channel.update_uses_workspace(&alice, true).await.unwrap();

    workspace(&world, docs.id)
        .await
        .update_classification(&alice, Classification::PUBLIC)
        .await
        .unwrap();
    assert!(
        in_main(&bob).await.workspace().await.unwrap().is_none(),
        "not where what is seen may not flow into it"
    );
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Issue 254: a command is its channel's, and runs on only as long as some
/// current participant could start it there. The runtime asks so on its own, in
/// each participant's name, at every event that can change the answer - the
/// first participant who may suffices; once nobody may, it ends the command.
#[tokio::test]
async fn a_command_runs_on_only_while_some_participant_could_start_it_in_its_channel() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let alice = World::as_user(world.alice);
    let docs = workspaces
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
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Read)).await.unwrap();
    let (_, shell) = shell_in(&session, session.channels()[0].id(), &alice).await.unwrap();
    let shell = shell.expect("a shell");
    let id = shell.start("sleep 30").await.unwrap();

    // Asked again at a change, of the participants in turn: alice may.
    let (_, asked) = world
        .recorded(async {
            workspace(&world, docs.id)
                .await
                .update_description(&alice, Some("Drafts"))
                .await
                .unwrap();
            while !world
                .access_policy
                .asked
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("WriteWorkspace"))
            {
                time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    let reaching = |who: &str| {
        [
            format!("{who}, asked by system: GetSession(alice's) yes"),
            format!("{who}, asked by system: GetChannel(alice's) yes"),
            format!("{who}, asked by system: JoinChannel({who}) yes"),
        ]
    };
    let may = [
        &reaching("alice")[..],
        &checks(&[
            "alice, asked by system: GetWorkspace(alice's) yes",
            "alice, asked by system: Publish yes",
            "alice, asked by system: Publish yes",
            "alice, asked by system: WriteWorkspace(alice's) yes",
        ]),
    ]
    .concat();
    // How often it is asked depends on the events it sees; what, does not.
    let rounds = |asked: &[String], round: &[String]| {
        !asked.is_empty() && asked.chunks(round.len()).all(|one| one == round)
    };
    let asked: Vec<String> =
        asked.into_iter().filter(|line| line.contains("asked by system")).collect();
    assert!(rounds(&asked, &may), "{asked:#?}");
    assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Running);

    // Once the channel no longer uses the workspace, nobody may.
    let channel = Arc::clone(&session.channels()[0]);
    let (ended, asked) = world
        .recorded(async {
            channel.update_uses_workspace(&alice, false).await.unwrap();
            shell.output(&id, 0, Duration::from_secs(10)).await.unwrap()
        })
        .await;
    assert_eq!(ended.status, CommandStatus::Killed);
    let asked: Vec<String> =
        asked.into_iter().filter(|line| line.contains("asked by system")).collect();
    let nobody = [reaching("alice"), reaching("bob")].concat();
    let (before, last) = asked.split_at(asked.len().saturating_sub(nobody.len()));
    assert!(last == nobody && (before.is_empty() || rounds(before, &may)), "{asked:#?}");

    // Stopping the session ends what runs in its channels, asking nothing
    // besides what stopping it asks.
    channel.update_uses_workspace(&alice, true).await.unwrap();
    let (_, shell) = shell_in(&session, channel.id(), &alice).await.unwrap();
    let shell = shell.expect("a shell");
    let id = shell.start("sleep 30").await.unwrap();
    let (stopped, asked) = world.asked(world.manager.stop(&alice, session.id())).await;
    assert!(stopped.unwrap());
    assert_eq!(
        asked,
        checks(&["alice: GetSession(alice's) yes", "alice: StopSession(alice's) yes"])
    );
    assert_eq!(shell.output(&id, 0, Duration::ZERO).await.unwrap().status, CommandStatus::Killed);
}

/// Issue 294: a command an agent's call handed back as running is ended by a
/// user through the workspace as the channel offers it - by whoever may
/// change its files there, who could have started the command; not by who
/// only reads them. A command is seen only in the channel it was started
/// in, and ended only there (issues 296, 344: by the workspace the channel
/// offers, asked as the views ask it).
#[tokio::test]
async fn a_command_is_ended_in_its_channel_by_who_may_change_the_workspace_there() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = workspaces
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
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Read)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    // As `run_command` starts them: in the shell of the channel.
    let (_, shell) = shell_in(&session, main, &alice).await.unwrap();
    let started = shell.expect("a shell").start("sleep 30").await.unwrap();
    let terminal = workspace(&world, docs.id).await.terminal(&alice, Grant::Write).await.unwrap();
    let other = terminal.shell(&ChannelId::new().to_string()).expect("a shell");
    let elsewhere = other.start("sleep 30").await.unwrap();

    let (refused, asked) = world.asked(kill(&session, main, &bob, &started)).await;
    assert!(
        matches!(refused, Err(WorkspaceError::ReadOnly { .. })),
        "not by who only reads its files: {refused:?}"
    );
    // Asked as it is now, as the views ask it: the session, the channel and
    // whether he is cleared for it, the workspace as the channel offers it,
    // then whether he may change its files.
    let reached = [
        "bob: GetSession(alice's) yes",
        "bob: GetChannel(alice's) yes",
        "bob: JoinChannel(bob) yes",
        "bob: GetWorkspace(alice's) yes",
        "bob: Publish yes",
        "bob: Publish yes",
    ];
    assert_eq!(asked, checks(&[&reached[..], &["bob: WriteWorkspace(alice's) no"]].concat()));
    workspace(&world, docs.id)
        .await
        .share(&alice, Subject::Group(GroupId::of_session(session_id)), Grant::Write)
        .await
        .unwrap();
    let (killed, asked) = world.asked(kill(&session, main, &bob, &started)).await;
    assert_eq!(killed.expect("by who may change them").status, CommandStatus::Killed);
    assert_eq!(asked, checks(&[&reached[..], &["bob: WriteWorkspace(alice's) yes"]].concat()));
    let unknown = kill(&session, main, &bob, &elsewhere).await;
    assert!(
        matches!(unknown, Err(WorkspaceError::Terminal(TerminalError::UnknownCommand(_)))),
        "not one of another channel: {unknown:?}"
    );
    other.kill(&elsewhere, 0).await.unwrap();
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Issue 23: a workspace is reached by its access list, wherever one acts -
/// not through a channel: who it is shared with reads it, or changes its
/// files as far as the list grants; only its owner shares it, never with an
/// agent; who it is not shared with does not learn it exists.
#[tokio::test]
async fn a_workspace_is_reached_by_its_list_wherever_one_acts() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let alice = World::as_user(world.alice);
    let bob = World::as_user(world.bob);
    let carol = World::as_user(world.carol);
    let docs = workspaces
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));
    assert!(
        matches!(views(&bob).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))),
        "not shared: not there for him"
    );

    let bobs = SecurityAttributes::owned_by(ActorId::User(world.alice))
        .granting(Subject::Actor(ActorId::User(world.bob)), Grant::Read)
        .acl;
    let (refused, asked) =
        world.asked(workspace(&world, docs.id).await.update_access(&bob, bobs.clone())).await;
    assert!(
        matches!(refused, Err(WorkspaceError::NotFound(_))),
        "nobody shares what they cannot read: {refused:?}"
    );
    assert_eq!(asked, checks(&["bob: GetWorkspace(alice's) no"]));
    let (shared, asked) =
        world.asked(workspace(&world, docs.id).await.update_access(&alice, bobs.clone())).await;
    shared.expect("its owner shares it");
    assert_eq!(asked, checks(&["alice: GetWorkspace(alice's) yes", "alice: JoinChannel(alice) yes", "alice: GetWorkspace(alice's) yes", "alice: JoinChannel(alice) yes", "alice: UpdateWorkspace(alice's) yes"]),
        "reached, then reached again and changed as stored: whether she reads a group it names tells her nothing of others (issue 312)");

    let elsewhere =
        SecurityContext::new(ActorId::User(world.bob), SessionId::new(), ChannelId::new())
            .with_roles(vec![*STAFF], None);
    let his = views(&elsewhere).workspace(docs.id).await.expect("he reads it, wherever he acts");
    workspace(&world, docs.id).await.terminal(&elsewhere, Grant::Read).await.expect("its files");
    assert!(
        matches!(
            workspace(&world, docs.id).await.terminal(&elsewhere, Grant::Write).await,
            Err(WorkspaceError::ReadOnly { .. })
        ),
        "not to change them"
    );
    let seen = his.snapshot().await;
    assert!(
        !seen.can_write && !seen.can_change && seen.access.is_empty(),
        "nor whom else it is shared with"
    );
    let listed: Vec<WorkspaceId> =
        views(&bob).workspaces().await.unwrap().iter().map(|w| w.id()).collect();
    assert_eq!(listed, [docs.id], "listed among his");
    let owners = views(&alice).workspace(docs.id).await.unwrap().snapshot().await;
    assert_eq!(
        owners.access.iter().map(|e| e.name.as_deref()).collect::<Vec<_>>(),
        [Some("bob")],
        "its owner sees whom, by name"
    );
    assert!(
        matches!(
            workspace(&world, docs.id).await.update_access(&bob, Vec::new()).await,
            Err(WorkspaceError::Store(StoreError::PermissionDenied(_)))
        ),
        "a reader does not change the list"
    );
    assert!(
        matches!(views(&carol).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))),
        "nor is it there for carol"
    );

    let writers = SecurityAttributes::owned_by(ActorId::User(world.alice))
        .granting(Subject::Actor(ActorId::User(world.bob)), Grant::Write)
        .acl;
    workspace(&world, docs.id).await.update_access(&alice, writers).await.unwrap();
    workspace(&world, docs.id)
        .await
        .terminal(&bob, Grant::Write)
        .await
        .expect("where it grants writing, he writes");
    let agent = SecurityAttributes::owned_by(ActorId::User(world.alice))
        .granting(Subject::Actor(ActorId::Agent(AgentId::new())), Grant::Read)
        .acl;
    assert!(
        matches!(
            workspace(&world, docs.id).await.update_access(&alice, agent).await,
            Err(WorkspaceError::Store(StoreError::Validation(_)))
        ),
        "never with an agent (issue 255)"
    );
}

/// G1 (issue 286): a team's members read what is shared with it, wherever
/// they act; who leaves it no longer does, and a stranger never did. Its
/// owner changes it; a participant reads it but changes nothing, and finds it
/// among his teams, which are asked for alone (issue 317). Deleted, it grants
/// nothing until it is restored, though its members hear that it went
/// (issue 332).
#[tokio::test]
async fn what_is_shared_with_a_team_is_read_by_its_members_while_they_are_members() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let bob = World::as_user(world.bob);
    let carol = World::as_user(world.carol);
    let groups = world.runtime.group_manager();
    let workspaces = world.runtime.workspace_manager();
    let views = |ctx: &SecurityContext| world.manager.views(Viewer::new(ctx.clone()));

    let project = groups
        .create(&alice, "project", Scope::UserScope { user_id: world.alice })
        .await
        .expect("alice makes a team");
    let (made, asked) = world.asked(groups.set_members(&alice, project.id, vec![world.bob])).await;
    made.expect("and names its members");
    assert_eq!(asked, checks(&["alice: GetGroup(alice's) yes", "alice: UpdateGroup(alice's) yes", "alice: GetGroup(alice's) yes", "alice: UpdateGroup(alice's) yes"]),
        "read, changed - before whether its members exist is asked (issue 312) -, then changed as stored");
    let (refused, asked) =
        world.asked(groups.set_members(&bob, project.id, vec![world.bob, world.carol])).await;
    assert!(
        matches!(refused, Err(GroupError::Store(StoreError::PermissionDenied(_)))),
        "a member reads it but changes nothing: {refused:?}"
    );
    assert_eq!(
        asked,
        checks(&["bob: GetGroup(alice's) yes", "bob: UpdateGroup(alice's) no"]),
        "refused before he learns whether carol exists"
    );
    assert!(
        matches!(views(&carol).group(project.id).await, Err(ViewError::GroupNotFound(_))),
        "a stranger does not find it"
    );
    groups
        .create(&carol, "hers", Scope::UserScope { user_id: world.carol })
        .await
        .expect("carol makes one of her own");
    let (listed, asked) = world.asked(views(&bob).groups()).await;
    let listed: Vec<GroupId> = listed.unwrap().iter().map(|g| g.id()).collect();
    assert_eq!(listed, [project.id], "listed among his");
    assert_eq!(
        asked,
        checks(&["bob: List(group) yes", "bob: GetGroup(alice's) yes"]),
        "asked of the teams he owns or belongs to alone (issue 317): {asked:?}"
    );

    let docs = workspaces
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    workspace(&world, docs.id)
        .await
        .share(&alice, Subject::Group(project.id), Grant::Read)
        .await
        .expect("shared with the team");
    let elsewhere =
        SecurityContext::new(ActorId::User(world.bob), SessionId::new(), ChannelId::new())
            .with_roles(vec![*STAFF], None);
    views(&elsewhere).workspace(docs.id).await.expect("a member reads it, wherever he acts");
    assert!(
        matches!(views(&carol).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))),
        "a stranger does not"
    );
    let named = views(&alice).workspace(docs.id).await.unwrap().snapshot().await;
    assert_eq!(
        named.access.iter().map(|e| e.name.as_deref()).collect::<Vec<_>>(),
        [Some("project")],
        "its owner sees the team by its name"
    );

    let mut bobs = world.runtime.event_bus().subscribe(bob.clone(), vec![SubscriptionTopic::All]);
    groups.set_members(&alice, project.id, Vec::new()).await.unwrap();
    assert!(
        matches!(views(&bob).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))),
        "who left no longer reads it"
    );
    // But learns that he left (issue 23).
    hears(
        &mut bobs,
        [GlobalEvent::updated::<Group>(&project.id, &GroupUpdateEvent::MembersChanged)],
    )
    .await;

    groups.set_members(&alice, project.id, vec![world.bob]).await.unwrap();
    groups.delete(&alice, project.id).await.unwrap();
    assert!(
        matches!(views(&elsewhere).workspace(docs.id).await, Err(ViewError::WorkspaceNotFound(_))),
        "a team deleted grants nothing"
    );
    hears(&mut bobs, [GlobalEvent::deleted::<Group>(&project.id)]).await;
    groups.restore(&alice, project.id).await.unwrap();
    views(&elsewhere).workspace(docs.id).await.expect("restored, it grants again");
}

/// G2 (issue 286): whoever names a group in an access list must read it -
/// asked after their own permission to change the list, and only of a group
/// the list did not name before.
#[tokio::test]
async fn a_group_is_named_in_an_access_list_only_by_who_reads_it() {
    let world = World::new().await;
    let (alice, carol) = (World::as_user(world.alice), World::as_user(world.carol));
    let project = world
        .runtime
        .group_manager()
        .create(&alice, "project", Scope::UserScope { user_id: world.alice })
        .await
        .unwrap();
    let workspaces = world.runtime.workspace_manager();
    let hers = workspaces
        .create(
            &carol,
            "notes",
            Scope::UserScope { user_id: world.carol },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let naming = vec![AclEntry { subject: Subject::Group(project.id), grant: Grant::Read }];
    let (refused, asked) =
        world.asked(workspace(&world, hers.id).await.update_access(&carol, naming)).await;
    assert!(
        matches!(refused, Err(WorkspaceError::Store(StoreError::Validation(_)))),
        "carol cannot read alice's team: {refused:?}"
    );
    assert_eq!(
        asked,
        checks(&[
            "carol: GetWorkspace(carol's) yes",
            "carol: JoinChannel(carol) yes",
            "carol: GetGroup(alice's) no"
        ]),
        "reached, then the group: her own question"
    );
}

/// G3 (issue 286): the participants of a channel are a group that follows
/// them - who is removed from the channel no longer reads it, though still
/// in the session; added again, they do.
#[tokio::test]
async fn the_group_of_a_channels_participants_follows_who_takes_part() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let readable = |user: UserId| {
        let manager = Arc::clone(&world.manager);
        let (session_id, channel_id) = (session.id(), main.id());
        async move {
            manager
                .views(Viewer::new(World::as_user(user)))
                .session(session_id)
                .await
                .unwrap()
                .channel(channel_id)
                .await
                .is_ok()
        }
    };
    assert!(readable(world.bob).await, "bob takes part, and reads it");
    let group = world
        .runtime
        .entity_stores()
        .groups
        .get(&World::system(), GroupId::of_channel(main.id()))
        .await
        .expect("the channel's participants are a group");
    assert!(group.has_member(ActorId::User(world.bob)));

    main.remove_participant(&World::as_user(world.alice), ActorId::User(world.bob))
        .await
        .expect("alice removes him");
    assert!(!readable(world.bob).await, "removed, he no longer reads it");
    let group = world
        .runtime
        .entity_stores()
        .groups
        .get(&World::system(), GroupId::of_channel(main.id()))
        .await
        .unwrap();
    assert!(!group.has_member(ActorId::User(world.bob)), "the group followed");
    let refused = world
        .runtime
        .group_manager()
        .set_members(&World::as_user(world.alice), group.id, vec![world.bob])
        .await;
    assert!(
        matches!(refused, Err(GroupError::NotATeam(_))),
        "nor is it changed by hand, not even by its owner: {refused:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Who takes part in the session but not in the channel does not use the
/// channel's workspace there - an agent of alice's waiting in the session,
/// say.
#[tokio::test]
async fn a_workspace_is_not_used_in_a_channel_by_who_is_not_in_it() {
    let world = World::new().await;
    let global = world.personality("global", None, &[]).await;
    let waiting = world.agent("waiting", world.alice, &global).await;
    let alice = World::as_user(world.alice);
    let docs = world
        .runtime
        .workspace_manager()
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let session = world.shared_session(&[], &[waiting]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    // Continued for the agent with its owner's roles, as its controller gives them (issue 304).
    let as_agent = alice
        .next(ActorId::Agent(waiting), ActorId::User(world.alice), ActorId::User(world.alice))
        .with_roles(vec![*STAFF], None);
    let main = world
        .manager
        .views(Viewer::new(as_agent))
        .session(session.id())
        .await
        .unwrap()
        .channel(session.channels()[0].id())
        .await
        .unwrap();
    assert!(main.workspace().await.unwrap().is_none());
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Waits until `events` has received each of `wanted`, in any order -
/// compared as they are shown, since an event has no equality.
async fn hears<const N: usize>(events: &mut EventSubscription, wanted: [GlobalEvent; N]) {
    let mut missing: Vec<String> = wanted.iter().map(|event| format!("{event:?}")).collect();
    let _ = time::timeout(Duration::from_secs(5), async {
        while !missing.is_empty() {
            let Some(event) = events.recv().await else { break };
            let shown = format!("{event:?}");
            missing.retain(|wanted| *wanted != shown);
        }
    })
    .await;
    assert!(missing.is_empty(), "not heard: {missing:?}");
}

/// Issue 319: the groups of the participants of a session, deleted and
/// restored with it - in the runtime's name, issue 348 -, are announced as
/// a team is - to bob, who belongs to them.
#[tokio::test]
async fn the_groups_of_a_session_go_and_come_back_with_it_announced() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let session = world.shared_session(&[], &[]).await;
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (of_session, of_channel) = (GroupId::of_session(session_id), GroupId::of_channel(main));

    let mut bobs = world
        .runtime
        .event_bus()
        .subscribe(World::as_user(world.bob), vec![SubscriptionTopic::All]);
    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    hears(
        &mut bobs,
        [
            GlobalEvent::deleted::<Group>(&of_session),
            GlobalEvent::deleted::<Group>(&of_channel),
            GlobalEvent::deleted::<Channel>(&(session_id, main)),
            GlobalEvent::deleted::<Session>(&session_id),
        ],
    )
    .await;

    world.manager.restore(&alice, session_id).await.unwrap();
    hears(
        &mut bobs,
        [GlobalEvent::restored::<Group>(&of_session), GlobalEvent::restored::<Group>(&of_channel)],
    )
    .await;
}

/// Issue 348: the groups of the participants of a session go and come
/// back with it in the runtime's name, whatever whoever deletes and
/// restores it may do with groups: one that is not alice's to delete or
/// restore - bob's here, as another policy might have it - neither keeps
/// her session nor stays deleted when it is back, and bob reaches the
/// session by it again.
#[tokio::test]
async fn the_group_of_a_sessions_participants_follows_it_whoever_may_delete_groups() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let system = World::system();
    let session_id = world.shared_session(&[], &[]).await.id();
    let groups = &world.runtime.entity_stores().groups;
    let mut of_session = groups.get(&system, GroupId::of_session(session_id)).await.unwrap();
    of_session.security_attributes.owner_id = ActorId::User(world.bob);
    groups.update(&system, &mut of_session).await.unwrap();
    world.manager.stop(&alice, session_id).await.unwrap();

    let (deleted, asked) = world.asked(world.manager.delete(&alice, session_id)).await;
    assert!(deleted.expect("hers to delete"), "deleted");
    assert!(
        !asked.iter().any(|line| line.contains("Group")),
        "nothing asked of her about the groups: {asked:?}"
    );
    assert!(groups.get_deleted(&system, of_session.id).await.is_ok(), "the group went with it");

    let (restored, asked) = world.asked(world.manager.restore(&alice, session_id)).await;
    restored.expect("hers to restore");
    assert!(
        !asked.iter().any(|line| line.contains("Group")),
        "nothing asked of her about the groups: {asked:?}"
    );
    assert!(groups.get(&system, of_session.id).await.is_ok(), "and came back with it");
    assert!(
        world
            .runtime
            .entity_stores()
            .sessions
            .get(&World::as_user(world.bob), session_id)
            .await
            .is_ok(),
        "bob reaches it by the group again"
    );
}

/// G4 (issue 347): an entry naming a group that is gone - a team purged,
/// the participants of a session purged with it - is dropped as the list is
/// next stored, by any change of the workspace; one naming a deleted group
/// stays, since it may come back. Until then its owner sees it without a
/// name.
#[tokio::test]
async fn an_entry_naming_a_group_that_is_gone_is_dropped_as_the_list_is_stored() {
    let world = World::new().await;
    let alice = World::as_user(world.alice);
    let system = World::system();
    let groups = world.runtime.group_manager();
    let workspaces = world.runtime.workspace_manager();
    let session_id = world.shared_session(&[], &[]).await.id();
    let project =
        groups.create(&alice, "project", Scope::UserScope { user_id: world.alice }).await.unwrap();
    let docs = workspaces
        .create(
            &alice,
            "docs",
            Scope::UserScope { user_id: world.alice },
            "main",
            None,
            Classification::general(Confidentiality::Internal),
        )
        .await
        .unwrap();
    let controller = workspace(&world, docs.id).await;
    controller.share(&alice, Subject::Group(project.id), Grant::Read).await.unwrap();
    controller
        .share(&alice, Subject::Group(GroupId::of_session(session_id)), Grant::Write)
        .await
        .unwrap();
    let named = |workspace: &Workspace| {
        workspace.security_attributes.acl.iter().map(|entry| entry.subject).collect::<Vec<_>>()
    };
    let both = [Subject::Group(project.id), Subject::Group(GroupId::of_session(session_id))];

    groups.delete(&alice, project.id).await.unwrap();
    controller.update_name(&alice, "papers").await.unwrap();
    assert_eq!(named(&controller.load()), both, "a deleted team keeps its entry");
    let shown = world
        .manager
        .views(Viewer::new(alice.clone()))
        .workspace(docs.id)
        .await
        .unwrap()
        .snapshot()
        .await;
    assert_eq!(
        shown.access.iter().map(|e| e.name.as_deref()).collect::<Vec<_>>(),
        [None, Some("shared")],
        "shown without a name meanwhile"
    );

    groups.purge(&system, project.id).await.unwrap();
    controller.update_name(&alice, "docs").await.unwrap();
    let renamed = Arc::clone(&controller.load());
    assert_eq!(
        named(&renamed),
        both[1..],
        "a team purged loses it as the workspace is stored again"
    );

    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    world.manager.purge(&system, session_id).await.unwrap();
    controller.update_access(&alice, renamed.security_attributes.acl.clone()).await.unwrap();
    let kept = Arc::clone(&controller.load());
    assert!(
        named(&kept).is_empty(),
        "the participants of a session purged too, also as the list is written: {:?}",
        named(&kept)
    );
}

/// Issue 344: a workspace lies in a session's scope only where its maker
/// may read the session - what lies there keeps the session from being
/// deleted by whoever may not delete it -, asked once making it was, so
/// that who may not make one learns nothing of the session, and before
/// anything is made. As an agent is put there (issue 227).
#[tokio::test]
async fn a_workspace_is_made_in_a_session_s_scope_only_by_who_may_read_the_session() {
    let world = World::new().await;
    let workspaces = world.runtime.workspace_manager();
    let (bob, carol) = (World::as_user(world.bob), World::as_user(world.carol));
    let session = world.shared_session(&[], &[]).await;
    let scope = Scope::SessionScope { session_id: session.id() };

    let (refused, asked) = world
        .asked(workspaces.create(
            &carol,
            "files",
            scope.clone(),
            "main",
            None,
            Classification::PUBLIC,
        ))
        .await;
    assert!(
        matches!(refused, Err(WorkspaceError::Store(StoreError::NotFound { .. }))),
        "not by a stranger to it: {refused:?}"
    );
    assert!(
        asked.ends_with(&checks(&[
            "carol: CreateWorkspace(carol's) yes",
            "carol: GetSession(alice's) no"
        ])),
        "making it, then the session: {asked:?}"
    );
    assert!(
        world
            .runtime
            .entity_stores()
            .workspaces
            .list(&World::system(), &Query::all())
            .await
            .unwrap()
            .items
            .is_empty(),
        "nothing made"
    );

    let (made, asked) = world
        .asked(workspaces.create(
            &bob,
            "files",
            scope.clone(),
            "main",
            None,
            Classification::PUBLIC,
        ))
        .await;
    assert_eq!(made.expect("by a member, who reads it").name.scope, scope);
    assert!(
        asked.ends_with(&checks(&[
            "bob: CreateWorkspace(bob's) yes",
            "bob: GetSession(alice's) yes",
            "bob: CreateWorkspace(bob's) yes"
        ])),
        "{asked:?}"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Commands an agent carries out itself (issue 269) ------------------------

/// A command of the session (issue 28) is seen and run by whoever reads the
/// channel it is run in - bob, taking part -, asked of the channel's
/// attributes as a command of an agent is of the agent's (issue 304): first
/// whether he sees it, then whether he may run it, then, for `/help`, what
/// else he sees.
#[tokio::test]
async fn a_command_of_the_session_is_seen_and_run_by_whoever_reads_the_channel() {
    let world = World::new().await;
    let session = world.shared_session(&[], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let bob = main.user_actor(world.bob).unwrap();

    let (reply, asked) = world.asked(bob.execute_command("help", "")).await;
    let text = reply.expect("his to run").output.expect("answered to him");
    assert!(text.contains("/wakeup") && text.contains("/nudge"), "{text}");
    let about: Vec<&str> =
        asked.iter().filter(|line| line.contains("Command(")).map(String::as_str).collect();
    assert_eq!(
        about,
        [
            "bob: ShowCommand(alice's) yes",
            "bob: ExecuteCommand(alice's) yes",
            "bob: ShowCommand(alice's) yes",
            "bob: ShowCommand(alice's) yes",
            "bob: ShowCommand(alice's) yes",
        ],
        "by the channel's attributes: the command, then what /help lists"
    );
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}
