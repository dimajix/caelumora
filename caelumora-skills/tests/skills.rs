//! Skills through the whole runtime (issues 22, 323): the plugin joined to
//! the world of the security suite - the ownership policy, recording what
//! it is asked, and alice, bob and carol (`caelumora_runtime::testing`).
//! The tests of an action that acts in someone's name assert what it asks,
//! as the suite does for the core's.

use caelumora_model::prompt_catalogue::PromptTexts;
use caelumora_model::history::testing::{items_of, messages_of};
use caelumora_model::actor::{ActorEvent, ActorId};
use caelumora_model::agent::{AgentChannelState, AgentId, Personality};
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::confidentiality::{Classification, Confidentiality, Place};
use caelumora_model::context::{KnowledgeRef, KnowledgeSource};
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::scope::Scope;
use caelumora_model::security::{Grant, SecurityAttributes, SecurityContext, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::query::Query;
use caelumora_model::store::{EntityStore, EntityStoreFactory};
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::tool::{ToolCall, ToolVisibility};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_runtime::actor::user::ChannelWatch;
use caelumora_agents::turn::Turn;
use caelumora_runtime::channel::ChannelLoop;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::compaction::KnowledgeRestorer;
use caelumora_agents::prompt::{PromptPosition, PromptProvider};
use caelumora_runtime::session::SessionLoop;
use caelumora_agents::testing::{self as agent_testing, register_once, TestAgents};
use caelumora_runtime::testing::{asked_for, shown, World, STAFF};
use caelumora_agents::tool::{ApprovalMode, ToolError, ToolOutput, ToolProvider, ToolResult};
use caelumora_runtime::view::Viewer;
use caelumora_skills::component::{SkillsComponent, CREATE_SKILL_TOOL_NAME, LIST_SKILLS_TOOL_NAME, LOAD_SKILL_TOOL_NAME, SKILLS_KIND, SKILL_COMMAND_NAME, UPDATE_SKILL_TOOL_NAME};
use caelumora_skills::manager::{SkillError, SkillManager};
use caelumora_skills::plugin::SkillsPlugin;
use caelumora_skills::skill::{self, Skill, SkillBody};
use caelumora_skills::view::SkillViews;
use serde_json::Value;
use std::sync::Arc;

/// The world with skills.
async fn world() -> World {
    World::with(&[&TestAgents, &SkillsPlugin], |_| {}).await
}

/// The one writer of skills of `world`.
fn manager(world: &World) -> Arc<SkillManager> {
    Arc::clone(world.runtime.services().get::<SkillManager>().expect("a service of the plugin"))
}

/// The store of skills of `world`, as the plugin has it.
fn store(world: &World) -> Arc<dyn EntityStore<Skill>> {
    world.store.entity_store::<Skill>().unwrap()
}

fn component(world: &World) -> SkillsComponent {
    SkillsComponent::new(PromptPosition::Start, ApprovalMode::Always, PromptTexts::new(Arc::clone(world.runtime.prompts()), None)).unwrap()
}

/// A session of alice and bob with alice's agent `helper`, in a channel
/// classified internal.
async fn with_an_agent(world: &World) -> (Arc<SessionLoop>, AgentId) {
    let personality = world.personality("plain", None, &[]).await;
    let helper = world.agent("helper", world.alice, &personality).await;
    (world.shared_session(&[helper], &[]).await, helper)
}

async fn call(component: &SkillsComponent, turn: &mut Turn<'_>, name: &str, params: Value) -> ToolResult<ToolOutput> {
    ToolProvider::execute(component, name, &params, None, turn).await
}

// --- The component --------------------------------------------------------

#[tokio::test]
async fn the_catalogue_lists_what_may_be_loaded_and_load_skill_gives_its_text_as_knowledge() {
    let world = world().await;
    let (session, helper) = with_an_agent(&world).await;
    let component = component(&world);
    let mut state = AgentChannelState::default();
    let prompt = component.get_prompt(&agent_testing::turn(&session, helper, world.alice, &mut state).situation()).await.unwrap();
    assert_eq!(prompt.text, "", "nothing to list, no part");

    let internal = Classification::general(Confidentiality::Internal);
    let made = manager(&world)
        .create(&SecurityContext::system(), "release-notes", Scope::GlobalScope, "How we write a release note", "1. Collect.\n2. Group.", internal.clone()).await.unwrap();
    let mut state = AgentChannelState::default();
    let prompt = component.get_prompt(&agent_testing::turn(&session, helper, world.alice, &mut state).situation()).await.unwrap();
    assert!(prompt.text.starts_with("[Skills you can load]") && prompt.text.contains("- release-notes: How we write a release note"), "{}", prompt.text);
    assert!(!prompt.text.contains("Collect"), "the catalogue, not the text");
    assert_eq!(prompt.classification, Some(internal.clone()));

    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    let listed = call(&component, &mut turn, LIST_SKILLS_TOOL_NAME, serde_json::json!({})).await.unwrap();
    assert_eq!(listed.value, serde_json::json!({ "skills": [{ "name": "release-notes", "description": "How we write a release note", "skill_id": made.id }] }), "each with its id, for a frontend to link to");
    assert_eq!(listed.classification, Some(internal.clone()));
    drop(turn);

    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    let loaded = call(&component, &mut turn, LOAD_SKILL_TOOL_NAME, serde_json::json!({ "name": "release-notes" })).await.unwrap();
    assert_eq!(loaded.value, serde_json::json!({ "status": "loaded", "name": "release-notes", "skill_id": made.id }), "the result only names it");
    assert_eq!(loaded.classification, Some(internal.clone()), "classified as the skill, as the catalogue names it");
    drop(turn);
    let read = state.turn.as_ref().unwrap().work.context.frame.knowledge.clone();
    let as_read = |source: &KnowledgeSource| source.about::<Skill>() == Some((made.id, Some(skill::digest("1. Collect.\n2. Group."))));
    assert!(matches!(read.as_slice(), [KnowledgeRef { source, .. }] if as_read(source)), "{read:?}");
    let given = state.transcript.last().map(|node| &node.entry);
    assert!(matches!(given, Some(TranscriptEntry::Given { message, source: Some(source) }) if as_read(source) && message.content.as_deref() == Some("[Skill: release-notes]\n1. Collect.\n2. Group.\n") && message.classification == internal),
        "its text given to read, under its title, as what it is (issue 389): {given:?}");

    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    let absent = call(&component, &mut turn, LOAD_SKILL_TOOL_NAME, serde_json::json!({ "name": "nothing" })).await;
    assert!(matches!(absent, Err(ToolError::InvalidParameter(_))), "the model's to correct");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

#[tokio::test]
async fn an_agent_writes_a_skill_of_its_owners_classified_as_what_it_saw_and_raises_one_it_changes() {
    let world = world().await;
    let (session, helper) = with_an_agent(&world).await;
    let component = component(&world);
    let mut state = AgentChannelState::default();
    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    let created = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.", "scope": "session" })).await.unwrap();
    assert_eq!(created.value["status"], "created");
    let created_id = created.value["skill_id"].clone();
    let global = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "other", "description": "d", "body": "b", "scope": "global" })).await;
    assert!(matches!(global, Err(ToolError::InvalidParameter(_))), "never global");
    let twice = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "Deploy", "description": "d", "body": "b", "scope": "session" })).await;
    assert!(matches!(twice, Err(ToolError::InvalidParameter(ref m)) if m.contains("update it instead")), "{twice:?}");
    let mine = call(&component, &mut turn, CREATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "notes", "description": "How we take notes", "body": "Short.", "scope": "user" })).await;
    assert!(mine.is_ok(), "{mine:?}");
    drop(turn);

    let skills = store(&world).list(&SecurityContext::system(), &Query::all()).await.map(|page| page.items).unwrap();
    let deploy = skills.iter().find(|s| s.name.name.as_str() == "deploy").expect("made");
    let owner = ActorId::User(world.alice);
    assert_eq!((deploy.security_attributes.owner_id, &deploy.name.scope), (owner, &Scope::SessionScope { session_id: session.id() }), "its owner's, in the session");
    assert_eq!(deploy.classification, Classification::PUBLIC, "what the turn had seen: nothing above public");
    let notes = skills.iter().find(|s| s.name.name.as_str() == "notes").expect("made");
    assert_eq!(notes.name.scope, Scope::UserScope { user_id: world.alice });

    // A turn that saw something internal writes it into the skill: raised first.
    let internal = Classification::general(Confidentiality::Internal);
    let mut state = AgentChannelState::default();
    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    turn.see(&internal);
    let updated = call(&component, &mut turn, UPDATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy", "body": "1. Build.\n2. Ship." })).await.unwrap();
    assert_eq!(updated.value["status"], "updated");
    assert_eq!(updated.value["skill_id"], created_id, "the skill it changed, for a frontend to link to");
    let nothing = call(&component, &mut turn, UPDATE_SKILL_TOOL_NAME, serde_json::json!({ "name": "deploy" })).await;
    assert!(matches!(nothing, Err(ToolError::InvalidParameter(_))), "nothing to change");
    drop(turn);
    let deploy = store(&world).get(&SecurityContext::system(), deploy.id).await.unwrap();
    assert_eq!((deploy.body.as_str(), &deploy.classification), ("1. Build.\n2. Ship.", &internal), "the new steps, classified as what was seen");
    assert_eq!(created_id, serde_json::json!(deploy.id), "the skill it made");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

#[tokio::test]
async fn an_approver_is_shown_the_skill_whole() {
    let world = world().await;
    let component = component(&world);
    let shown = component.describe(CREATE_SKILL_TOOL_NAME, &serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.\n2. Ship.", "scope": "user" })).unwrap();
    assert_eq!(shown, "Write the skill \"deploy\" (for every session of the agent's owner): How we deploy\n\n1. Build.\n2. Ship.");
    let tools = component.tools();
    assert!(tools.iter().filter(|t| t.name == CREATE_SKILL_TOOL_NAME || t.name == UPDATE_SKILL_TOOL_NAME).all(|t| t.requires_approval), "approved by default");
    let unapproved = SkillsComponent::new(PromptPosition::Start, ApprovalMode::Never, PromptTexts::new(Arc::clone(world.runtime.prompts()), None)).unwrap().tools();
    assert!(unapproved.iter().all(|t| !t.requires_approval), "unless configured otherwise");
}

#[tokio::test]
async fn load_skill_shows_the_channel_which_skill_it_loaded_not_what_it_gave() {
    let world = world().await;
    use caelumora_skills::component::{SKILL_LIST_PRESENTATION, SKILL_PRESENTATION};
    let tools = component(&world).tools();
    let load = tools.iter().find(|t| t.name == LOAD_SKILL_TOOL_NAME).unwrap();
    assert_eq!(load.visibility, ToolVisibility::SHOWN, "its result names the skill; its text is given to the agent, not published (issue 389)");
    let skills: Vec<_> = tools.iter().filter(|t| t.presentation == SKILL_PRESENTATION).map(|t| t.name.as_str()).collect();
    assert_eq!(skills, [LOAD_SKILL_TOOL_NAME, CREATE_SKILL_TOOL_NAME, UPDATE_SKILL_TOOL_NAME], "shown as a skill");
    let list = tools.iter().find(|t| t.name == LIST_SKILLS_TOOL_NAME).unwrap();
    assert_eq!((list.presentation, list.visibility), (SKILL_LIST_PRESENTATION, ToolVisibility::SHOWN), "the catalogue as a list of skills");
    assert!(tools.iter().filter(|t| t.name == LOAD_SKILL_TOOL_NAME || t.name == LIST_SKILLS_TOOL_NAME).all(|t| !t.requires_approval), "reading needs no approval");
}

/// Issue 323: a skill given before a compaction is given again as it is
/// now - one that can no longer be loaded where the agent is is not.
#[tokio::test]
async fn the_skills_given_are_given_again_where_they_may_still_be_loaded() {
    let world = world().await;
    let (session, helper) = with_an_agent(&world).await;
    let internal = Classification::general(Confidentiality::Internal);
    let manager = manager(&world);
    let kept = manager.create(&SecurityContext::system(), "release-notes", Scope::GlobalScope, "How we write a release note", "1. Collect.", internal.clone()).await.unwrap();
    let gone = manager.create(&SecurityContext::system(), "deploy", Scope::GlobalScope, "How we deploy", "1. Build.", internal).await.unwrap();
    let mut state = AgentChannelState::default();
    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    AgentCommandProvider::execute(&component(&world), SKILL_COMMAND_NAME, "release-notes", &mut turn).await.expect("given");
    manager.rewrite(&SecurityContext::system(), kept.id, "1. Collect.\n2. Group.").await.unwrap();
    manager.delete(&SecurityContext::system(), gone.id).await.unwrap();

    let again = component(&world).give_again(&mut turn, &[gone.id.0, kept.id.0]).await.unwrap();
    let again: Vec<_> = again.iter().map(|given| (given.name.as_str(), given.text.as_str())).collect();
    assert_eq!(again, [("release-notes", "[Skill: release-notes]\n1. Collect.\n2. Group.\n")], "as it is now; not the deleted one");
    drop(turn);
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

// --- The views -------------------------------------------------------------

#[tokio::test]
async fn an_agent_loads_the_skills_its_owner_may_use_that_flow_into_its_channel_the_innermost_of_each_name() {
    let world = world().await;
    let (alice, bob) = (world.alice, world.bob);
    let personality = world.personality("plain", None, &[]).await;
    let helper = world.agent("helper", bob, &personality).await;
    let (session_id, channel) = (SessionId::new(), ChannelId::new());
    let skill = |name: &str, scope: Scope, owner: ActorId, classification: Classification| Skill::new(
        ScopedName::new(name, scope).unwrap(), Description::new(format!("{name}, described")).unwrap(),
        SkillBody::new(format!("{name}: how it is done")).unwrap(), classification, owner,
    );
    let secret = Classification::general(Confidentiality::StrictlyConfidential);
    for made in [
        skill("notes", Scope::GlobalScope, ActorId::System, Classification::CHANNEL_DEFAULT),
        skill("notes", Scope::SessionScope { session_id }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
        skill("deploy", Scope::UserScope { user_id: bob }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
        skill("secret", Scope::UserScope { user_id: bob }, ActorId::User(bob), secret),
        skill("hers", Scope::GlobalScope, ActorId::User(alice), Classification::CHANNEL_DEFAULT),
        Skill { security_attributes: SecurityAttributes::owned_by(ActorId::User(alice)).granting(Subject::Actor(ActorId::User(bob)), Grant::Read), ..skill("review", Scope::GlobalScope, ActorId::User(alice), Classification::CHANNEL_DEFAULT) },
        skill("elsewhere", Scope::SessionScope { session_id: SessionId::new() }, ActorId::User(bob), Classification::CHANNEL_DEFAULT),
    ] {
        store(&world).create(&World::system(), &made).await.unwrap();
    }

    let context = SecurityContext::new(ActorId::User(bob), session_id, channel).next(ActorId::Agent(helper), ActorId::User(bob), ActorId::User(bob)).with_roles(vec![*STAFF], None);
    let from = Place { channel, classification: Classification::CHANNEL_DEFAULT, session: SessionPrivacy { id: session_id, ..SessionPrivacy::default() } };
    let views = world.manager.views(Viewer::from_place(context, from));
    let usable: Vec<(String, Scope)> = views.usable_skills().await.unwrap().iter().map(|s| (s.name().to_string(), s.scope().clone())).collect();
    assert_eq!(usable, [
        ("deploy".to_string(), Scope::UserScope { user_id: bob }),
        ("notes".to_string(), Scope::SessionScope { session_id }),
        ("review".to_string(), Scope::GlobalScope),
    ], "its owner's, the session's before the global one of a name, one shared with him; not one above the channel, not another's, not another session's");
    assert_eq!(views.usable_skill_named("Notes").await.unwrap().body().as_str(), "notes: how it is done", "found as names are compared");
    for absent in ["secret", "hers", "elsewhere", "nothing", ""] {
        assert!(matches!(views.usable_skill_named(absent).await, Err(SkillError::NoSkillNamed(_))), "{absent:?} answers as absent");
    }
    assert!(world.manager.views(Viewer::new(World::as_user(bob))).usable_skills().await.unwrap().is_empty(), "a user loads none: skills are loaded into turns");
}

// --- The seed --------------------------------------------------------------

#[tokio::test]
async fn the_skills_of_a_seed_are_the_systems_and_seeded_once() {
    let world = world().await;
    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("seed.yaml");
    std::fs::write(&seed, "skills:\n- id: 4f9a5e56-e5ea-43f9-9114-2098a6f417a9\n  name: quiz\n  description: How to run a quiz\n  body: Ask.\n  classification: { level: internal }\n").unwrap();
    let seeds = [seed];
    world.runtime.seed(&seeds).await.unwrap();
    world.runtime.seed(&seeds).await.expect("seeding again is no conflict");
    let skills = store(&world).list(&World::system(), &Query::all()).await.map(|page| page.items).unwrap();
    assert_eq!(skills.len(), 1, "once");
    assert_eq!(skills[0].security_attributes.owner_id, ActorId::System, "the system's");
}

// --- Who may do what (issue 22, the security suite) -------------------------

/// A session of alice and bob with alice's `helper`, which has the `skills`
/// component, and a skill of alice's, `release-notes`, classified as the
/// channel; and how bob's stream shows him what comes of his commands.
async fn with_a_skilled_agent() -> (World, Arc<SessionLoop>, Arc<ChannelLoop>, AgentId, ChannelWatch) {
    let world = World::with(&[&TestAgents, &SkillsPlugin], |registry| register_once(registry, "silent", Message::new(Classification::PUBLIC))).await;
    let skilled = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "silent"),
            ComponentDescriptor::new("skills", SKILLS_KIND),
        ] },
        ..world.personality("skilled", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &skilled).await.unwrap();
    let helper = world.agent("helper", world.alice, &skilled).await;
    let internal = Classification::general(Confidentiality::Internal);
    manager(&world)
        .create(&World::as_user(world.alice), "release-notes", Scope::UserScope { user_id: world.alice }, "How we write a release note", "1. Collect.\n2. Group.", internal).await
        .expect("hers to make");
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    let his = main.user_actor(world.bob).expect("he takes part").watch();
    (world, session, main, helper, his)
}

/// A skill loaded by a command (issue 22) is loaded by the agent for its
/// owner, on the word of whoever asked: bob's `/skill` loads alice's skill
/// into her agent - `UseSkill` asked as the agent for alice, bob the
/// initiator. Its text goes into the agent's transcript in bob's name, and
/// the channel's history records the command, not the text.
#[tokio::test]
async fn an_agent_loads_a_skill_for_its_owner_whoever_asked() {
    let (world, session, main, helper, mut his) = with_a_skilled_agent().await;

    let (answer, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().execute_command("skill", "@helper release-notes").await.expect("his to give");
        shown(&mut his, "CommandExecuted").await
    }).await;
    assert!(matches!(answer, ActorEvent::CommandExecuted { ref output, .. } if output.contains("release-notes")), "{answer:?}");
    let using: Vec<&String> = asked.iter().filter(|line| line.contains("UseSkill")).collect();
    assert_eq!(using, ["alice as helper, asked by bob: UseSkill(alice's) yes"], "for its owner, asked by him");

    let state = world.store.state_store().get_agent_channel_state(helper, main.id()).await.unwrap();
    let given = state.transcript.messages().into_iter().last().expect("given");
    assert!(given.content.as_deref().is_some_and(|c| c.starts_with("[Skill: release-notes]\n1. Collect.")), "{given:?}");
    assert_eq!((given.sender_id, given.role), (ActorId::User(world.bob), MessageRole::User), "in his name");
    let history = world.store.state_store().get_channel_state(main.id()).await.unwrap().history;
    assert!(messages_of(&history).iter().all(|m| !m.content.as_deref().unwrap_or_default().contains("Collect.")), "nothing of the text in the channel's history");
    assert!(items_of(&history).iter().any(|item| matches!(item, HistoryEntry::CommandExecuted { command, issuer, .. } if command == "skill" && *issuer == ActorId::User(world.bob))), "but the command");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A skill classified above the channel (issue 22) may not flow into it:
/// the agent neither lists it nor loads it, and answers for it as for one
/// that does not exist.
#[tokio::test]
async fn a_skill_classified_above_the_channel_is_neither_listed_nor_loaded() {
    let (world, session, main, _, mut his) = with_a_skilled_agent().await;
    manager(&world)
        .create(&World::system(), "secret-plan", Scope::GlobalScope, "What is not said", "Nothing.", Classification::general(Confidentiality::StrictlyConfidential)).await
        .expect("the system's");
    let bob = main.user_actor(world.bob).unwrap();

    bob.execute_command("skills", "@helper").await.expect("his to ask");
    let ActorEvent::CommandExecuted { output, .. } = shown(&mut his, "CommandExecuted").await else { unreachable!() };
    assert!(output.contains("release-notes") && !output.contains("secret-plan"), "{output}");

    bob.execute_command("skill", "@helper secret-plan").await.expect("sent");
    let ActorEvent::CommandFailed { reason, .. } = shown(&mut his, "CommandFailed").await else { unreachable!() };
    assert!(reason.contains("no skill named \"secret-plan\""), "as absent: {reason}");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A session of alice and bob with alice's `helper`, whose model calls the
/// tool `function` with `arguments` once it is asked, and which writes
/// skills without approval; and a skill of bob's, `review`, shared with
/// alice.
async fn with_a_writing_agent(function: &str, arguments: Value) -> (World, Arc<SessionLoop>, Arc<ChannelLoop>) {
    let call = Message {
        role: MessageRole::Assistant,
        tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: function.to_string(), arguments, visibility: ToolVisibility::SHOWN, presentation: Default::default() }],
        ..Message::new(Classification::general(Confidentiality::Internal))
    };
    let world = World::with(&[&TestAgents, &SkillsPlugin], |registry| register_once(registry, "writing", call)).await;
    let writing = Personality {
        components: ComponentConfig { component_nodes: vec![
            ComponentDescriptor::new("model", "writing"),
            ComponentDescriptor { params: serde_json::json!({ "requires_approval": false }), ..ComponentDescriptor::new("skills", SKILLS_KIND) },
        ] },
        ..world.personality("writing", None, &[]).await
    };
    world.runtime.entity_stores().personalities.update(&World::system(), &writing).await.unwrap();
    let helper = world.agent("helper", world.alice, &writing).await;
    let review = manager(&world)
        .create(&World::as_user(world.bob), "review", Scope::GlobalScope, "How we review", "Read it twice.", Classification::general(Confidentiality::Internal)).await
        .expect("his to make");
    let shared = Skill { security_attributes: SecurityAttributes::owned_by(ActorId::User(world.bob)).granting(Subject::Actor(ActorId::User(world.alice)), Grant::Read), ..review };
    store(&world).update(&World::as_user(world.bob), &shared).await.expect("shared with alice");
    let session = world.shared_session(&[helper], &[]).await;
    let main = Arc::clone(&session.channels()[0]);
    world.manager.open(&World::as_user(world.bob), session.id()).await.expect("he opens it");
    (world, session, main)
}

/// A skill an agent writes (issue 22) is its owner's - asked as the agent
/// for its owner, whoever asked - and lies in the session's scope it asked
/// for, classified as what its turn had seen.
#[tokio::test]
async fn an_agents_skill_belongs_to_its_owner_and_lies_in_the_session() {
    let arguments = serde_json::json!({ "name": "deploy", "description": "How we deploy", "body": "1. Build.", "scope": "session" });
    let (world, session, main) = with_a_writing_agent("create_skill", arguments).await;

    let (_, asked) = world.asked(async {
        main.user_actor(world.bob).unwrap().send_message("@helper write it down".to_string(), None).await.expect("sent");
        asked_for(&world, "alice as helper, asked by bob: CreateSkill(alice's) yes").await;
    }).await;
    assert!(asked.iter().any(|line| line == "alice as helper, asked by bob: CreateSkill(alice's) yes"), "{asked:?}");
    let skills = store(&world).list(&World::system(), &Query::all()).await.map(|page| page.items).unwrap();
    let deploy = skills.iter().find(|s| s.name.name.as_str() == "deploy").expect("written");
    assert_eq!(deploy.security_attributes.owner_id, ActorId::User(world.alice), "the agent's owner's");
    assert_eq!(deploy.name.scope, Scope::SessionScope { session_id: session.id() }, "in the session's scope");
    assert!(deploy.classification.surely_flows_to(&main.classification()), "classified as what the turn saw, never above the channel");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A skill shared with the agent's owner is loaded, not changed: changing a
/// skill (`UpdateSkill`) is its owner's alone, also through an agent.
#[tokio::test]
async fn an_agent_changes_only_skills_of_its_owners() {
    let arguments = serde_json::json!({ "name": "review", "body": "Skip it." });
    let (world, session, main) = with_a_writing_agent("update_skill", arguments).await;

    main.user_actor(world.bob).unwrap().send_message("@helper change it".to_string(), None).await.expect("sent");
    asked_for(&world, "alice as helper, asked by bob: UpdateSkill(bob's) no").await;
    let review = store(&world).find_by_name(&World::system(), &ScopedName::new("review", Scope::GlobalScope).unwrap()).await.unwrap();
    assert_eq!(review.body.as_str(), "Read it twice.", "unchanged");
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// A skill of a session's scope (issue 178) goes with the session: deleted
/// with it and restored with it - the plugin's manager a member of the
/// scope (issue 323).
#[tokio::test]
async fn a_skill_of_a_sessions_scope_is_deleted_and_restored_with_it() {
    let world = world().await;
    let (session, _) = with_an_agent(&world).await;
    let alice = World::as_user(world.alice);
    let made = manager(&world)
        .create(&alice, "deploy", Scope::SessionScope { session_id: session.id() }, "How we deploy", "1. Build.", Classification::general(Confidentiality::Internal)).await
        .expect("hers, in her session");
    let id = session.id();
    drop(session);

    world.manager.delete(&alice, id).await.expect("hers to delete");
    assert!(store(&world).get(&World::system(), made.id).await.is_err(), "deleted with it");
    world.manager.restore(&alice, id).await.expect("hers to restore");
    assert!(store(&world).get(&World::system(), made.id).await.is_ok(), "restored with it");
}
