//! The agents' part of the security suite (issues 198, 370): what agents do
//! in someone's name - their tool calls, commands, readings and the answers
//! of agents that run elsewhere - tested in the world of the runtime's suite
//! with the agents' plugin (`caelumora_agents::testing`). As the runtime's
//! part: users with different permissions doing different things, through
//! the whole runtime - the stores checking with
//! the ownership policy, the `SessionManager`, running channels and the
//! views - as a frontend would. What the system starts from is put into the
//! store as the system, as a setup would; everything a user does goes
//! through the public API, in that user's name.
//!
//! The policy records every question it is asked (`Recorder`): a test of
//! an action asserts the questions asked in a user's name, in order - which
//! permissions the action needs. Each test names the scenario or action of
//! issue 198 it checks; the issue says what is expected and why.

use caelumora_model::store::query::Query;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use caelumora_model::actor::{ActorId, Rejection, ActorEvent, Envelope};
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityKind};
use caelumora_model::approval::{ApprovalRule, ArgumentMatcher, Reach, RuleDecision, ToolMatcher, ApprovalRequest, ApprovalRuleId};
use caelumora_model::channel::HistoryAccess;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor, StoredComponents};
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::question::{Addressee, Answers, QuestionId, QuestionRequest, Questions};
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::group::GroupId;
use caelumora_model::security::{AclEntry, Grant, SecurityContext, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::{ComponentStore, StateStore, StoreError};
use caelumora_model::template::{ScopeTemplate, SessionTemplateId, TemplateError, WorkspaceTemplate};
use caelumora_model::user::UserId;
use caelumora_model::work::TurnId;
use caelumora_runtime::channel::{ChannelError, ChannelLoop};
use caelumora_runtime::component::{ComponentFacets, ComponentKind, PRECONFIGURED_REF_KIND};
use caelumora_agents::external::{ExternalAgent, ExternalAgentResult, ExternalEvent, ExternalStream, OutboundMessage};
use caelumora_agents::testing::{asked_as_a_call_on_the_workspace, asked_for_a_call, calling, register_once, Once, TestAgents, ONCE_MODEL_KIND};
use caelumora_agents::testing as agent_testing;
use caelumora_runtime::testing::{asked_for, attributes, checks, denied, not_found, quiet, shown, workspace, World};
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop};
use caelumora_runtime::view::{ChannelView, ViewError, Viewer};
use caelumora_model::snapshot::PersonalitySnapshot;
use caelumora_runtime::workspace::WorkspaceError;
use caelumora_model::workspace::WorkspaceChoice;
use anyhow::Error;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::compaction::command::COMPACTION_KIND;
use caelumora_agents::components::channel_history::CHANNEL_HISTORY_KIND;
use caelumora_agents::llm::LLMClient;
use caelumora_agents::tool::{ToolOutput, ToolProvider, ToolResult};
use caelumora_agents::tool::echo_tool::ECHO_TOOL_KIND;
use caelumora_agents::tool::sleep_tool::SLEEP_TOOL_KIND;
use caelumora_agents::tool::terminal_tool::TERMINAL_TOOL_KIND;
use caelumora_agents::turn::Turn;
use caelumora_model::command::{Command, CommandOutput};
use caelumora_model::context::ModelProfile;
use caelumora_model::message::Message;
use caelumora_model::tool::{Tool, ToolParameters};
use caelumora_model::turn::GivenUpReason;
use caelumora_runtime::actor::user::ChannelWatch;
use caelumora_runtime::command::{CommandError, CommandResult, ProvidesCommands};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::Semaphore;

// --- Personalities and agents ---------------------------------------------

/// P2: a personality shared with the user, or the system's, makes an agent
/// of the user; its owner gets no say over that agent.
#[tokio::test]
async fn a_shared_personality_makes_an_agent_of_whoever_uses_it() {
    let world = agent_testing::world().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let global = world.personality("global", None, &[]).await;
    for personality in [&shared, &global] {
        let template = world.template(Some(world.alice), &[], personality.name.name.as_str(), personality.id).await;
        let session = world.manager.create_from_template(&World::as_user(world.alice), template, None, None).await.expect("hers to use");
        let agents = world.runtime.entity_stores().agents.list(&World::as_user(world.alice), &Query::all()).await.map(|page| page.items).unwrap();
        let agent = agents.iter().find(|a| a.personality_id == personality.id).expect("an agent of hers");
        assert_eq!(agent.security_attributes.owner_id, ActorId::User(world.alice), "owned by who made it");
        let gone = world.runtime.entity_stores().agents.get(&World::as_user(world.bob), agent.id).await.expect_err("not bob's to read");
        assert!(matches!(gone, StoreError::NotFound { .. }), "{gone}");
        let renamed = Agent { name: ScopedName::new("taken", Scope::GlobalScope).unwrap(), ..agent.clone() };
        assert!(world.runtime.entity_stores().agents.update(&World::as_user(world.bob), &renamed).await.is_err(), "nor his to change, though the personality is his");
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}

/// P3: an agent whose owner may no longer read its personality - a share
/// withdrawn - is not run: its session opens without it.
#[tokio::test]
async fn an_agent_whose_personality_was_withdrawn_is_left_out_of_its_channel() {
    let world = agent_testing::world().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &shared).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    assert!(world.members(world.alice, session.id(), channel).await.contains(&ActorId::Agent(helper)), "it runs while shared");
    assert!(session.channels()[0].member_ids().contains(&ActorId::Agent(helper)));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let withdrawn = Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world.runtime.entity_stores().personalities.update(&World::as_user(world.bob), &withdrawn).await.expect("bob owns it");

    let reopened = world.manager.open(&World::as_user(world.alice), session.id()).await.expect("the session still opens");
    assert!(!reopened.channels()[0].member_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    assert!(reopened.channels()[0].member_ids().contains(&ActorId::User(world.alice)), "everyone else is");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P3b: an agent whose personality is gone is not run either; its session
/// opens without it.
#[tokio::test]
async fn an_agent_whose_personality_is_gone_is_left_out_of_its_channel() {
    let world = agent_testing::world().await;
    let bobs = world.personality("bobs", Some(world.bob), &[world.alice]).await;
    let helper = world.agent("helper", world.alice, &bobs).await;
    let session = world.shared_session(&[helper], &[]).await;
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    world.runtime.entity_stores().personalities.delete(&World::as_user(world.bob), bobs.id).await.expect("bob deletes his personality");

    let reopened = world.manager.open(&World::as_user(world.alice), session.id()).await.expect("the session still opens");
    assert!(!reopened.channels()[0].member_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P6: what of a personality is public - how its agents introduce
/// themselves - reaches whoever sees the agent, though they may not read
/// the personality.
#[tokio::test]
async fn an_agents_introduction_reaches_whoever_sees_it_but_not_its_personality() {
    let world = agent_testing::world().await;
    let hers = world.personality("hers", Some(world.alice), &[]).await;
    let helper = world.agent("helper", world.alice, &hers).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();

    assert!(world.runtime.entity_stores().personalities.get(&World::as_user(world.bob), hers.id).await.is_err(), "bob may not read the personality");
    let view = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(channel).await.unwrap();
    let members = view.members().await;
    let member = members.iter().find(|m| m.id() == ActorId::Agent(helper)).expect("he sees the agent");
    assert_eq!(member.name(), Some("helper"));
    assert_eq!(member.introduction().await.unwrap().as_deref(), Some("I am hers."), "and how it introduces itself");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// P7: the preconfigured components a personality includes (issue 270) -
/// shared with the agent's owner by someone else - are read in the agent's
/// name, as its channel starts, as its personality is; once the share is
/// withdrawn, the agent is not run, but its session opens without it, as
/// with a withdrawn personality (P3).
#[tokio::test]
async fn the_components_a_personality_includes_are_read_in_its_agents_name() {
    let world = agent_testing::world().await;
    let tools = StoredComponents::new("bobs-tools", ComponentConfig { component_nodes: vec![ComponentDescriptor::new("calculator", "calculator_tool")] }, attributes(Some(world.bob), &[world.alice]));
    world.store.create_components(&World::as_user(world.bob), &tools).await.expect("bob keeps his tools");
    let mut hers = world.personality("hers", Some(world.alice), &[]).await;
    hers.components = ComponentConfig { component_nodes: vec![ComponentDescriptor { params: serde_json::json!({ "id": "bobs-tools" }), ..ComponentDescriptor::new("tools", PRECONFIGURED_REF_KIND) }] };
    world.runtime.entity_stores().personalities.update(&World::as_user(world.alice), &hers).await.unwrap();
    let helper = world.agent("helper", world.alice, &hers).await;

    let (session, asked) = world.asked(world.shared_session(&[helper], &[])).await;
    let personality = asked.iter().position(|line| line == "alice as helper: GetPersonality(alice's) yes").expect("its personality is read in its name");
    let components = asked.iter().position(|line| line == "alice as helper: GetComponents(bob's) yes").expect("and so are the components it includes");
    assert!(personality < components, "first the personality, then what it includes: {asked:?}");
    assert!(session.channels()[0].member_ids().contains(&ActorId::Agent(helper)), "it runs");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let withdrawn = StoredComponents { security_attributes: attributes(Some(world.bob), &[]), ..tools };
    world.store.update_components(&World::as_user(world.bob), &withdrawn).await.expect("bob owns them");
    let (reopened, asked) = world.asked(world.manager.open(&World::as_user(world.alice), session.id())).await;
    let reopened = reopened.expect("the session still opens: bob cannot stop alice's session so");
    assert!(asked.contains(&"alice as helper: GetComponents(bob's) no".to_string()), "{asked:?}");
    assert!(!reopened.channels()[0].member_ids().contains(&ActorId::Agent(helper)), "the agent is not run");
    assert!(reopened.channels()[0].member_ids().contains(&ActorId::User(world.alice)), "everyone else is");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Templates --------------------------------------------------------------

/// T1: a template is used by whoever may read it - its owner, who it is
/// shared with, everybody for the system's - and by nobody else.
#[tokio::test]
async fn a_template_is_used_only_by_who_may_read_it() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let bobs = world.template(Some(world.bob), &[], "private", global.id).await;
    let shared = world.template(Some(world.bob), &[world.alice], "shared", global.id).await;
    let systems = world.template(None, &[], "public", global.id).await;
    let alice = World::as_user(world.alice);

    let refused = world.manager.create_from_template(&alice, bobs, None, None).await.expect_err("not hers");
    assert!(not_found(&refused), "{refused:#}");
    for usable in [shared, systems] {
        let session = world.manager.create_from_template(&alice, usable, None, None).await.expect("hers to use");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(world.alice), "the session is hers, not the template's owner's");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
}

// --- Taking someone into a session (issue 221) ------------------------------

/// Joining by one's own act needs an entry of the access list that lets one
/// join - the session's, and the channel's for the channel -, asked in
/// one's own context; who may neither join nor read the session finds
/// nothing of it, and an invitation is used up as it is taken.
#[tokio::test]
async fn joining_needs_an_entry_that_lets_one_join() {
    let world = agent_testing::world().await;
    let shared = world.personality("shared", None, &[]).await;
    let template_id = world.template(Some(world.alice), &[], "helper", shared.id).await;
    let mut template = world.runtime.entity_stores().session_templates.get(&World::system(), template_id).await.unwrap();
    let bob_joins = AclEntry { subject: Subject::Actor(ActorId::User(world.bob)), grant: Grant::Join };
    template.access = vec![bob_joins];
    template.channels[0].access = vec![AclEntry { subject: Subject::Everyone, grant: Grant::Join }];
    world.runtime.entity_stores().session_templates.update(&World::system(), &template).await.unwrap();
    let alice = World::as_user(world.alice);
    let session = world.manager.create_from_template(&alice, template_id, None, None).await.unwrap();
    let main = session.channels()[0].id();
    let (bob, carol) = (World::as_user(world.bob), World::as_user(world.carol));

    let (refused, asked) = world.asked(world.manager.join(&carol, session.id(), ActorId::User(world.carol), Some(main))).await;
    assert!(not_found(&refused.expect_err("carol may neither join nor read it")), "nothing of it is told");
    assert_eq!(asked, checks(&["carol: JoinSession(alice's) no", "carol: GetSession(alice's) no"]));

    let (joined, asked) = world.asked(world.manager.join(&bob, session.id(), ActorId::User(world.bob), Some(main))).await;
    joined.expect("bob is let in");
    assert_eq!(asked, checks(&[
        "bob: JoinSession(alice's) yes",
        "bob: GetUser(system's) yes",
        "bob: JoinChannel(bob) yes",
        "bob: JoinSession(alice's) yes",
        "bob: JoinChannel(alice's) yes",
        "bob: JoinChannel(alice's) yes",
        "bob: JoinChannel(bob) yes",
    ]), "joining the session and the channel by the entries in his own name, and his clearance - stored in the runtime's");
    assert!(world.members(world.bob, session.id(), main).await.contains(&ActorId::User(world.bob)), "in the channel");
    assert!(!session.session().security_attributes.acl.contains(&bob_joins), "his invitation is used up");
    world.manager.stop(&alice, session.id()).await.unwrap();
}

// --- Messages ----------------------------------------------------------------


/// S4: cancelling one actor (issue 235) is cancelling, asked as
/// `CancelChannel` of the channel: whoever may stop the channel may stop
/// one of its actors - bob as a participant - and who may not is refused
/// before anything of the actor is looked at, so that he does not learn who
/// is in the channel. It is offered of an agent by the same question (issue
/// 243).
#[tokio::test]
async fn cancelling_one_actor_is_for_whoever_may_cancel_the_channel() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (bob, carol) = (World::as_user(world.bob), World::as_user(world.carol));
    world.manager.open(&bob, session.id()).await.expect("he opens it");

    let (cancelled, asked) = world.asked(main.cancel_actor(&bob, ActorId::User(world.alice))).await;
    cancelled.expect("his to cancel, as a participant");
    assert_eq!(asked, checks(&["bob: CancelChannel(alice's) yes"]), "asked as cancelling the channel");

    let nobody = ActorId::User(UserId::new());
    let (refused, asked) = world.asked(main.cancel_actor(&carol, nobody)).await;
    assert!(denied(&refused.expect_err("not hers to cancel")), "refused, not told that nobody of that id is there");
    assert_eq!(asked, checks(&["carol: CancelChannel(alice's) no"]));

    let view = world.manager.views(Viewer::new(bob)).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
    let member = view.member(ActorId::Agent(helper)).await.expect("the agent is a member");
    let (offered, asked) = world.asked(member.may_cancel()).await;
    assert!(offered, "offered to him, of the agent");
    assert_eq!(asked, checks(&["bob: CancelChannel(alice's) yes"]), "by the same question");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Interrupting one tool call of an actor (issue 276) is cancelling a part
/// of its turn, asked as cancelling one actor (S4): `CancelChannel` of the
/// channel. Whoever may - bob as a participant - learns that no such call
/// runs; who may not is refused before anything of the actor or its calls
/// is looked at.
#[tokio::test]
async fn interrupting_one_tool_call_is_for_whoever_may_cancel_the_channel() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (bob, carol) = (World::as_user(world.bob), World::as_user(world.carol));
    world.manager.open(&bob, session.id()).await.expect("he opens it");

    let (permitted, asked) = world.asked(main.interrupt_tool_call(&bob, ActorId::Agent(helper), TurnId::new(), "1")).await;
    assert!(matches!(permitted.expect_err("no call runs").downcast_ref::<ChannelError>(), Some(ChannelError::ToolCallNotRunning { .. })), "his to interrupt, but nothing runs");
    assert_eq!(asked, checks(&["bob: CancelChannel(alice's) yes"]), "asked as cancelling the channel");

    let (refused, asked) = world.asked(main.interrupt_tool_call(&carol, ActorId::Agent(helper), TurnId::new(), "1")).await;
    assert!(denied(&refused.expect_err("not hers to interrupt")), "refused, not told whether the call runs");
    assert_eq!(asked, checks(&["carol: CancelChannel(alice's) no"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M2: asking an agent to answer again (issue 234) is an act on the
/// conversation, like cancelling it: whoever takes part may - bob as well
/// as alice - asked as `RetryMessage` of the channel by the channel alone,
/// as it admits the request (issue 304). His actor looks at what it can
/// see at once - a message no agent said, an answer under way (issue 235)
/// or the agent's last turn given up (issue 259) that is neither - and asks
/// nothing. Whether the agent answers again is its own to decide.
/// It is offered of an agent by the same question (issue 242).
#[tokio::test]
async fn asking_an_agent_to_answer_again_is_for_whoever_takes_part() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::message::{Message, MessageRole};
    let answer = Message { role: MessageRole::Assistant, content: Some("an answer".to_string()), ..Message::new(Classification::PUBLIC) };
    let world = World::with(&[&TestAgents], |registry| register_once(registry, "answering", answer)).await;
    let answering = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "answering")] },
        ..world.personality("answering", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &answering).await.unwrap();
    let helper = world.agent("helper", world.alice, &answering).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let (alice, bob) = (main.user_actor(world.alice).unwrap(), main.user_actor(world.bob).unwrap());
    let mut watch = bob.watch();
    alice.send_message("@helper hello".to_string(), None).await.expect("sent");
    let ActorEvent::CompleteMessageGenerated(said) = shown(&mut watch, "CompleteMessageGenerated").await else { unreachable!() };
    let answered = loop {
        let ActorEvent::CompleteMessageGenerated(answered) = shown(&mut watch, "CompleteMessageGenerated").await else { unreachable!() };
        if answered.sender_id == ActorId::Agent(helper) {
            break answered;
        }
    };

    let (refused, asked) = world.asked(bob.request_retry(said.id)).await;
    assert!(matches!(refused.expect_err("no agent said it").downcast_ref::<ChannelError>(), Some(ChannelError::NotAnAgentsMessage { .. })));
    assert!(asked.is_empty(), "his actor asks nothing: {asked:?}");
    let (refused, asked) = world.asked(bob.request_retry_of_turn(ActorId::User(world.alice), TurnId::new())).await;
    assert!(matches!(refused.expect_err("no agent answers").downcast_ref::<ChannelError>(), Some(ChannelError::NotAParticipant { .. })));
    assert!(asked.is_empty(), "nor of an answer under way: {asked:?}");
    let (refused, asked) = world.asked(bob.request_retry_of_turn(ActorId::Agent(helper), TurnId::new())).await;
    assert!(matches!(refused.expect_err("neither under way nor given up").downcast_ref::<ChannelError>(), Some(ChannelError::TurnNotRetryable { .. })));
    assert!(asked.is_empty(), "nor of a turn given up (issue 259): {asked:?}");
    let (_, asked) = world.asked(async {
        bob.request_retry(answered.id).await.expect("sent: the channel decides");
        asked_for(&world, "bob: RetryMessage(alice's) yes").await;
    }).await;
    assert_eq!(asked.first().map(String::as_str), Some(checks(&["bob: RetryMessage(alice's) yes"])[0].as_str()), "his to ask, as a participant - asked by the channel as it admits it");
    let view = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
    let member = view.member(ActorId::Agent(helper)).await.expect("the agent is a member");
    let (offered, asked) = world.asked(member.may_retry()).await;
    assert!(offered, "and offered to him, of the agent (issue 242)");
    assert_eq!(asked, checks(&["bob: RetryMessage(alice's) yes"]), "by the same question");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// M4: nudging an agent (issue 278) asks it to respond without a message,
/// as an `@`-mention in a message would: whoever takes part may - bob as
/// well as alice -, and nothing is asked in his name; someone who does not
/// take part has no actor to nudge it through. The agent responds for its
/// owner, on the word of whoever nudged it.
#[tokio::test]
async fn nudging_an_agent_is_for_whoever_takes_part_and_it_responds_on_their_word() {
    let world = World::with(&[&TestAgents], |registry| register_once(registry, "silent", Message::new(Classification::PUBLIC))).await;
    let silent = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "silent")] },
        ..world.personality("silent", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &silent).await.unwrap();
    let helper = world.agent("helper", world.alice, &silent).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let bob = main.user_actor(world.bob).unwrap();
    let mut his = bob.watch();

    let (nudged, asked) = world.asked(bob.request_response(ActorId::Agent(helper))).await;
    nudged.expect("his to nudge, as a participant");
    assert!(asked.is_empty(), "nothing beyond taking part: {asked:?}");
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = his.recv().await.expect("the channel runs");
            if envelope.event.kind() == "TurnFinished" {
                break envelope;
            }
        }
    }).await.expect("the agent responds");
    let turn = finished.security;
    assert_eq!((turn.caller_id, turn.owner_id, turn.principal_id, turn.initiator_id), (ActorId::Agent(helper), ActorId::User(world.alice), ActorId::User(world.alice), ActorId::User(world.bob)), "alice's, for alice, on his word");
    assert!(main.user_actor(world.carol).is_none(), "carol takes no part, and has no actor to nudge through");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- What an agent saw --------------------------------------------------------

/// A1: what an agent saw is its owner's to read alone - not even who reads
/// the channel with it.
#[tokio::test]
async fn what_an_agent_saw_is_its_owners_alone() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    assert!(hers.may_read_context().await, "offered to its owner");
    assert!(hers.context().await.is_ok(), "who reads it");
    let his = member(world.bob).await;
    assert!(!his.may_read_context().await, "not offered to bob");
    assert!(matches!(his.context().await, Err(ViewError::Forbidden { .. })), "nor read by him");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent's private notes across the channels of a session (issue 233)
/// are read as what it saw: by its owner, not by another member.
#[tokio::test]
async fn an_agents_private_notes_are_its_owners_alone() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let notes = |user| {
        let world = &world;
        let session = session.id();
        async move { world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().private_notes_of(helper).await }
    };

    assert!(notes(world.alice).await.is_ok(), "read by its owner");
    assert!(matches!(notes(world.bob).await, Err(ViewError::Forbidden { .. })), "not by bob, who takes part");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What an agent can do now - its tools (issue 194) - is read by every
/// member of the channel it works in, not only by its owner; not by
/// whoever is no member.
#[tokio::test]
async fn which_tools_an_agent_has_is_read_by_the_members_of_its_channel() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    let (tools, asked) = world.asked(hers.tool_sets()).await;
    assert!(tools.is_ok(), "read from its running actor");
    assert_eq!(asked, checks(&["alice: ReadAgentTools(alice's) yes"]));
    let his = member(world.bob).await;
    assert!(his.may_read_tools().await, "offered to bob, who takes part in the channel");
    let (tools, asked) = world.asked(his.tool_sets()).await;
    assert!(tools.is_ok(), "and read by him");
    assert_eq!(asked, checks(&["bob: ReadAgentTools(alice's) yes"]));
    let theirs = world.manager.views(Viewer::new(World::as_user(world.carol))).session(session.id()).await;
    assert!(theirs.is_err(), "carol, who is no member, does not even reach the channel");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What an agent was built from - its components, the preconfigured ones
/// it includes resolved (issue 270) - is read by its owner alone: it is
/// configuration, and what it includes may have been shared with the owner
/// alone. Not by bob, who takes part in the channel and sees its tools.
#[tokio::test]
async fn which_components_an_agent_was_built_from_is_read_by_its_owner_alone() {
    let world = agent_testing::world().await;
    let tools = StoredComponents::new("bobs-tools", ComponentConfig { component_nodes: vec![ComponentDescriptor::new("calculator", "calculator_tool")] }, attributes(Some(world.bob), &[world.alice]));
    world.store.create_components(&World::as_user(world.bob), &tools).await.unwrap();
    let mut hers = world.personality("hers", Some(world.alice), &[]).await;
    hers.components = ComponentConfig { component_nodes: vec![
        ComponentDescriptor::new("stop", "stop_tool"),
        ComponentDescriptor { params: serde_json::json!({ "id": "bobs-tools" }), ..ComponentDescriptor::new("tools", PRECONFIGURED_REF_KIND) },
    ] };
    world.runtime.entity_stores().personalities.update(&World::as_user(world.alice), &hers).await.unwrap();
    let helper = world.agent("helper", world.alice, &hers).await;
    let session = world.shared_session(&[helper], &[]).await;
    let channel = session.channels()[0].id();
    let member = |user| {
        let world = &world;
        let session = session.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };

    let hers = member(world.alice).await;
    assert!(hers.may_read_components().await, "offered to alice, its owner");
    let (components, asked) = world.asked(hers.components()).await;
    let ids: Vec<String> = components.expect("read from its running actor").component_nodes.into_iter().map(|node| node.id).collect();
    assert_eq!(ids, ["stop", "calculator"], "resolved, as it was built");
    assert_eq!(asked, checks(&["alice: ReadAgentComponents(alice's) yes"]));
    let his = member(world.bob).await;
    assert!(!his.may_read_components().await, "not offered to bob");
    let (components, asked) = world.asked(his.components()).await;
    assert!(matches!(components, Err(ViewError::Forbidden { .. })), "nor read by him, though he sees its tools");
    assert_eq!(asked, checks(&["bob: ReadAgentComponents(alice's) no"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Approvals ---------------------------------------------------------------


/// alice's agent `helper`, which calls `sleep` (a call that needs approval)
/// once it is asked, in a session alice shares with bob and he opened;
/// with the session's main channel.
async fn with_a_call_to_approve() -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, AgentId) {
    let sleep = ComponentDescriptor {
        params: serde_json::json!({ "requires_approval": true }),
        ..ComponentDescriptor::new("sleep", SLEEP_TOOL_KIND)
    };
    with_a_call_of(calling("sleep", serde_json::json!({ "seconds": 0 })), sleep).await
}

/// `with_a_call_to_approve`, with an agent that calls `run` of `Commands`
/// with the command line `command`.
async fn with_a_command_to_approve(command: &str) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, AgentId) {
    with_a_call_of(calling("run", serde_json::json!({ "command": command })), ComponentDescriptor::new("commands", "commands")).await
}

/// `with_a_call_to_approve`, with an agent that answers `call` with its tool
/// `tool`.
async fn with_a_call_of(call: Message, tool: ComponentDescriptor) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, AgentId) {
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with(&[&TestAgents], |registry| {
        registry.register(ComponentKind::new("once").provides::<dyn LLMClient>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn LLMClient>(Arc::<Once>::clone(&model));
            Ok(facets)
        });
        registry.register(ComponentKind::new("commands").provides::<dyn ToolProvider>(), |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(Commands));
            Ok(facets)
        });
    }).await;
    let asking = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "once"), tool] },
        ..world.personality("asking", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &asking).await.unwrap();
    let helper = world.agent("helper", world.alice, &asking).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (world, session, main, helper)
}

/// A tool that asks `ask` - the agent's owner, or whoever caused the call -
/// their name before it runs (issue 165), the question `sensitive` or not.
#[derive(Debug)]
struct Asking {
    ask: Addressee,
    sensitive: bool,
}

#[async_trait]
impl ToolProvider for Asking {
    fn description(&self) -> String {
        "asks before it runs".to_string()
    }
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "asking".to_string(),
            description: "asks before it runs".to_string(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    async fn execute(&self, _: &str, _: &Value, answers: Option<&Answers>, _: &mut Turn) -> ToolResult<ToolOutput> {
        Ok(serde_json::json!({ "answered": answers.is_some() }).into())
    }
    fn questions(&self, _: &str, _: &Value) -> ToolResult<Option<Questions>> {
        use caelumora_model::question::{Question, Questions};
        Ok(Some(Questions {
            questions: vec![Question { header: "Name".to_string(), question: "Who are you?".to_string(), choices: Vec::new(), multi_select: false }],
            ask: vec![self.ask.clone()],
            sensitive: self.sensitive,
        }))
    }
}

/// alice's agent `helper`, which calls `asking` - a tool that asks alice,
/// its owner, before it runs - once `asker` asks it, in a session alice
/// shares with bob and he opened; with the session's main channel, the
/// questions as the asker is shown them, and what bob was shown since
/// before it was asked.
async fn with_a_call_asking(sensitive: bool, asker: fn(&World) -> UserId) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, QuestionRequest, ChannelWatch) {
    with_a_call_asking_whom(Addressee::Owner, sensitive, asker).await
}

/// `with_a_call_asking`, with a tool that asks `ask`.
async fn with_a_call_asking_whom(ask: Addressee, sensitive: bool, asker: fn(&World) -> UserId) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, QuestionRequest, ChannelWatch) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "asking".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with(&[&TestAgents], |registry| {
        registry.register(ComponentKind::new("once").provides::<dyn LLMClient>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn LLMClient>(Arc::<Once>::clone(&model));
            Ok(facets)
        });
        registry.register(ComponentKind::new("asking").provides::<dyn ToolProvider>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(Asking { ask: ask.clone(), sensitive }));
            Ok(facets)
        });
    }).await;
    let asking = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "once"),
            ComponentDescriptor::new("asking", "asking"),
        ] },
        ..world.personality("asking", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &asking).await.unwrap();
    let helper = world.agent("helper", world.alice, &asking).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut watch = main.user_actor(asker(&world)).expect("the asker takes part").watch();
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    main.user_actor(asker(&world)).unwrap().send_message("@helper ask".to_string(), None).await.expect("sent");
    let ActorEvent::QuestionRequested(request) = shown(&mut watch, "QuestionRequested").await else { unreachable!() };
    (world, session, main, request, his)
}

/// The questions of `helper`'s calls in `main`, as `user` is shown them in
/// a snapshot.
async fn questions_shown(world: &World, session: SessionId, main: ChannelId, user: UserId) -> Vec<QuestionId> {
    use caelumora_model::snapshot::Detail;
    let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(main).await.unwrap();
    view.snapshot(Detail::MEMBERS | Detail::APPROVALS).await.members.into_iter().flat_map(|m| m.pending_questions).map(|q| q.id).collect()
}

/// Answering the questions of an agent's call (issue 165) is asked as
/// `AnswerQuestion` of the channel, of whoever answers, by the channel as it
/// admits the answer (issue 304) - bob may, as a participant - and counts
/// only of those the questions were put to: bob, shown alice's questions,
/// is refused by his actor, which asks nothing; alice answers.
#[tokio::test]
async fn questions_are_answered_by_whom_they_are_put_to() {
    use caelumora_model::question::{QuestionError, Response};
    let (world, session, main, request, _) = with_a_call_asking(false, |world| world.alice).await;
    assert_eq!(request.questions.asked(), [ActorId::User(world.alice)], "the agent's owner");
    assert_eq!(questions_shown(&world, session.id(), main.id(), world.bob).await, [request.id], "everyone taking part sees what the agent waits for");
    let name = || Some(vec![Response::Text { text: "Alice".to_string() }]);

    let (refused, asked) = world.asked(main.user_actor(world.bob).unwrap().answer_questions(request.id, name())).await;
    let refused = refused.expect_err("not put to him");
    assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: QuestionError::NotAsked, .. })), "{refused}");
    assert!(asked.is_empty(), "his actor asks nothing: {asked:?}");

    let (_, asked) = world.asked(async {
        main.user_actor(world.alice).unwrap().answer_questions(request.id, name()).await.expect("put to her");
        asked_for(&world, "alice: AnswerQuestion(alice's) yes").await;
    }).await;
    assert_eq!(asked.first().map(String::as_str), Some(checks(&["alice: AnswerQuestion(alice's) yes"])[0].as_str()), "asked by the channel as it admits it");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Sensitive questions (issue 165) are shown to those they are put to
/// alone: bob, taking part, neither is sent them nor sees them in a
/// snapshot, and cannot answer them.
#[tokio::test]
async fn sensitive_questions_reach_only_whom_they_are_put_to() {
    use caelumora_model::question::{QuestionError, Response};
    let (world, session, main, request, mut his) = with_a_call_asking(true, |world| world.alice).await;
    assert_eq!(questions_shown(&world, session.id(), main.id(), world.alice).await, [request.id]);
    assert!(questions_shown(&world, session.id(), main.id(), world.bob).await.is_empty(), "not in his snapshot");

    let refused = main.user_actor(world.bob).unwrap().answer_questions(request.id, Some(vec![Response::Text { text: "Bob".to_string() }])).await.expect_err("never shown to him");
    assert!(matches!(refused.downcast_ref::<ChannelError>(), Some(ChannelError::Question { reason: QuestionError::NotOpen, .. })), "{refused}");
    main.user_actor(world.alice).unwrap().answer_questions(request.id, Some(vec![Response::Text { text: "Alice".to_string() }])).await.expect("put to her");
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(envelope) = his.try_recv() {
        assert!(!matches!(envelope.event, ActorEvent::QuestionRequested(_) | ActorEvent::QuestionClosed { .. }), "nothing of them reaches him: {}", envelope.event.kind());
    }
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The owner a call's questions are put to (`Addressee::Owner`, issue 165)
/// is the agent's owner whoever asked the agent (issue 255): bob asks
/// alice's agent, and alice is asked - a tool that asks its owner for a
/// password never asks whoever happens to talk to the agent.
#[tokio::test]
async fn questions_for_the_owner_are_put_to_the_agents_owner_whoever_asked() {
    let (world, session, _, request, _) = with_a_call_asking(false, |world| world.bob).await;
    assert_eq!(request.questions.asked(), [ActorId::User(world.alice)], "the agent's owner, not bob who asked");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Questions for the initiator (`Addressee::Initiator`, issue 341) are put
/// to whoever caused the call, not the agent's owner: bob asks alice's
/// agent, a tool asks him for something sensitive - as a bank's agent asks
/// its customer for their PIN - and alice is neither asked nor shown it.
#[tokio::test]
async fn questions_for_the_initiator_are_put_to_whoever_asked_the_agent() {
    let (world, session, main, request, _) = with_a_call_asking_whom(Addressee::Initiator, true, |world| world.bob).await;
    assert_eq!(request.questions.asked(), [ActorId::User(world.bob)], "bob who asked, not alice who owns the agent");
    assert!(questions_shown(&world, session.id(), main.id(), world.alice).await.is_empty(), "not shown to the owner");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What came of asking `helper` in `main`, as `user` follows it: the
/// request the approver put up - `None` if none within a while, as for a
/// call refused before anyone is asked - the rule `user`'s actor answered it
/// by, if one did, and the approver's decision, if it decided within a
/// while.
struct Asked {
    request: Option<ApprovalRequest>,
    by_rule: Option<ApprovalRuleId>,
    approved: Option<bool>,
}

async fn ask_helper(world: &World, main: &ChannelLoop, user: UserId) -> Asked {
    use caelumora_model::actor::ActorEvent;
    let mut watch = main.user_actor(user).expect("they take part").watch();
    main.user_actor(world.alice).unwrap().send_message("@helper wait".to_string(), None).await.expect("sent");
    let mut asked = Asked { request: None, by_rule: None, approved: None };
    let _ = tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            let envelope = watch.recv().await.expect("the channel runs");
            let from_approver = matches!(envelope.context.sender_id, ActorId::Approver(_));
            match envelope.event {
                ActorEvent::ApprovalRequested(request) if from_approver => asked.request = Some(request),
                ActorEvent::ApprovalGranted(answer) | ActorEvent::ApprovalDenied(answer) if envelope.context.sender_id == ActorId::User(user) => asked.by_rule = answer.by_rule,
                ActorEvent::ApprovalGranted(_) if from_approver => { asked.approved = Some(true); break }
                ActorEvent::ApprovalDenied(_) if from_approver => { asked.approved = Some(false); break }
                _ => {}
            }
        }
    }).await;
    asked
}

/// A tool `run` that needs approval and runs nothing, whose `command` is a
/// shell command line (issue 247): what makes two of its calls alike is
/// what its parameters say.
#[derive(Debug)]
struct Commands;

#[async_trait]
impl ToolProvider for Commands {
    fn description(&self) -> String {
        "runs commands".to_string()
    }
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "run".to_string(),
            description: "runs a command".to_string(),
            parameters: ToolParameters::object([("command", ToolParameters::shell("The command line"))], ["command"]),
            requires_approval: true,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    async fn execute(&self, _: &str, _: &Value, _: Option<&Answers>, _: &mut Turn) -> ToolResult<ToolOutput> {
        Ok(serde_json::json!({}).into())
    }
}

fn sleeping(decision: RuleDecision, reach: Reach) -> ApprovalRule {
    ApprovalRule::new(ToolMatcher { tool: "sleep".to_string(), arguments: Default::default() }, decision, reach, None).unwrap()
}

/// A person's rule (issue 13) is their answer given in advance: it answers
/// a request for them, through their actor, as their click would, and the
/// approver decides on it as on any answer of theirs. Only they set it.
#[tokio::test]
async fn a_persons_rule_answers_for_them_as_they_would() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    let rule = sleeping(RuleDecision::Allow, Reach::Session(session.id()));
    assert!(denied(&Error::new(hers.add_approval_rule(&World::as_user(world.bob), rule.clone()).await.expect_err("not his to set"))));
    hers.add_approval_rule(&World::as_user(world.alice), rule.clone()).await.expect("hers");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_some_and(|r| !r.explicit), "the call is put up for approval");
    assert_eq!(asked.by_rule, Some(rule.id), "her actor answers by her rule, and shows her so");
    assert_eq!(asked.approved, Some(true), "and the approver decides on it");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A rule answers only where its person's own answer would count: bob's
/// rule does not answer for alice's agent, whose calls he may not approve.
#[tokio::test]
async fn a_rule_answers_only_where_its_person_may_approve() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let his = world.runtime.controller_manager().user_controller(world.bob).await.unwrap();
    his.add_approval_rule(&World::as_user(world.bob), sleeping(RuleDecision::Allow, Reach::Everywhere)).await.expect("his");

    let asked = ask_helper(&world, &main, world.bob).await;
    assert!(asked.request.is_some(), "the call is put up for approval");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "his rule does not answer, and nothing is decided");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A rule answers only within its reach: one for another session or
/// another channel does not.
#[tokio::test]
async fn a_rule_out_of_its_reach_does_not_answer() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    let alice = World::as_user(world.alice);
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Session(SessionId::new()))).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Channel(ChannelId::new()))).await.unwrap();

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_some());
    assert_eq!((asked.by_rule, asked.approved), (None, None), "neither answers");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// What a place restricts outweighs what a person allows: a channel's
/// `ask` leaves the call to her own decision, and a session's `deny`
/// refuses it before anyone is asked.
#[tokio::test]
async fn a_rule_of_a_place_outweighs_a_persons_rule() {
    let (world, session, main, _) = with_a_call_to_approve().await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Everywhere)).await.unwrap();
    main.update_approval_rules(&alice, vec![sleeping(RuleDecision::Ask, Reach::Everywhere)]).await.expect("her channel");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.as_ref().is_some_and(|r| r.explicit), "asked for explicitly");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "her rule does not answer");
    world.manager.stop(&alice, session.id()).await.unwrap();

    let (world, session, main, _) = with_a_call_to_approve().await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, sleeping(RuleDecision::Allow, Reach::Everywhere)).await.unwrap();
    session.update_approval_rules(&alice, vec![sleeping(RuleDecision::Deny, Reach::Everywhere)]).await.expect("her session");

    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.is_none(), "nobody is asked");
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// A rule derived from a command (issue 247) is about the commands that
/// begin as it began, as its tool proposes: alice approves `ls` and from
/// now on, and her rule answers `ls -la` - but not `ls; rm -rf ~`, for which
/// she is asked again.
#[tokio::test]
async fn a_rule_derived_from_a_command_answers_only_the_commands_like_it() {
    let (world, session, main, _) = with_a_command_to_approve("ls").await;
    let asked = ask_helper(&world, &main, world.alice).await;
    let suggested = asked.request.and_then(|request| request.suggested).expect("a rule is proposed");
    assert_eq!(suggested.arguments["command"], ArgumentMatcher::Shell { shell: vec!["ls".to_string()] });
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    // As the approval card makes it of her answer.
    let rule = ApprovalRule::new(suggested, RuleDecision::Allow, Reach::Everywhere, None).expect("a rule she may keep");

    for (command, answered) in [("ls -la", true), ("ls; rm -rf ~", false)] {
        let (world, session, main, _) = with_a_command_to_approve(command).await;
        let alice = World::as_user(world.alice);
        let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
        hers.add_approval_rule(&alice, rule.clone()).await.unwrap();
        let asked = ask_helper(&world, &main, world.alice).await;
        assert!(asked.request.is_some(), "{command}: put up for approval");
        assert_eq!(asked.by_rule, answered.then_some(rule.id), "{command}");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
}

/// A rule that denies a command (issue 247) denies a line it is part of,
/// and asks for one it cannot judge: a place's leaves it to alice's own
/// decision, which her rule that allows every command does not take; her
/// own rule that denies does not answer for her.
#[tokio::test]
async fn a_rule_that_denies_a_command_asks_for_a_line_it_cannot_judge() {
    let commands = |words: &[&str], decision| {
        let shell = ArgumentMatcher::Shell { shell: words.iter().map(|word| word.to_string()).collect() };
        ApprovalRule::new(ToolMatcher { tool: "run".to_string(), arguments: [("command".to_string(), shell)].into() }, decision, Reach::Everywhere, None).unwrap()
    };
    let every_command = ApprovalRule::new(ToolMatcher { tool: "run".to_string(), arguments: Default::default() }, RuleDecision::Allow, Reach::Everywhere, None).unwrap();

    let (world, session, main, _) = with_a_command_to_approve("ls && rm x").await;
    let alice = World::as_user(world.alice);
    session.update_approval_rules(&alice, vec![commands(&["rm"], RuleDecision::Deny)]).await.expect("her session");
    assert!(ask_helper(&world, &main, world.alice).await.request.is_none(), "denied before anyone is asked");
    world.manager.stop(&alice, session.id()).await.unwrap();

    let (world, session, main, _) = with_a_command_to_approve("echo $HOME").await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, every_command.clone()).await.unwrap();
    session.update_approval_rules(&alice, vec![commands(&["rm"], RuleDecision::Deny)]).await.expect("her session");
    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.as_ref().is_some_and(|r| r.explicit), "asked for explicitly");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "her rule does not answer");
    world.manager.stop(&alice, session.id()).await.unwrap();

    let (world, session, main, _) = with_a_command_to_approve("echo $HOME").await;
    let alice = World::as_user(world.alice);
    let hers = world.runtime.controller_manager().user_controller(world.alice).await.unwrap();
    hers.add_approval_rule(&alice, commands(&["rm"], RuleDecision::Deny)).await.unwrap();
    let asked = ask_helper(&world, &main, world.alice).await;
    assert!(asked.request.as_ref().is_some_and(|r| !r.explicit), "put up for approval");
    assert_eq!((asked.by_rule, asked.approved), (None, None), "her rule that denies does not answer");
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Approving a tool call (issue 14) is for who is responsible for the
/// agent - its owner and contributors: alice's answer counts, bob's, who
/// only takes part, is refused by the channel as it admits it, and he is
/// told (`MessageRejected`, issue 312). The approver asks of everybody
/// taking part to know whom to wait for; the view offers it to alice
/// alone.
#[tokio::test]
async fn a_tool_call_is_approved_by_who_is_responsible_for_the_agent() {
    use caelumora_model::actor::ActorEvent;
    use caelumora_model::approval::ApprovalResponse;

    let (world, session, main, helper) = with_a_call_to_approve().await;
    let mut watch = main.user_actor(world.alice).expect("she takes part").watch();

    main.user_actor(world.alice).unwrap().send_message("@helper wait".to_string(), None).await.expect("sent");
    let request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let ActorEvent::ApprovalRequested(request) = watch.recv().await.expect("the channel runs").event {
                break request;
            }
        }
    }).await.expect("the call is put up for approval");

    let view = |user| {
        let world = &world;
        let session = session.id();
        let channel = main.id();
        async move {
            let view = world.manager.views(Viewer::new(World::as_user(user))).session(session).await.unwrap().channel(channel).await.unwrap();
            view.members().await.into_iter().find(|m| m.id() == ActorId::Agent(helper)).expect("the agent is a member")
        }
    };
    let (offered, asked) = world.asked(async { (view(world.alice).await.may_approve().await, view(world.bob).await.may_approve().await) }).await;
    assert_eq!(offered, (true, false), "offered to alice alone");
    let approving: Vec<&String> = asked.iter().filter(|line| line.contains("ApproveToolCall")).collect();
    assert_eq!(approving, ["alice: ApproveToolCall(alice's) yes", "bob: ApproveToolCall(alice's) no"]);

    let response = ApprovalResponse::new(request.id, helper);
    let mut his = main.user_actor(world.bob).unwrap().watch();
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_approval(response.clone(), false).await.expect("sent");
        let ActorEvent::MessageRejected { rejected, reason, .. } = shown(&mut his, "MessageRejected").await else { unreachable!() };
        assert_eq!((rejected.as_str(), reason), ("ApprovalDenied", Rejection::Forbidden), "his answer is refused, and he is told");
        main.user_actor(world.alice).unwrap().send_approval(response.clone(), true).await.expect("sent");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // The approver's decision, not the answers shown as given.
                let envelope = watch.recv().await.expect("the channel runs");
                if !matches!(envelope.context.sender_id, ActorId::Approver(_)) {
                    continue;
                }
                match envelope.event {
                    ActorEvent::ApprovalGranted(decided) if decided.id == request.id => break,
                    ActorEvent::ApprovalDenied(decided) if decided.id == request.id => panic!("bob's answer counted"),
                    _ => {}
                }
            }
        }).await.expect("decided");
    }).await;
    let approving: Vec<&String> = asked.iter().filter(|line| line.contains("ApproveToolCall")).collect();
    assert_eq!(approving, [
        // bob answers; the channel refuses it.
        "bob: ApproveToolCall(alice's) no",
        // alice answers, the channel admits it; whom to wait for: her alone.
        "alice: ApproveToolCall(alice's) yes",
        "alice: ApproveToolCall(alice's) yes",
        "bob: ApproveToolCall(alice's) no",
    ]);
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A tool whose arguments go to a remote side cleared for public alone -
/// as an MCP server that names nothing (issue 180).
#[derive(Debug)]
struct Outward;

#[async_trait]
impl ToolProvider for Outward {
    fn description(&self) -> String {
        "sends out".to_string()
    }
    fn tools(&self) -> Vec<Tool> {
        vec![Tool {
            name: "send_out".to_string(),
            description: "sends out".to_string(),
            parameters: ToolParameters::object(Vec::<(String, ToolParameters)>::new(), Vec::<String>::new()),
            requires_approval: false,
            visibility: Default::default(),
            classification: None,
            presentation: Default::default(),
        }]
    }
    fn destination(&self) -> Option<Classification> {
        Some(Classification::PUBLIC)
    }
    async fn execute(&self, _: &str, _: &Value, _: Option<&Answers>, _: &mut Turn) -> ToolResult<ToolOutput> {
        panic!("a withheld tool is never run")
    }
}

/// The door (issue 180): a tool set whose remote side the channel may not
/// flow to is not offered there - asked as publishing what the channel
/// holds there, with its owner's rights in a turn, asked by whoever asked
/// it, and in its owner's name for who looks at its tools (issue 255) - and
/// its tools are not called by name.
#[tokio::test]
async fn a_tool_whose_remote_side_is_not_cleared_for_the_channel_is_not_offered_there() {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolResultStatus, ToolVisibility};

    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "send_out".to_string(), arguments: serde_json::json!({}), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let model = Arc::new(Once(Mutex::new(Some(call))));
    let world = World::with(&[&TestAgents], |registry| {
        registry.register(ComponentKind::new("once").provides::<dyn LLMClient>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn LLMClient>(Arc::<Once>::clone(&model));
            Ok(facets)
        });
        registry.register(ComponentKind::new("outward").provides::<dyn ToolProvider>(), |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn ToolProvider>(Arc::new(Outward));
            Ok(facets)
        });
    }).await;
    let sending = Personality {
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "once"), ComponentDescriptor::new("outward", "outward")] },
        ..world.personality("sending", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &sending).await.unwrap();
    let helper = world.agent("helper", world.alice, &sending).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut watch = main.user_actor(world.bob).expect("he takes part").watch();

    let (answered, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper send it".to_string(), None).await.expect("sent");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ActorEvent::CompleteMessageGenerated(message) = watch.recv().await.expect("the channel runs").event {
                    if let Some(result) = message.tool_results.first() {
                        break result.status;
                    }
                }
            }
        }).await.expect("the call is answered")
    }).await;
    assert_eq!(answered, ToolResultStatus::UnknownTool, "not called by its name");
    assert!(asked.contains(&"alice as helper, asked by bob: Publish no".to_string()), "judged as alice, its owner, asked by bob: {asked:?}");
    assert!(!asked.iter().any(|line| line.contains("CallTool")), "refused before the policy is asked of the call: {asked:?}");

    let (sets, asked) = world.asked(async {
        let view = world.manager.views(Viewer::new(World::as_user(world.alice))).session(session.id()).await.unwrap().channel(main.id()).await.unwrap();
        view.member(ActorId::Agent(helper)).await.unwrap().tool_sets().await.unwrap()
    }).await;
    assert!(sets[0].tools.is_empty() && sets[0].unavailable.is_some(), "shown as unavailable: {sets:?}");
    assert!(asked.contains(&"alice as helper: Publish no".to_string()), "as a turn would be offered them, in the agent's own name: {asked:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- In whose name an agent acts (issue 255) ---------------------------------


/// An agent asked by another agent acts for its own owner, and is recorded
/// as asked by whoever began the conversation: carol asks bob's `relay`,
/// which asks alice's `doer`, whose call is judged as alice and recorded as
/// asked by carol. The agent between them lends nothing, nor does its owner:
/// one agent mentioning another does not delegate to it.
#[tokio::test]
async fn an_agent_asked_by_an_agent_acts_for_its_own_owner() {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let internal = Classification::general(Confidentiality::Internal);
    let relayed = Message { role: MessageRole::Assistant, content: Some("@doer echo it".to_string()), ..Message::new(internal.clone()) };
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "echo".to_string(), arguments: serde_json::json!({ "message": "it" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(internal)
    };
    let world = World::with(&[&TestAgents], |registry| {
        register_once(registry, "relaying", relayed);
        register_once(registry, "calling", call);
    }).await;
    let with = |name: &'static str, components: Vec<ComponentDescriptor>| {
        let world = &world;
        async move {
            let personality = Personality { components: ComponentConfig { component_nodes: components }, ..world.personality(name, None, &[]).await };
            world.runtime.entity_stores().personalities.update(&World::system(), &personality).await.unwrap();
            personality
        }
    };
    let relaying = with("relaying", vec![ComponentDescriptor::new("model", "relaying")]).await;
    let calling = with("calling", vec![
        ComponentDescriptor::new("model", "calling"),
        ComponentDescriptor::new("echo", ECHO_TOOL_KIND),
    ]).await;
    let relay = world.agent("relay", world.bob, &relaying).await;
    let doer = world.agent("doer", world.alice, &calling).await;
    let session = world.session_of(&[world.bob, world.carol], &[relay, doer], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.carol), session.id()).await.expect("she opens it");

    let (_, asked) = world.asked(async {
        main.user_actor(world.carol).expect("she takes part").send_message("@relay ask doer".to_string(), None).await.expect("sent");
        asked_for(&world, "alice as doer, asked by carol: CallTool yes").await;
    }).await;
    let calling: Vec<&String> = asked.iter().filter(|line| line.contains("CallTool")).collect();
    assert!(!calling.is_empty() && calling.iter().all(|line| *line == "alice as doer, asked by carol: CallTool yes"), "judged as its owner, asked by who began it: {asked:?}");
    assert!(asked.iter().filter(|line| line.contains("as doer")).all(|line| line.starts_with("alice as doer, asked by carol:")), "nothing of it in bob's name: {asked:?}");
    assert!(asked.iter().any(|line| line.starts_with("bob as relay, asked by carol:")), "the relay acted for its own owner: {asked:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// The agent `reader` of `owner`, which reads the channel "side" with
/// `read_history` once it is asked, in a session alice shares with bob and
/// he opened: "side" is alice's, and `reader` takes part in it if
/// `in_side`; alice said something there, classified as the main channel
/// is. With the main channel.
async fn with_a_reader_of(owner: fn(&World) -> UserId, in_side: bool) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>) {
    use caelumora_model::message::{Message, MessageRole};
    use caelumora_model::tool::{ToolCall, ToolVisibility};

    let internal = Classification::general(Confidentiality::Internal);
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "read_history".to_string(), arguments: serde_json::json!({ "channel": "side" }), visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(internal)
    };
    let world = World::with(&[&TestAgents], |registry| register_once(registry, "reading", call)).await;
    let reading = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "reading"),
            ComponentDescriptor::new("history", CHANNEL_HISTORY_KIND),
        ] },
        ..world.personality("reading", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &reading).await.unwrap();
    let reader = world.agent("reader", owner(&world), &reading).await;
    let session = world.shared_session(&[reader], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let participants = if in_side { vec![ActorId::Agent(reader)] } else { vec![] };
    let side = session.create_channel(&World::as_user(world.alice), ChannelSpec { name: "side".to_string(), components: quiet(), participants, classification: None, language: None, uses_workspace: None }).await.expect("hers");
    side.user_actor(world.alice).expect("she takes part").send_message("only for side".to_string(), None).await.expect("said");
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = world.manager.views(Viewer::new(World::as_user(world.bob))).session(session.id()).await.unwrap();
    assert!(his.channel(side.id()).await.is_err(), "not his to read");
    (world, session, main)
}

/// What the policy is asked when bob asks `reader` in `main` to read
/// "side", once it was asked `line` and a little after: what side holds is
/// asked of entry by entry after the channel as a whole.
async fn asked_to_read_side(world: &World, main: &ChannelLoop, line: &str) -> Vec<String> {
    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@reader what is on side?".to_string(), None).await.expect("sent");
        asked_for(world, line).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }).await;
    asked
}

/// An agent reads another channel with its owner's rights, whoever asked
/// it (issue 255): bob asks alice's `reader` to read her channel "side",
/// which he may not read himself - the agent reaches it as alice, asked by
/// bob (`GetChannel`, issue 264), and copies it as far as the flow rule
/// lets it into the channel he asked in (`CopyInformation`). What the
/// decision accepts: her agent, asked without approval, lets every member
/// read through it what she may.
#[tokio::test]
async fn an_agent_reads_another_channel_with_its_owners_rights_whoever_asked() {
    let (world, session, main) = with_a_reader_of(|world| world.alice, false).await;
    let asked = asked_to_read_side(&world, &main, "alice as reader, asked by bob: CopyInformation yes").await;
    let readers: Vec<&String> = asked.iter().filter(|line| line.contains("as reader")).collect();
    assert!(readers.iter().all(|line| line.starts_with("alice as reader, asked by bob:")), "with her rights, asked by him: {asked:?}");
    assert!(readers.contains(&&"alice as reader, asked by bob: GetChannel(alice's) yes".to_string()), "reached first: {asked:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent does not read a channel of its session that its owner may not
/// read and it takes no part in (issue 264): bob's `reader` does not reach
/// alice's "side" (`GetChannel`), and nothing of it is asked to be copied -
/// not as a whole, though the flow rule would let it, nor entry by entry:
/// a release adds no readers.
#[tokio::test]
async fn an_agent_does_not_read_a_channel_its_owner_may_not_read() {
    let (world, session, main) = with_a_reader_of(|world| world.bob, false).await;
    let asked = asked_to_read_side(&world, &main, "bob as reader: GetChannel(alice's) no").await;
    assert!(!asked.iter().any(|line| line.contains("CopyInformation")), "nothing of side to copy: {asked:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent reads a channel it takes part in from another, though its
/// owner may not read it (issue 264): bob's `reader`, added to alice's
/// "side" by her, reads it from the main channel when bob asks.
#[tokio::test]
async fn an_agent_reads_a_channel_it_takes_part_in_whose_owner_may_not() {
    let (world, session, main) = with_a_reader_of(|world| world.bob, true).await;
    let asked = asked_to_read_side(&world, &main, "bob as reader: CopyInformation yes").await;
    assert!(asked.contains(&"bob as reader: GetChannel(alice's) yes".to_string()), "reached as taking part: {asked:?}");
    assert!(asked.iter().filter(|line| line.contains("CopyInformation")).all(|line| line.ends_with(" yes")), "{asked:?}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- External agents ---------------------------------------------------------

/// A remote agent that takes every request on as a task and works on it
/// for ever - or, given a `gate`, until the gate lets it answer, one permit
/// a request. Every call reads its credential, as an A2A agent does - in
/// the name it is called in - and is recorded. What it brings in itself is
/// classified as it `declared`.
#[derive(Debug)]
struct RemoteAtWork {
    secrets: Arc<SecretManager>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    declared: Option<Classification>,
    gate: Option<Arc<Semaphore>>,
}

impl RemoteAtWork {
    async fn called(&self, call: &'static str, security: &SecurityContext) {
        // Whether there is such a secret does not matter: who is asked does.
        let _ = self.secrets.resolve(security, "REMOTE_TOKEN").await;
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl ExternalAgent for RemoteAtWork {
    async fn send(&self, security: &SecurityContext, _: Option<&ConversationId>, _: OutboundMessage) -> ExternalAgentResult<ExternalStream> {
        self.called("send", security).await;
        let started = futures::stream::iter([Ok(ExternalEvent::Started(TaskId("t-1".to_string())))]);
        let Some(gate) = self.gate.clone() else { return Ok(Box::pin(started.chain(futures::stream::pending()))) };
        let opened = futures::stream::once(async move {
            if let Ok(permit) = gate.acquire_owned().await {
                permit.forget();
            }
        });
        let answered = futures::stream::iter([Ok(ExternalEvent::Message("Found.".to_string())), Ok(ExternalEvent::Completed)]);
        Ok(Box::pin(started.chain(opened.filter_map(|()| async { None })).chain(answered)))
    }

    async fn resume(&self, security: &SecurityContext, _: &TaskId) -> ExternalAgentResult<ExternalStream> {
        self.called("resume", security).await;
        Ok(Box::pin(futures::stream::pending()))
    }

    async fn cancel(&self, security: &SecurityContext, _: &TaskId) -> ExternalAgentResult<()> {
        self.called("cancel", security).await;
        Ok(())
    }

    fn classification(&self) -> Option<Classification> {
        self.declared.clone()
    }

    fn profile(&self) -> ModelProfile {
        ModelProfile::default()
    }
}

/// The world, with the kind `remote_at_work`: a `RemoteAtWork` declaring
/// `declared`, recording its calls in `calls`, working for ever.
async fn with_remote_at_work(calls: &Arc<Mutex<Vec<&'static str>>>, declared: Option<Classification>) -> World {
    with_remote(calls, declared, None).await
}

/// `with_remote_at_work`, answering each request once `gate` lets it, if
/// there is one.
async fn with_remote(calls: &Arc<Mutex<Vec<&'static str>>>, declared: Option<Classification>, gate: Option<Arc<Semaphore>>) -> World {
    let calls = Arc::clone(calls);
    World::with(&[&TestAgents], |registry| registry.register(ComponentKind::new("remote_at_work").provides::<dyn ExternalAgent>().uses::<SecretManager>(), move |_, context| {
        let mut facets = ComponentFacets::empty();
        facets.insert::<dyn ExternalAgent>(Arc::new(RemoteAtWork { secrets: Arc::clone(context.service::<SecretManager>()?), calls: Arc::clone(&calls), declared: declared.clone(), gate: gate.clone() }));
        Ok(facets)
    })).await
}

/// An external personality whose agent is reached as `remote_at_work`.
async fn remote_personality(world: &World) -> Personality {
    let remote = Personality {
        kind: PersonalityKind::External,
        components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("remote", "remote_at_work")] },
        ..world.personality("remote", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &remote).await.unwrap();
    remote
}

/// Waits until `calls` holds `count` calls.
async fn calls_made(calls: &Mutex<Vec<&'static str>>, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.lock().unwrap().len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the remote agent is called");
}

/// An external agent's task is done as the request that began it, as an
/// agent's turn is (issue 211), with its owner's rights (issue 255): asked
/// by bob, the remote agent is sent his request with alice's credential,
/// asked by him - and followed so after a restart, whoever opened the
/// session.
#[tokio::test]
async fn an_external_agents_task_is_followed_for_its_owner_as_whoever_asked_began_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");

    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        calls_made(&calls, 1).await;
    }).await;
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "with alice's credential, asked by bob");

    // The task is stored as begun; the session stops with it still open.
    tokio::time::timeout(Duration::from_secs(5), async {
        while world.store.get_agent_channel_state(hermes, main.id()).await.unwrap().remote.open_tasks.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the task is stored");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();

    let (reopened, asked) = world.asked(async {
        let reopened = world.manager.open(&World::as_user(world.alice), session.id()).await.expect("alice opens it again");
        calls_made(&calls, 2).await;
        reopened
    }).await;
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "resume"]);
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "followed so too, whoever opened the session");
    world.manager.stop(&World::as_user(world.alice), reopened.id()).await.unwrap();
}

/// A session deleted has its external agents leave, as a deleted channel
/// does (issue 377): the open remote task is cancelled, in the name it
/// was begun in - alice's credential, asked by bob - as he asked, though
/// alice deletes the session. A session merely stopped keeps it (above).
#[tokio::test]
async fn deleting_a_session_cancels_its_external_agents_task_as_whoever_asked_began_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while world.store.get_agent_channel_state(hermes, main.id()).await.unwrap().remote.open_tasks.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the task is stored");

    let (deleted, asked) = world.asked(world.manager.delete(&World::as_user(world.alice), session.id())).await;
    assert!(deleted.expect("hers to delete"), "it ran");
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "cancel"], "cancelled before the session's task ended");
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "in the name the task was begun in");
}

/// Asked several times while it answers, an agent answers once afterwards
/// (issue 222) - as asked by the last who asked, not by the first nor by
/// all of them, and with its owner's rights (issue 255): alice's request is
/// dropped, and the remote agent is sent the conversation as asked by bob
/// alone.
#[tokio::test]
async fn an_agent_asked_by_several_while_it_answers_answers_once_as_asked_by_the_last() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Semaphore::new(0));
    let world = with_remote(&calls, None, Some(Arc::clone(&gate))).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch();
    main.user_actor(world.alice).expect("she takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;

    let (_, asked) = world.asked(async {
        // Both ask while it works; both messages are said before it is free.
        main.user_actor(world.alice).expect("she takes part").send_message("@hermes and this".to_string(), None).await.expect("she asks");
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes and that".to_string(), None).await.expect("he asks");
        let said_by = |envelope: &Envelope, sender: ActorId| envelope.context.sender_id == sender
            && matches!(envelope.event, ActorEvent::CompleteMessageGenerated(_));
        let finished_by = |envelope: &Envelope, sender: ActorId| envelope.context.sender_id == sender
            && matches!(envelope.event, ActorEvent::TurnFinished { .. });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !said_by(&events.recv().await.expect("the channel runs"), ActorId::User(world.bob)) {}
        }).await.expect("his message is said");
        gate.add_permits(3);
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut answers = 0;
            while answers < 2 {
                answers += usize::from(finished_by(&events.recv().await.expect("the channel runs"), ActorId::Agent(hermes)));
            }
        }).await.expect("it answers the first request and then the rest");
        // A further answer would call the remote agent right away.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }).await;
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "send"], "one call for all that was asked meanwhile");
    let read_secret: Vec<&String> = asked.iter().filter(|line| line.contains("ReadSecret")).collect();
    assert_eq!(read_secret, ["alice as hermes, asked by bob: ReadSecret yes"], "as asked by the last who asked");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An external agent whose answers, as its component declares them, may no
/// longer enter its channel once the channel was lowered is refused with
/// every request (issue 212): asked with its owner's rights, as asked by
/// whoever asks, the remote agent is not called, and the users are told.
#[tokio::test]
async fn an_external_agent_whose_answers_no_longer_enter_its_lowered_channel_is_refused() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, Some(Classification::general(Confidentiality::Internal))).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch();

    let (failed, asked) = world.asked(async {
        main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                    break match reason {
                        GivenUpReason::Failed { message } => message,
                        other => other.cause().to_string(),
                    };
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("may not be published"), "{failed}");
    let publish: Vec<&String> = asked.iter().filter(|line| line.starts_with("alice as hermes, asked by bob: Publish")).collect();
    assert_eq!(publish, ["alice as hermes, asked by bob: Publish no"], "as its owner, asked by bob");
    assert!(calls.lock().unwrap().is_empty(), "the remote agent is not called");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An external agent's task begun before its channel was lowered is given up
/// as a turn is (issue 212): the remote agent is asked to cancel it as
/// asked by who asked for it, with its owner's rights, and so is whether
/// what came of it may still be published.
#[tokio::test]
async fn lowering_a_channel_gives_up_an_external_agents_task_as_whoever_asked_began_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let world = with_remote_at_work(&calls, None).await;
    let remote = remote_personality(&world).await;
    let hermes = world.agent("hermes", world.alice, &remote).await;
    let session = world.shared_session(&[hermes], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let mut events = main.user_actor(world.bob).expect("he takes part").watch();
    main.user_actor(world.bob).expect("he takes part").send_message("@hermes find it out".to_string(), None).await.expect("and asks");
    calls_made(&calls, 1).await;

    let (failed, asked) = world.asked(async {
        main.reclassify(&World::as_user(world.alice), Classification::general(Confidentiality::Public)).await.expect("hers to lower");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel runs");
                if let ActorEvent::TurnGivenUp { reason, .. } = envelope.event {
                    break match reason {
                        GivenUpReason::Failed { message } => message,
                        other => other.cause().to_string(),
                    };
                }
            }
        }).await.expect("the users are told")
    }).await;
    assert!(failed.contains("lowered"), "{failed}");
    let his: Vec<&String> = asked.iter().filter(|line| line.starts_with("alice as hermes, asked by bob")).collect();
    // The lowering cancels the answer, which cancels its remote task - as
    // every cancel does - and only then is asked whether it may go on.
    assert_eq!(his, ["alice as hermes, asked by bob: ReadSecret yes", "alice as hermes, asked by bob: Publish no"], "cancelled, and asked whether it goes on, as he asked");
    assert_eq!(calls.lock().unwrap().as_slice(), ["send", "cancel"]);
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- The permissions each action needs --------------------------------------
//
// What each action asks of the policy, in whose name, of whose entity, and
// what it is answered - for its owner, for someone taking part, for a
// stranger: the record of which permissions an action needs (issue 198).
// The runtime's own reads in its own name are left out (`World::asked`).

/// Making a session from a template: reading the template, then creating
/// the session - before anything is found or made for it (issue 312) -,
/// reading its owner, everyone's clearance for the channels and the store
/// creating the session again and its channels; then, once it is made,
/// finding or making the agents - creating each, the session read where it
/// may lie, before its personality is read, and the store creating it
/// again - and taking each in as anyone is taken into a running session
/// (issue 227): changing the session and the channel, the agent read, its
/// personality in its own name and its clearance, and the store storing
/// the session and the channel.
#[tokio::test]
async fn making_a_session_needs_the_template_the_personalities_and_creating_what_is_made() {
    let world = agent_testing::world().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let bobs = world.personality("bobs", Some(world.bob), &[]).await;
    let template = world.template(None, &[], "helper", shared.id).await;
    let (alice, carol) = (World::as_user(world.alice), World::as_user(world.carol));

    let (made, asked) = world.asked(world.manager.create_from_template(&alice, template, None, None)).await;
    let session = made.expect("alice makes it");
    assert_eq!(asked, checks(&[
        "alice: GetSessionTemplate(system's) yes",
        "alice: CreateSession(alice's) yes",
        "alice: GetUser(system's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: CreateSession(alice's) yes",
        "alice: CreateChannel(alice's) yes",
        // Its agent, found or made once the session is.
        "alice: List(agent) yes",
        "alice: CreateAgent(alice's) yes",
        "alice: GetPersonality(bob's) yes",
        "alice: CreateAgent(alice's) yes",
        // And taken in: into the session and its channel at once.
        "alice: UpdateSession(alice's) yes",
        "alice: GetChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: UpdateChannel(alice's) yes",
        "alice: GetAgent(alice's) yes",
        "alice as helper: GetPersonality(bob's) yes",
        "alice: JoinChannel(helper) yes",
        "alice: GetSession(alice's) yes",
        "alice: UpdateSession(alice's) yes",
        // The channel takes it in, its actor built.
        "alice: UpdateChannel(alice's) yes",
        "alice as helper: GetPersonality(bob's) yes",
        "alice: JoinChannel(helper) yes",
        "alice: GetChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: UpdateChannel(alice's) yes",
    ]), "the agent's personality is asked for in its name twice: whether it is cleared, and as its actor is built");
    world.manager.stop(&alice, session.id()).await.unwrap();

    // Again: the agent is found, not made - its personality is not read
    // for making it, only in its own name, to run.
    let (again, asked) = world.asked(world.manager.create_from_template(&alice, template, None, None)).await;
    assert!(again.is_ok());
    assert_eq!(asked.iter().filter(|line| line.contains("CreateAgent")).count(), 0, "{asked:?}");
    assert!(asked.contains(&"alice as helper: GetPersonality(bob's) yes".to_string()), "{asked:?}");
    world.manager.stop(&alice, again.unwrap().id()).await.unwrap();

    let private = world.template(Some(world.bob), &[], "private", shared.id).await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&carol, private, None, None)).await;
    assert!(not_found(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["carol: GetSessionTemplate(bob's) no"]), "a template one may not read: nothing more is asked");

    let unusable = world.template(Some(world.alice), &[], "unusable", bobs.id).await;
    let sessions = || async { world.runtime.entity_stores().sessions.list(&World::system(), &Query::all()).await.map(|page| page.items.len()).unwrap() };
    let before = sessions().await;
    let (refused, asked) = world.asked(world.manager.create_from_template(&alice, unusable, None, None)).await;
    assert!(not_found(&refused.unwrap_err()));
    assert_eq!(asked, checks(&[
        "alice: GetSessionTemplate(alice's) yes",
        "alice: CreateSession(alice's) yes",
        "alice: GetUser(system's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: CreateSession(alice's) yes",
        "alice: CreateChannel(alice's) yes",
        "alice: List(agent) yes",
        "alice: GetAgent(alice's) yes",
        "alice: CreateAgent(alice's) yes",
        "alice: GetPersonality(bob's) no",
    ]), "a personality one may not read: no agent - the list is filtered by what she may read, her agent from before");
    assert_eq!(sessions().await, before, "and the session made for it is taken back");
}

/// Seeing an agent on its own (issue 227): who may read it (`GetAgent`)
/// sees it, with whether they may rename (`UpdateAgent`) and delete it
/// (`DeleteAgent`) - its owner, not who it is shared with -, and of the
/// channels it takes part in only those they may read: reading the agent
/// grants nothing of where it takes part. The personalities are those one
/// may read (`GetPersonality`).
#[tokio::test]
async fn an_agent_is_seen_by_who_may_read_it_with_the_channels_they_may_read() {
    let world = agent_testing::world().await;
    let personality = world.personality("shared", Some(world.bob), &[world.alice]).await;
    world.personality("bobs", Some(world.bob), &[]).await;
    let helper = world.agent("helper", world.alice, &personality).await;
    let together = world.session_of(&[world.bob], &[helper], &[]).await;
    let alone = world.session_of(&[], &[helper], &[]).await;
    let (alice, bob) = (Viewer::new(World::as_user(world.alice)), Viewer::new(World::as_user(world.bob)));
    let channels = |found: Vec<ChannelView>| found.iter().map(|channel| channel.session().id()).collect::<Vec<_>>();

    let hers = world.manager.views(alice.clone()).agent(helper).await.expect("hers");
    let seen = hers.snapshot().await.unwrap();
    assert_eq!((seen.owner, seen.personality.as_deref(), seen.can_change, seen.can_delete), (ActorId::User(world.alice), Some("shared"), true, true));
    let mut taking_part = channels(hers.channels().await.unwrap());
    taking_part.sort();
    let mut both = vec![together.id(), alone.id()];
    both.sort();
    assert_eq!(taking_part, both, "the channels of both her sessions");

    assert!(matches!(world.manager.views(bob.clone()).agent(helper).await, Err(ViewError::AgentNotFound(_))), "not his to read");
    assert!(world.manager.views(bob.clone()).agents().await.unwrap().is_empty());

    let mut shared = world.runtime.entity_stores().agents.get(&World::system(), helper).await.unwrap();
    shared.security_attributes = shared.security_attributes.granting(Subject::Actor(ActorId::User(world.bob)), Grant::Read);
    world.runtime.entity_stores().agents.update(&World::system(), &shared).await.unwrap();
    let (seen, asked) = world.asked(async { world.manager.views(bob.clone()).agent(helper).await.expect("shared with him").snapshot().await.unwrap() }).await;
    assert_eq!((seen.can_change, seen.can_delete), (false, false), "read, not changed");
    assert_eq!(asked, checks(&["bob: GetAgent(alice's) yes", "bob: UpdateAgent(alice's) no", "bob: DeleteAgent(alice's) no"]));
    let his = world.manager.views(bob.clone()).agent(helper).await.unwrap();
    assert_eq!(channels(his.channels().await.unwrap()), [together.id()], "only where he may read");

    let names = |listed: Vec<PersonalitySnapshot>| listed.into_iter().map(|p| p.name).collect::<Vec<_>>();
    assert_eq!(names(world.manager.views(alice).personalities().await.unwrap()), ["shared"]);
    assert_eq!(names(world.manager.views(bob).personalities().await.unwrap()), ["bobs", "shared"]);
}

/// Adding an agent to a channel: its personality in its own name - may it
/// run -, its clearance, and changing the channel - its owner's alone.
#[tokio::test]
async fn adding_an_agent_needs_owning_the_channel_and_the_agent_its_personality() {
    let world = agent_testing::world().await;
    let shared = world.personality("shared", Some(world.bob), &[world.alice]).await;
    let (helper, other) = (world.agent("helper", world.alice, &shared).await, world.agent("other", world.alice, &shared).await);
    let session = world.shared_session(&[], &[helper, other]).await;
    let main = Arc::clone(&session.channels()[0]);
    let alice = World::as_user(world.alice);

    let (added, asked) = world.asked(main.add_participant(&alice, ActorId::Agent(helper), HistoryAccess::All)).await;
    assert!(added.is_ok());
    assert_eq!(asked, checks(&[
        "alice: UpdateChannel(alice's) yes",
        "alice as helper: GetPersonality(bob's) yes",
        "alice: JoinChannel(helper) yes",
        "alice: GetChannel(alice's) yes",
        "alice: JoinChannel(alice) yes",
        "alice: UpdateChannel(alice's) yes",
    ]));

    let withdrawn = Personality { security_attributes: attributes(Some(world.bob), &[]), ..shared };
    world.runtime.entity_stores().personalities.update(&World::as_user(world.bob), &withdrawn).await.unwrap();
    let (refused, asked) = world.asked(main.add_participant(&alice, ActorId::Agent(other), HistoryAccess::All)).await;
    assert!(denied(&refused.unwrap_err()));
    assert_eq!(asked, checks(&["alice: UpdateChannel(alice's) yes", "alice as other: GetPersonality(bob's) no"]), "an agent that may not run is not added");
    world.manager.stop(&alice, session.id()).await.unwrap();
}

/// Removing an agent from a channel: changing the channel - its owner's
/// alone; nothing is asked of the agent.
#[tokio::test]
async fn removing_an_agent_needs_owning_the_channel() {
    let world = agent_testing::world().await;
    let global = world.personality("global", None, &[]).await;
    let helper = world.agent("helper", world.alice, &global).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    let (_, asked) = world.asked(main.remove_participant(&World::as_user(world.bob), ActorId::Agent(helper))).await;
    assert_eq!(asked, checks(&["bob: GetChannel(alice's) yes", "bob: JoinChannel(bob) yes", "bob: UpdateChannel(alice's) no"]), "reached by the store, the clearance included (issue 322)");
    let (removed, asked) = world.asked(main.remove_participant(&World::as_user(world.alice), ActorId::Agent(helper))).await;
    assert!(removed.is_ok());
    assert_eq!(asked, checks(&["alice: GetChannel(alice's) yes", "alice: JoinChannel(alice) yes", "alice: UpdateChannel(alice's) yes"]));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Workspaces (issues 251, 253) ---

/// What is asked when the agent of `owner`, asked by bob, makes `call` in the
/// session's workspace, where the files `present` lie already - one that
/// leaves `made`, a file of the workspace.
async fn asked_for_a_call_on_the_workspace(function: &str, arguments: Value, present: &[&str], made: &str, owner: fn(&World) -> UserId) -> Vec<String> {
    let world = World::with(&[&TestAgents], |registry| register_once(registry, ONCE_MODEL_KIND, calling(function, arguments))).await;
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
    use caelumora_model::tool::ToolResultStatus;

    let write = calling("write_file", serde_json::json!({ "path": "notes/a.md", "content": "written" }));
    let world = World::with(&[&TestAgents], |registry| register_once(registry, ONCE_MODEL_KIND, write)).await;
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
        tokio::time::timeout(Duration::from_secs(5), async {
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
        let sets = view.member(ActorId::Agent(helper)).await.unwrap().tool_sets().await.unwrap();
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

/// A template of the system whose sessions have a workspace "files" in
/// `scope`, in `storage`.
async fn template_with_workspace(world: &World, scope: ScopeTemplate, storage: &str) -> SessionTemplateId {
    let global = world.personality(&format!("global-{storage}"), None, &[]).await;
    let id = world.template(None, &[], &format!("helper-{storage}"), global.id).await;
    let mut template = world.runtime.entity_stores().session_templates.get(&World::system(), id).await.unwrap();
    template.workspace = Some(WorkspaceTemplate { name: Name::new("files").unwrap(), scope, storage: storage.to_string(), classification: Classification::general(Confidentiality::Internal), description: None });
    world.runtime.entity_stores().session_templates.update(&World::system(), &template).await.unwrap();
    id
}

/// A session made from a template that names a workspace in the session's
/// scope gets one of its own, made with it and attached (issue 251,
/// decision 13) - its owner's - which is used in that session alone and
/// goes with it: deleted, restored and purged with it, its files too.
#[tokio::test]
async fn a_workspace_of_a_session_s_scope_is_made_with_it_and_goes_with_it() {
    let world = agent_testing::world().await;
    let alice = World::as_user(world.alice);
    let template = template_with_workspace(&world, ScopeTemplate::SessionScope, "main").await;
    let first = world.manager.create_from_template(&alice, template, None, None).await.expect("a session with its workspace");
    let second = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    let (mine, others) = (first.session().workspace.expect("attached"), second.session().workspace.expect("attached"));
    assert_ne!(mine, others, "one for every session");
    let made = world.runtime.entity_stores().workspaces.get(&alice, mine).await.expect("hers");
    assert_eq!(made.name.scope, Scope::SessionScope { session_id: first.id() });

    assert_eq!(made.security_attributes.acl, [AclEntry { subject: Subject::Group(GroupId::of_session(first.id())), grant: Grant::Write }], "its members' to work on (issue 23)");
    assert!(workspace(&world, mine).await.terminal(&alice, Grant::Write).await.is_ok(), "used directly, wherever who may use it acts (issue 23)");
    let refused = second.update_workspace(&alice, Some(mine), None).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::WorkspaceOutOfScope(_))), "nor attached to another session: {refused}");

    world.manager.delete(&alice, first.id()).await.unwrap();
    assert!(world.runtime.entity_stores().workspaces.get_deleted(&alice, mine).await.is_ok(), "deleted with it");
    world.manager.restore(&alice, first.id()).await.unwrap();
    assert!(world.runtime.entity_stores().workspaces.get(&alice, mine).await.is_ok(), "restored with it");
    world.manager.delete(&alice, first.id()).await.unwrap();
    world.manager.purge(&World::system(), first.id()).await.unwrap();
    assert!(!world.files.path().join(mine.to_string()).exists(), "purged with it, its files too");
    assert!(world.files.path().join(others.to_string()).is_dir(), "the other session's stays");
    world.manager.stop(&alice, second.id()).await.unwrap();
}

/// A template that names a workspace in its owner's scope finds it there:
/// every session of the user made from it shares the one (issue 251,
/// decision 13); one of that name in another storage is neither reused
/// nor replaced.
#[tokio::test]
async fn a_workspace_of_the_user_s_scope_is_found_by_every_session_of_theirs() {
    let world = agent_testing::world().await;
    let alice = World::as_user(world.alice);
    let template = template_with_workspace(&world, ScopeTemplate::UserScope, "main").await;
    let first = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    let second = world.manager.create_from_template(&alice, template, None, None).await.unwrap();
    assert_eq!(first.session().workspace, second.session().workspace, "the same one");
    let bobs = world.manager.create_from_template(&World::as_user(world.bob), template, None, None).await.unwrap();
    assert_ne!(bobs.session().workspace, first.session().workspace, "bob's own");

    let elsewhere = template_with_workspace(&world, ScopeTemplate::UserScope, "other").await;
    let refused = world.manager.create_from_template(&alice, elsewhere, None, None).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<TemplateError>(), Some(TemplateError::WorkspaceNameTaken(_))), "{refused}");
    for session in [first, second] {
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
    world.manager.stop(&World::as_user(world.bob), bobs.id()).await.unwrap();
}

/// A workspace chosen as a session is made takes the place of its
/// template's, which is then neither found nor made (issue 266): none; one
/// of its maker's own - asked about as attaching it to a running session,
/// before anything is made, and never another's nor one of another
/// session's scope; or one made for the session alone, in its scope and
/// named as it, which a classification its maker is not cleared for
/// refuses, leaving no session behind.
#[tokio::test]
async fn a_workspace_chosen_as_a_session_is_made_takes_the_place_of_its_template_s() {
    let world = agent_testing::world().await;
    let workspaces = world.runtime.workspace_manager();
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let internal = Classification::general(Confidentiality::Internal);
    let template = template_with_workspace(&world, ScopeTemplate::UserScope, "main").await;
    let hers = workspaces.create(&alice, "hers", Scope::UserScope { user_id: world.alice }, "main", None, internal.clone()).await.unwrap();
    let his = workspaces.create(&bob, "his", Scope::UserScope { user_id: world.bob }, "main", None, internal.clone()).await.unwrap();
    let views = || world.manager.views(Viewer::new(alice.clone()));

    let without = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::None)).await.expect("without one");
    assert_eq!(without.session().workspace, None);

    let (attached, asked) = world.asked(world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: hers.id, share: None }))).await;
    let attached = attached.expect("hers");
    assert_eq!(attached.session().workspace, Some(hers.id));
    assert_eq!(asked[..5], checks(&["alice: GetSessionTemplate(system's) yes", "alice: CreateSession(alice's) yes", "alice: GetWorkspace(alice's) yes", "alice: JoinChannel(alice) yes", "alice: UpdateWorkspace(alice's) yes"]), "{asked:?}");
    let mut making = asked.iter().take_while(|line| !line.contains("CreateChannel"));
    assert!(!making.any(|line| line.contains("UpdateSession")), "attached as she makes the session, not as she changes it: {asked:?}");

    let (refused, asked) = world.asked(world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: his.id, share: None }))).await;
    assert!(not_found(&refused.unwrap_err()), "not bob's");
    assert_eq!(asked, checks(&["alice: GetSessionTemplate(system's) yes", "alice: CreateSession(alice's) yes", "alice: GetWorkspace(bob's) no"]), "refused before anything is made");

    let made = world.manager.create_from_template(&alice, template, Some("Report"), Some(WorkspaceChoice::New { storage: "main".to_string(), classification: internal.clone() })).await.expect("one of its own");
    let own = world.runtime.entity_stores().workspaces.get(&alice, made.session().workspace.expect("attached")).await.unwrap();
    assert_eq!((own.name.name.as_str(), &own.name.scope, own.storage.as_str()), ("Report", &Scope::SessionScope { session_id: made.id() }, "main"));
    assert_eq!(own.owner(), Some(world.alice));
    let refused = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::Existing { id: own.id, share: None })).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::WorkspaceOutOfScope(_))), "not of another session's scope: {refused}");

    let sessions = views().sessions().await.unwrap().len();
    let confidential = Classification::general(Confidentiality::Confidential);
    let refused = world.manager.create_from_template(&alice, template, None, Some(WorkspaceChoice::New { storage: "main".to_string(), classification: confidential })).await.unwrap_err();
    assert!(matches!(refused.downcast_ref::<WorkspaceError>(), Some(WorkspaceError::NotCleared)), "{refused}");
    assert_eq!(views().sessions().await.unwrap().len(), sessions, "no session left behind");

    let names: Vec<String> = views().workspaces().await.unwrap().iter().map(|w| w.name().to_string()).collect();
    assert!(!names.contains(&"files".to_string()), "the template's was never made: {names:?}");
    for session in [without, attached, made] {
        world.manager.stop(&alice, session.id()).await.unwrap();
    }
}

// --- Commands an agent carries out itself (issue 269) ------------------------

/// A component whose agent carries out `/mark`, for whoever may write in the
/// channel, and `/seal`, for its owners alone; it keeps the context of every
/// turn it carries one out in.
#[derive(Debug)]
struct Commanded(Arc<Mutex<Vec<SecurityContext>>>);

impl ProvidesCommands for Commanded {
    fn commands(&self) -> Vec<Command> {
        let command = |name: &str, owners_only| Command { name: name.to_string(), description: String::new(), usage: None, owners_only };
        vec![command("mark", false), command("seal", true)]
    }
}

#[async_trait]
impl AgentCommandProvider for Commanded {
    async fn execute(&self, name: &str, _args: &str, turn: &mut Turn<'_>) -> CommandResult<CommandOutput> {
        self.0.lock().unwrap().push(turn.security_context().clone());
        Ok(CommandOutput::private(format!("{name}: done")))
    }
}

/// A session of alice and bob with alice's `helper`, which carries out the
/// commands of `Commanded`; what its turns ran in, and how bob's stream
/// shows the answers to his commands.
async fn with_a_commanded_agent() -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, Arc<Mutex<Vec<SecurityContext>>>, ChannelWatch) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let world = World::with(&[&TestAgents], |registry| {
        register_once(registry, "silent", Message::new(Classification::PUBLIC));
        let seen = Arc::clone(&seen);
        registry.register(ComponentKind::new("commanded").provides::<dyn AgentCommandProvider>(), move |_, _| {
            let mut facets = ComponentFacets::empty();
            facets.insert::<dyn AgentCommandProvider>(Arc::new(Commanded(Arc::clone(&seen))));
            Ok(facets)
        });
    }).await;
    let commanded = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("commands", "commanded"),
        ] },
        ..world.personality("commanded", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &commanded).await.unwrap();
    let helper = world.agent("helper", world.alice, &commanded).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    (world, session, main, seen, his)
}

/// What was asked about the commands of agents, of `asked`.
fn about_agent_commands(asked: &[String]) -> Vec<&str> {
    asked.iter().filter(|line| line.contains("AgentCommand")).map(String::as_str).collect()
}

/// A command of an agent (issue 269) is seen and asked for by whoever may
/// write in the channel - bob, taking part -, unless it is for the agent's
/// owners alone: `/seal` bob neither sees nor asks for, alice, the agent's
/// owner, does. Bob's actor finds whom he names among the agents by what he
/// sees, and asks nothing more; the agent asks, once, in the context the
/// request came with, as it carries it out (issue 304).
#[tokio::test]
async fn a_bystander_asks_an_agent_for_its_command_but_not_for_one_of_its_owners() {
    let (world, session, main, _, mut his) = with_a_commanded_agent().await;
    let bob = main.user_actor(world.bob).unwrap();

    let ((sent, answer), asked) = world.asked(async {
        let sent = bob.execute_command("mark", "@helper").await;
        (sent, shown(&mut his, "CommandExecuted").await)
    }).await;
    sent.expect("his to ask for");
    assert!(matches!(answer, ActorEvent::CommandExecuted { ref output, .. } if output == "mark: done"), "{answer:?}");
    assert_eq!(about_agent_commands(&asked), [
        "bob: ShowAgentCommand(alice's) yes",
        "bob: ShowAgentCommand(alice's) no",
        "bob: ShowAgentCommand(alice's) yes",
        "bob: ExecuteAgentCommand(alice's) yes",
    ], "what he sees, then the agent asking whether he may ask for it");

    let refused = bob.execute_command("seal", "@helper").await.expect_err("not his");
    assert!(matches!(refused.downcast_ref::<CommandError>(), Some(CommandError::Unknown(name)) if name == "seal"), "unknown to him: {refused}");
    let mut hers = main.user_actor(world.alice).unwrap().watch();
    main.user_actor(world.alice).unwrap().execute_command("seal", "@helper").await.expect("hers, as its owner");
    assert!(matches!(shown(&mut hers, "CommandExecuted").await, ActorEvent::CommandExecuted { ref output, .. } if output == "seal: done"));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// An agent carries a command out for its owner, on the word of whoever
/// asked (issue 255): bob's `/mark` runs in a turn whose caller is the agent,
/// whose principal is alice, and whose initiator is bob.
#[tokio::test]
async fn an_agents_command_runs_for_its_owner_on_the_word_of_whoever_asked() {
    let (world, session, main, seen, mut his) = with_a_commanded_agent().await;
    let helper = main.members().into_iter().find(|m| m.name == "helper").expect("helper takes part").id;

    main.user_actor(world.bob).unwrap().execute_command("mark", "@helper").await.expect("his to ask for");
    shown(&mut his, "CommandExecuted").await;
    let turn = seen.lock().unwrap().pop().expect("carried out");
    assert_eq!((turn.caller_id, turn.principal_id, turn.initiator_id), (helper, ActorId::User(world.alice), ActorId::User(world.bob)));
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- Compaction (issue 16) --------------------------------------------------------

/// Compacting a shared agent's context is its owners' (issue 16, `owners_only`
/// as for approving its calls): bob, who may write in the channel, neither
/// sees nor asks for `/compact` of alice's `helper`; alice does, and the
/// agent carries it out for her - here refusing, since it heard nothing yet.
#[tokio::test]
async fn only_an_agents_owner_compacts_its_context() {
    let world = World::with(&[&TestAgents], |registry| register_once(registry, "silent", Message::new(Classification::PUBLIC))).await;
    let compacting = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("compaction", COMPACTION_KIND),
        ] },
        ..world.personality("compacting", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &compacting).await.unwrap();
    let helper = world.agent("helper", world.alice, &compacting).await;
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
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
