//! Artifacts through the whole runtime (issues 271, 23, 316, 328): the
//! plugin joined to the world of the security suite - the ownership
//! policy, recording what it is asked, and alice, bob and carol
//! (`caelumora_runtime::testing`), whose storage of content "kept" lies
//! beside the workspaces' files. The tests of an action that acts in someone's name
//! assert what it asks, as the suite does for the core's.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test code: what fails here fails the test that runs it")]

use axum::body::{to_bytes, Body};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use caelumora_artifacts::artifact::{Artifact, ArtifactId, PublicationSnapshot};
use caelumora_artifacts::component::{ArtifactTool, ARTIFACT_PUBLISH_PRESENTATION, ARTIFACT_TOOL_KIND, PUBLISH_ARTIFACT};
use caelumora_artifacts::manager::{ArtifactError, ArtifactManager};
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_artifacts::view::{ArtifactChannelViews, ArtifactProjectViews, ArtifactViews};
use caelumora_model::prompt_catalogue::PromptTexts;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::digest;
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId};
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, Refused, Secured, SecurityAttributes, SecurityContext, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::config::{AppConfig, AttachmentsConfig, AuthConfig, BackendConfig, ContentConfig};
use caelumora_model::store::{EntityStore, EntityStoreFactory, StoreError, Stored};
use caelumora_model::user_provider::accept_any::AcceptAnyUserProvider;
use caelumora_model::user_provider::UserManager;
use caelumora_model::user::UserId;
use caelumora_model::project::Project;
use caelumora_model::value::name::ScopedName;
use caelumora_model::workspace::Workspace;
use caelumora_rest_server::auth::{Auth, TokenService};
use caelumora_rest_server::error::ApiError;
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes::api;
use caelumora_rest_server::state::AppState;
use caelumora_runtime::backend::StorageGone;
use caelumora_runtime::content::ContentError;
use caelumora_runtime::event::{EventBus, EventSubscription};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::scoped::ScopeNotEmpty;
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop};
use caelumora_runtime::terminal::{ByteStream, WorkspacePath};
use caelumora_agents::testing::{self as agent_testing, asked_as_a_call_on_the_workspace, asked_for_a_call, calling, register_scripted, TestAgents, ONCE_MODEL_KIND};
use caelumora_runtime::testing::{quiet, TestRuntime, World, STAFF};
use caelumora_agents::tool::{ToolError, ToolProvider};
use caelumora_runtime::view::Viewer;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use bytes::Bytes;
use futures::stream;
use std::fs;
use tokio::time;

/// The world with artifacts.
async fn world() -> World {
    World::with(&[&TestAgents, &ArtifactsPlugin], |_| {}).await
}

/// The one owner of artifacts of `world`.
fn manager(world: &World) -> Arc<ArtifactManager> {
    Arc::clone(world.runtime.services().get::<ArtifactManager>().expect("a service of the plugin"))
}

/// The store of artifacts of `world`, as the plugin has it.
fn store(world: &World) -> Arc<dyn EntityStore<Artifact>> {
    world.store.entity_store::<Artifact>().unwrap()
}

fn checks(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| line.to_string()).collect()
}

fn file(text: &'static str) -> ByteStream {
    Box::pin(stream::iter([Ok(Bytes::from_static(text.as_bytes()))]))
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
    }).await;
    assert!(missing.is_empty(), "not heard: {missing:?}");
}

// --- The component --------------------------------------------------------

/// A session of alice's and bob's with alice's agent `helper` and alice's
/// workspace `docs` attached for writing, and the tool of a component
/// configured with `params`.
async fn with_the_tool(world: &World, params: Value) -> (Arc<SessionLoop>, AgentId, Workspace, ArtifactTool) {
    let alice = World::as_user(world.alice);
    let personality = world.personality("plain", None, &[]).await;
    let helper = world.agent("helper", world.alice, &personality).await;
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[helper], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let tool = ArtifactTool::new(&params, manager(world), &PromptTexts::new(Arc::clone(world.runtime.prompts()), None)).expect("parameters it takes");
    (session, helper, docs, tool)
}

#[tokio::test]
async fn a_file_is_published_as_versions_of_an_artifact_kept_apart_from_the_workspace() {
    let world = world().await;
    let (session, helper, docs, tool) = with_the_tool(&world, json!({})).await;
    let mut state = AgentChannelState::default();
    let mut turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    let dir = world.files.path().join(docs.id.to_string());
    fs::create_dir_all(dir.join("out")).unwrap();
    fs::write(dir.join("out/report.csv"), "a,b").unwrap();

    let first = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "description": "The figures" }), None, &mut turn).await.unwrap();
    assert_eq!(first.classification.as_ref(), Some(&docs.classification), "classified as the workspace");
    let first = first.value;
    assert_eq!((first["name"].as_str(), first["version"].as_u64(), first["size"].as_u64(), first["description"].as_str()), (Some("report.csv"), Some(1), Some(3), Some("The figures")));
    assert_eq!(first["digest"], digest::content(b"a,b").to_string());

    fs::write(dir.join("out/report.csv"), "a,b\n1,2").unwrap();
    let second = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "name": "report.csv" }), None, &mut turn).await.unwrap().value;
    assert_eq!((second["artifact"].clone(), second["version"].as_u64()), (first["artifact"].clone(), Some(2)), "a version of the same artifact");
    let kept = world.files.path().join("content").join("artifacts").join(first["artifact"].as_str().unwrap());
    assert_eq!(fs::read_to_string(kept.join("1")).unwrap(), "a,b", "the first as it was");
    assert_eq!(fs::read_to_string(kept.join("2")).unwrap(), "a,b\n1,2");
    fs::remove_file(dir.join("out/report.csv")).unwrap();
    assert!(kept.join("2").is_file(), "kept beyond the file");

    let missing = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv" }), None, &mut turn).await;
    assert!(matches!(missing, Err(ToolError::InvalidParameter(_))), "{missing:?}");
    let directory = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out" }), None, &mut turn).await;
    assert!(matches!(directory, Err(ToolError::InvalidParameter(_))), "{directory:?}");
    fs::write(dir.join("out/x.csv"), "x").unwrap();
    let long = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/x.csv", "name": "x".repeat(101) }), None, &mut turn).await;
    assert!(matches!(long, Err(ToolError::InvalidParameter(_))), "a name of at most 100 characters: {long:?}");
    drop(turn);
    world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
}

/// Issue 290: the tool is offered where a file can be published - where
/// the turn may use a workspace in its channel - and not where it may not,
/// saying why.
#[tokio::test]
async fn the_tool_is_offered_where_the_channel_uses_a_workspace() {
    let world = world().await;
    let (session, helper, _, tool) = with_the_tool(&world, json!({})).await;
    let mut state = AgentChannelState::default();
    let turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    assert_eq!(tool.offered(&turn.situation()).await.map(|tools| tools.len()), Ok(1), "offered where it is used");
    drop(turn);

    let alice = World::as_user(world.alice);
    session.channels()[0].update_uses_workspace(&alice, false).await.unwrap();
    let mut state = AgentChannelState::default();
    let turn = agent_testing::turn(&session, helper, world.alice, &mut state);
    assert_eq!(tool.offered(&turn.situation()).await, Err("there is no workspace you can use in this channel".to_string()), "not where the channel uses none");
    drop(turn);
    world.manager.stop(&alice, session.id()).await.unwrap();
}

#[tokio::test]
async fn a_publication_is_approved_as_a_change_and_presented_as_one() {
    let world = world().await;
    let tool = |params: Value| ArtifactTool::new(&params, manager(&world), &PromptTexts::new(Arc::clone(world.runtime.prompts()), None)).unwrap();
    let asks = |mode: Value| tool(json!({ "requires_approval": mode })).tools()[0].requires_approval;
    assert!(asks(json!(true)) && asks(json!("unless_read_only")), "it changes what is kept");
    assert!(!asks(json!(false)));
    assert!(tool(json!({})).tools()[0].requires_approval, "unless configured");
    assert_eq!(tool(json!({})).tools()[0].presentation, ARTIFACT_PUBLISH_PRESENTATION);
    assert_eq!(tool(json!({})).describe(PUBLISH_ARTIFACT, &json!({ "path": "out/r.csv", "name": "Report.csv" })).unwrap(), "Publish 'out/r.csv' in the workspace as the artifact 'Report.csv'");
    assert!(ArtifactTool::new(&json!({ "requires": true }), manager(&world), &PromptTexts::new(Arc::clone(world.runtime.prompts()), None)).is_err(), "nor parameters it does not take");
}

// --- The plugin -----------------------------------------------------------

/// A storage of content takes no version classified above its
/// `destination` (`require_storable`, issue 282): refused before anything
/// is kept.
#[tokio::test]
async fn a_storage_of_content_takes_nothing_above_its_destination() {
    let files = tempfile::tempdir().unwrap();
    let storage = [("kept".to_string(), BackendConfig::new("local", json!({ "root": files.path(), "destination": { "level": "public" } })))].into();
    let config = AppConfig { content: ContentConfig { storage }, ..Default::default() };
    let runtime = TestRuntime { config, plugins: vec![&TestAgents, &ArtifactsPlugin], ..Default::default() }.build();
    let manager = Arc::clone(runtime.services().get::<ArtifactManager>().unwrap());
    let owner = UserId::new();
    let alice = SecurityContext::new(ActorId::User(owner), SessionId::new(), ChannelId::new());
    let project = Project::new(ScopedName::new("report", Scope::UserScope { user_id: owner }).unwrap(), owner);
    runtime.entity_stores().projects.create(&SecurityContext::system(), &project).await.unwrap();
    let name = ScopedName::new("r.csv", project.holds()).unwrap();
    let internal = Classification::general(Confidentiality::Internal);
    let refused = manager.publish(&alice, (SessionId::new(), ChannelId::new()), (3, file("a,b")), &internal, &name, None).await;
    assert!(matches!(refused, Err(ArtifactError::StorageNotCleared(ref name)) if name == "kept"), "{refused:?}");
    assert!(fs::read_dir(files.path()).unwrap().next().is_none(), "nothing kept");
    let public = manager.publish(&alice, (SessionId::new(), ChannelId::new()), (3, file("a,b")), &Classification::PUBLIC, &name, None).await;
    assert!(public.is_ok(), "what it may hold it takes: {public:?}");
}

/// New artifacts are kept in the storage of content the section names, or
/// the one there is (issue 282); one it cannot have is said naming
/// `artifacts.keep_in`.
#[tokio::test]
async fn new_artifacts_go_to_the_storage_the_section_names_or_the_one_there_is() {
    let world = world().await;
    assert!(manager(&world).is_configured(), "kept, the one there is");

    fn built(names: &[&str], section: Value) -> Result<RuntimeManager, String> {
        let storage = names.iter().map(|name| (name.to_string(), BackendConfig::new("local", json!({ "root": name })))).collect();
        // Attachments kept in the first, so that only the artifacts' choice is asked.
        let attachments = AttachmentsConfig { keep_in: names.first().map(|name| name.to_string()), ..Default::default() };
        let config = AppConfig { content: ContentConfig { storage }, attachments, plugins: [("artifacts".to_string(), section)].into(), ..Default::default() };
        TestRuntime { config, plugins: vec![&TestAgents, &ArtifactsPlugin], ..Default::default() }.try_build().map_err(|e| format!("{e:#}"))
    }
    let configured = |runtime: RuntimeManager| runtime.services().get::<ArtifactManager>().is_some_and(|manager| manager.is_configured());
    assert!(!configured(built(&[], json!({})).unwrap()), "no storage unless configured");
    let message = built(&["a", "b"], json!({})).map(|_| ()).expect_err("which one");
    assert!(message.contains("artifacts.keep_in"), "which one, among several, must be said: {message}");
    let message = built(&["a", "b"], json!({ "keep_in": "c" })).map(|_| ()).expect_err("none such");
    assert!(message.contains("artifacts.keep_in") && message.contains("'c'"), "not one that is not there: {message}");
    assert!(configured(built(&["a", "b"], json!({ "keep_in": "b" })).unwrap()));
}

/// What the API answers for what went wrong with an artifact: its status
/// and its code.
#[tokio::test]
async fn an_artifacts_errors_answer_with_status_and_code() {
    async fn answer(error: ArtifactError) -> (u16, String) {
        let response = ApiError::from(error).into_response();
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 1 << 16).await.unwrap()).unwrap();
        (status, body["code"].as_str().unwrap_or_default().to_string())
    }
    let refused = Refused { subject: "artifact", action: "UpdateArtifact".to_string() };
    let artifact = ArtifactId::new();
    assert_eq!(answer(ArtifactError::NotFound(artifact)).await, (404, "artifact_not_found".to_string()));
    assert_eq!(answer(ArtifactError::VersionNotFound { artifact, version: 2 }).await, (404, "version_not_found".to_string()));
    assert_eq!(answer(ArtifactError::ClassifiedOtherwise("r.csv".to_string())).await, (409, "classified_otherwise".to_string()));
    assert_eq!(answer(ArtifactError::Store(StoreError::PermissionDenied(refused))).await, (403, "artifact_forbidden".to_string()));
    assert_eq!(answer(ArtifactError::Store(StoreError::NameTaken { entity: "artifact" })).await, (409, "name_taken".to_string()));
    let unavailable = (503, "storage_unavailable".to_string());
    assert_eq!(answer(ArtifactError::Content(ContentError::Unavailable("broken".to_string()))).await, unavailable, "as an attachment's storage, and a workspace's");
    assert_eq!(answer(ArtifactError::StorageGone(StorageGone("old".to_string()))).await, unavailable, "named by what is kept, not by the request");
    assert_eq!(answer(ArtifactError::Content(ContentError::TooLarge { limit: 1 })).await, (413, "file_too_large".to_string()));
    assert_eq!(answer(ArtifactError::Content(ContentError::Changed)).await, (400, "changed".to_string()));
    assert_eq!(answer(ArtifactError::NotJoined).await.0, 500, "nor a runtime without the plugin");
}

/// What publishing failed of, as the artifact tool tells its model (issue 466): what a storage of
/// content reported as everywhere else, a file that changed with what the model can do about it.
#[test]
fn an_artifacts_errors_are_told_the_model_as_a_storages_are() {
    assert!(matches!(ToolError::from(ArtifactError::Content(ContentError::TooLarge { limit: 1 })), ToolError::InvalidParameter(_)));
    assert!(matches!(ToolError::from(ArtifactError::Content(ContentError::Unavailable("broken".to_string()))), ToolError::ExecutionFailed(_)));
    let changed = ToolError::from(ArtifactError::Content(ContentError::Changed));
    assert!(matches!(&changed, ToolError::InvalidParameter(m) if m.contains("publish it again")), "{changed}");
}

// --- The routes -----------------------------------------------------------

/// The API of `world` with the plugin's routes, which lets anybody log in - as one of the staff.
fn api_of(world: &World) -> Router {
    let users = Arc::clone(&world.runtime.entity_stores().users);
    let provider = AcceptAnyUserProvider::with_roles(Arc::clone(&users), BTreeMap::new(), vec![*STAFF]);
    let auth = Arc::new(Auth {
        tokens: TokenService::new(b"0123456789abcdef0123456789abcdef", &AuthConfig::default()).unwrap(),
        users: Arc::new(UserManager::new(vec![Arc::new(provider)], users, Arc::<EventBus>::clone(world.runtime.event_bus()))),
        controllers: Arc::clone(world.runtime.controller_manager()),
    });
    let state = Arc::new(AppState { auth, runtime_manager: Arc::clone(&world.runtime), session_manager: Arc::clone(&world.manager), project_manager: Arc::clone(&world.projects), compartments: Default::default(), shutdown: Default::default() });
    let plugins: [Box<dyn RestPlugin>; 1] = [Box::new(ArtifactsPlugin)];
    api(state, &plugins)
}

/// The cookie `name` is logged in by.
async fn logged_in(api: &Router, name: &str) -> String {
    let login = Request::post("/auth/login").header("content-type", "application/json").body(Body::from(format!(r#"{{"username":"{name}","password":"x"}}"#))).unwrap();
    let response = api.clone().oneshot(login).await.unwrap();
    response.headers().get(SET_COOKIE).and_then(|v| v.to_str().ok()).and_then(|v| v.split(';').next()).unwrap().to_string()
}

/// What `uri` answers whoever is logged in by `cookie`: its status and its
/// body.
async fn get(api: &Router, cookie: &str, uri: &str) -> (StatusCode, Vec<u8>) {
    let response = api.clone().oneshot(Request::get(uri).header(COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    (status, to_bytes(response.into_body(), 1 << 20).await.unwrap().to_vec())
}

/// The plugin's routes are served behind the core's login, under the paths
/// and in the form the web UI knows: the artifacts of a user, one with
/// where its versions were published, a version's content, and the
/// artifacts of a channel.
#[tokio::test]
async fn the_routes_serve_artifacts_as_the_web_ui_knows_them() {
    let world = world().await;
    let api = api_of(&world);
    let anonymous = api.clone().oneshot(Request::get("/artifacts").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();
    let (artifact, _) = publish(&world, &alice, session_id, main, &at, "report.csv", Some("The figures")).await.unwrap();

    let cookie = logged_in(&api, "alice").await;
    let (status, body) = get(&api, &cookie, "/artifacts").await;
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, listed[0]["id"].clone(), listed[0]["can_change"].clone()), (StatusCode::OK, json!(artifact.id), json!(true)), "{listed}");
    assert!(listed[0]["versions"][0].get("channel_name").is_none(), "not where it was published, where artifacts are listed");
    let (_, body) = get(&api, &cookie, &format!("/artifacts/{}", artifact.id)).await;
    let one: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((one["versions"][0]["channel_name"].clone(), one["versions"][0]["description"].clone()), (json!("main"), json!("The figures")), "{one}");
    let (status, content) = get(&api, &cookie, &format!("/artifacts/{}/versions/1/content", artifact.id)).await;
    assert_eq!((status, content), (StatusCode::OK, b"a,b".to_vec()));

    let his = logged_in(&api, "bob").await;
    let (status, body) = get(&api, &his, &format!("/sessions/{session_id}/channels/{main}/artifacts")).await;
    let in_channel: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, in_channel[0]["id"].clone(), in_channel[0]["can_change"].clone()), (StatusCode::OK, json!(artifact.id), json!(false)), "{in_channel}");
    let (status, body) = get(&api, &cookie, &format!("/projects/{}/artifacts", session.session().project)).await;
    let in_project: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, in_project[0]["id"].clone(), in_project[0]["project_id"].clone()), (StatusCode::OK, json!(artifact.id), json!(session.session().project)), "{in_project}");
    let (status, body) = get(&api, &his, &format!("/artifacts/{}", ArtifactId::new())).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, problem["code"].clone()), (StatusCode::NOT_FOUND, json!("artifact_not_found")));
    world.manager.stop(&alice, session_id).await.unwrap();
}

// --- Who may do what (issues 271, 23, 316, the security suite) -------------

/// Publishes the file `at` of the workspace of channel `channel` as a
/// version of the artifact `name`, in `ctx`'s name, as `artifact_tool` does
/// (issue 296): the workspace reached there for reading, the file read, and
/// what was read handed to the `ArtifactManager`, named in the scope of the
/// session's project (issue 467).
async fn publish(world: &World, ctx: &SecurityContext, session: SessionId, channel: ChannelId, at: &WorkspacePath, name: &str, description: Option<&str>) -> Result<(Artifact, u32), ArtifactError> {
    let running = world.manager.open(&World::system(), session).await.expect("the session");
    let name = ScopedName::new(name, Scope::ProjectScope { project_id: running.session().project })?;
    let (workspace, terminal) = running.channel(channel).expect("the channel").workspace_terminal(ctx, Grant::Read).await.expect("the workspace, to read");
    let content = terminal.read(at).await.expect("the file");
    manager(world).publish(ctx, (session, channel), content, &workspace.classification, &name, description).await
}

/// G4 (issue 308): an artifact shared with a team is among the artifacts
/// of its participants while they are participants - with its versions where it was
/// shared with them, without where it was not, since reading a version
/// needs its own list besides the artifact's (issue 316). What was
/// published in a channel is shared with the channel's participants and
/// stays found through it, not among theirs.
#[tokio::test]
async fn an_artifact_shared_with_a_team_is_among_its_members_artifacts() {
    let world = world().await;
    let (alice, bob, carol) = (World::as_user(world.alice), World::as_user(world.bob), World::as_user(world.carol));
    let groups = world.runtime.group_manager();
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.expect("hers to use");
    let at = WorkspacePath::parse("report.md").unwrap();
    terminal.write(&at, file("final"), 100).await.unwrap();
    let (whole, _) = publish(&world, &alice, session_id, main, &at, "report.md", None).await.expect("one to share with its versions");
    let (alone, _) = publish(&world, &alice, session_id, main, &at, "notes.md", None).await.expect("one to share without them");
    let listed = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move {
            let mut ids = views.artifacts().await.unwrap().iter().map(|a| a.artifact().id).collect::<Vec<_>>();
            ids.sort();
            ids
        }
    };
    assert!(listed(&bob).await.is_empty(), "the participants of the channel are not named");

    let project = groups.create(&alice, "project", Scope::UserScope { user_id: world.alice }).await.unwrap();
    groups.set_members(&alice, project.id, vec![world.carol]).await.unwrap();
    let the_team = AclEntry { subject: Subject::Group(project.id), grant: Grant::Read };
    let with_the_team = |artifact: &Artifact| [artifact.security_attributes.acl.clone(), vec![the_team]].concat();
    manager(&world).set_access(&alice, whole.id, with_the_team(&whole), true).await.expect("shared with the team, with its versions");
    manager(&world).set_access(&alice, alone.id, with_the_team(&alone), false).await.expect("shared with the team alone");

    let mut both = vec![whole.id, alone.id];
    both.sort();
    assert_eq!(listed(&carol).await, both, "a member finds both among hers");
    assert_eq!(read_version(&world, &carol, whole.id, 1).await, Read::Content(5, "final".to_string()), "and reads the version of the one shared with its versions");
    assert_eq!(read_version(&world, &carol, alone.id, 1).await, Read::NoVersion, "not that of the other, whose own list does not name the team");
    assert!(listed(&bob).await.is_empty(), "who is not in the team does not");
    groups.set_members(&alice, project.id, Vec::new()).await.unwrap();
    assert!(listed(&carol).await.is_empty(), "who left it no longer does");
    world.manager.stop(&alice, session_id).await.unwrap();
}

/// The content of an artifact's version, as text.
async fn kept(content: ByteStream) -> String {
    let chunks: Vec<_> = content.collect().await;
    String::from_utf8(chunks.into_iter().flat_map(|chunk| chunk.unwrap().to_vec()).collect()).unwrap()
}

/// How reading version `number` of artifact `id` in `ctx`'s name went, as a
/// frontend reads it - through the views (issue 292): its size and text, the
/// artifact not being there for them, or the version not.
#[derive(Debug, PartialEq)]
enum Read {
    Content(u64, String),
    NoArtifact,
    NoVersion,
}

async fn read_version(world: &World, ctx: &SecurityContext, id: ArtifactId, number: u32) -> Read {
    let artifact = match world.manager.views(Viewer::new(ctx.clone())).artifact(id).await {
        Ok(artifact) => artifact,
        Err(ArtifactError::NotFound(_)) => return Read::NoArtifact,
        Err(e) => panic!("{e}"),
    };
    match artifact.content(number).await {
        Ok((size, content)) => Read::Content(size, kept(content).await),
        Err(ArtifactError::VersionNotFound { .. }) => Read::NoVersion,
        Err(e) => panic!("{e}"),
    }
}

/// An artifact (issue 271) is published from the workspace of a channel,
/// in the name of whoever publishes - its owner, who makes it
/// (`CreateArtifact`), or whom its list lets add a version
/// (`WriteArtifact`, issue 23) -, into the scope of its session's project
/// (issue 467). Its owner reads every version; a version is read by the
/// participants of the channel it was published in, by the artifact,
/// wherever they act; only its owner shares it; and what lies in a
/// session's own project goes with the session, its content too.
#[tokio::test]
async fn an_artifact_is_published_in_a_channel_for_its_publisher_and_read_by_who_reads_the_channel() {
    let world = world().await;
    let artifacts = manager(&world);
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let in_main = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.session(session_id).await.unwrap().channel(main).await.unwrap() }
    };
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.expect("hers to use");
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();

    let (published, asked) = world.asked(publish(&world, &alice, session_id, main, &at, "report.csv", Some("The figures"))).await;
    let (artifact, first) = published.expect("published");
    let own = session.session().project;
    assert_eq!((first, &artifact.name.scope, artifact.owner()), (1, &Scope::ProjectScope { project_id: own }, Some(world.alice)), "in its session's own project");
    assert!(asked.contains(&"alice: GetProject(alice's) yes".to_string()) && asked.contains(&"alice: CreateArtifact(alice's) yes".to_string()), "the project reached, the artifact made, in her name: {asked:?}");
    let members = AclEntry { subject: Subject::Group(GroupId::of_project(own)), grant: Grant::Read };
    assert!(artifact.security_attributes.acl.contains(&members) && !artifact.versions[0].acl.contains(&members), "the project's members read the artifact, not the version");
    terminal.write(&at, file("a,b\n1,2"), 100).await.unwrap();
    let (published, asked) = world.asked(publish(&world, &alice, session_id, main, &at, "report.csv", None)).await;
    assert_eq!(published.expect("a second version").1, 2);
    assert!(asked.contains(&"alice: WriteArtifact(alice's) yes".to_string()), "added to as hers: {asked:?}");

    assert_eq!(read_version(&world, &alice, artifact.id, 1).await, Read::Content(3, "a,b".to_string()), "its owner reads every version, as it was then");
    let elsewhere = SecurityContext::new(ActorId::User(world.bob), SessionId::new(), ChannelId::new()).with_roles(vec![*STAFF], None);
    assert_eq!(read_version(&world, &elsewhere, artifact.id, 2).await, Read::Content(7, "a,b\n1,2".to_string()), "a member of the channel reads what was published there, wherever he acts");
    assert_eq!(read_version(&world, &bob, artifact.id, 3).await, Read::NoVersion);
    assert_eq!(read_version(&world, &World::as_user(world.carol), artifact.id, 1).await, Read::NoArtifact, "nobody else, who learns nothing");
    let (found, asked) = world.asked(store(&world).get(&bob, artifact.id)).await;
    assert!(found.is_ok(), "the store finds it in his name, by its list, which publishing gave the channel's members (issue 316)");
    assert_eq!(asked.first().map(String::as_str), Some("bob: GetArtifact(alice's) yes"), "asked in his name: {asked:?}");
    assert!(store(&world).get(&World::as_user(world.carol), artifact.id).await.is_err(), "not in carol's");

    assert!(in_main(&bob).await.workspace().await.unwrap().is_some(), "bob uses it too");
    let refused = publish(&world, &bob, session_id, main, &at, "report.csv", None).await;
    assert!(matches!(refused, Err(ArtifactError::Store(StoreError::PermissionDenied(_)))), "nor adds to alice's artifact of that name: {refused:?}");
    let listed = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.artifacts().await.unwrap().iter().map(|a| a.artifact().id).collect::<Vec<_>>() }
    };
    let in_channel = in_main(&bob).await.artifacts().await.unwrap().iter().map(|a| a.artifact().id).collect::<Vec<_>>();
    assert_eq!(in_channel, [artifact.id], "a member finds what was published in the channel there");
    assert!(listed(&bob).await.is_empty(), "not among his own (issue 293): it was shared with the channel, not with him");
    assert_eq!(listed(&alice).await, [artifact.id], "among its owner's");
    let writers = SecurityAttributes { owner_id: ActorId::User(world.alice), acl: artifact.security_attributes.acl.clone() }.granting(Subject::Actor(ActorId::User(world.bob)), Grant::Write).acl;
    let refused = artifacts.set_access(&bob, artifact.id, writers.clone(), true).await;
    assert!(matches!(refused, Err(ArtifactError::Store(StoreError::PermissionDenied(_)))), "nor shares it - refused, since he reads it (issue 316): {refused:?}");
    artifacts.set_access(&alice, artifact.id, writers, true).await.expect("its owner shares it");
    assert_eq!(listed(&bob).await, [artifact.id], "shared with him by name, it is among his");
    let (added, third) = publish(&world, &bob, session_id, main, &at, "report.csv", None).await.expect("whom it is shared with to write adds a version");
    assert_eq!((third, added.owner()), (3, Some(world.alice)), "hers still");
    let (draft, _) = publish(&world, &alice, session_id, main, &at, "draft.csv", None).await.unwrap();

    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    world.manager.purge(&World::system(), session_id).await.unwrap();
    for gone in [artifact.id, draft.id] {
        assert!(store(&world).get_deleted(&World::system(), gone).await.is_err(), "what lay in its own project went with the session");
        assert!(!world.files.path().join("content").join("artifacts").join(gone.to_string()).exists(), "its content too");
    }
}

/// A session of alice's with her workspace attached for writing and a file
/// in it, and an artifact published from it in its channel "main" by each
/// of alice and bob - each his publisher's (issue 271), in the session's
/// scope.
async fn session_with_artifacts(world: &World) -> (Arc<SessionLoop>, Artifact, Artifact) {
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();
    let (hers, _) = publish(world, &alice, session_id, main, &at, "hers.csv", None).await.unwrap();
    let (his, _) = publish(world, &bob, session_id, main, &at, "his.csv", None).await.unwrap();
    assert_eq!((hers.owner(), his.owner()), (Some(world.alice), Some(world.bob)));
    (session, hers, his)
}

/// Issue 320: what lies in a session's scope goes with it in the name of
/// whoever deletes it, as far as they may. An artifact another participant
/// published in the session is his: deleting the session deletes hers - in
/// its own project (issue 467) -, in her name, and keeps the session,
/// telling her what is left - until he deletes his.
#[tokio::test]
async fn an_artifact_goes_with_its_session_only_as_far_as_whoever_deletes_it_may() {
    let world = world().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let (session, hers, his) = session_with_artifacts(&world).await;
    let session_id = session.id();

    let (refused, asked) = world.asked(world.manager.delete(&alice, session_id)).await;
    let refused = refused.expect_err("bob's artifact is left");
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ScopeNotEmpty(ScopeNotEmpty { left, .. })) if *left == [(Artifact::KIND, 1)]), "{refused}");
    assert!(refused.to_string().contains("1 artifact") && !refused.to_string().contains("his.csv"), "what is left, by kind, not by name: {refused}");
    let (session_checks, artifact_checks) = asked.split_at(4.min(asked.len()));
    assert_eq!(session_checks, checks(&[
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
        "alice: GetSession(alice's) yes",
        "alice: DeleteSession(alice's) yes",
    ]), "asked in her name");
    // The artifacts in the order the store lists them, which is by id: each
    // read, her clearance for it (issue 322), and deleted - bob's read by
    // the list of its version published in the channel, which she reads
    // (issue 316), but not hers to delete.
    let his_checks = checks(&["alice: GetArtifact(bob's) yes", "alice: JoinChannel(alice) yes", "alice: DeleteArtifact(bob's) no"]);
    let her_checks = checks(&["alice: GetArtifact(alice's) yes", "alice: JoinChannel(alice) yes", "alice: DeleteArtifact(alice's) yes"]);
    assert!(artifact_checks == [his_checks.clone(), her_checks.clone()].concat() || artifact_checks == [her_checks, his_checks].concat(),
        "asked in her name, who reads bob's but may not delete it: {artifact_checks:?}");
    assert!(world.runtime.entity_stores().sessions.get(&alice, session_id).await.is_ok(), "the session is kept");
    assert!(world.manager.views(Viewer::new(alice.clone())).session(session_id).await.is_ok(), "and runs on");
    assert!(store(&world).get(&bob, his.id).await.is_ok(), "bob's artifact stays");
    let removal = store(&world).get_deleted(&World::system(), hers.id).await.expect("hers went").removal;
    assert_eq!(removal.by, ActorId::User(world.alice), "in her name");

    assert!(manager(&world).delete(&bob, his.id).await.expect("bob deletes his"));
    assert!(world.manager.delete(&alice, session_id).await.expect("nothing of anybody else's is left"));
}

/// Issue 320: what went with a session comes back with it in the name of
/// whoever restores it, as far as they may; the session comes back all the
/// same, and what they may not restore stays deleted for its owner to
/// restore.
#[tokio::test]
async fn an_artifact_comes_back_with_its_session_as_far_as_whoever_restores_it_may() {
    let world = world().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let (session, hers, his) = session_with_artifacts(&world).await;
    let session_id = session.id();
    world.manager.delete(&World::system(), session_id).await.expect("the system's to delete, all of it");

    let (restored, asked) = world.asked(world.manager.restore(&alice, session_id)).await;
    restored.expect("restored all the same");
    assert!(asked.contains(&"alice: ReadDeletedArtifact(bob's) no".to_string()), "asked in her name: {asked:?}");
    assert!(store(&world).get(&alice, hers.id).await.is_ok(), "hers comes back with it");
    assert!(store(&world).get_deleted(&World::system(), his.id).await.is_ok(), "bob's stays deleted: not hers to restore");
    manager(&world).restore(&bob, his.id).await.expect("bob restores his");
}

/// Issue 319: what is said of an artifact as a whole reaches whoever reads
/// it, however it went or came back - here with its session, to a participant of
/// the channel a version was published in, whom publishing let read it
/// (issue 316). And the groups of the participants, deleted
/// and restored with the session, are announced as a team is.
#[tokio::test]
async fn what_goes_and_comes_back_with_a_session_is_announced_to_whoever_reads_it() {
    let world = world().await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();
    let (artifact, _) = publish(&world, &alice, session_id, main, &at, "report.csv", None).await.unwrap();
    let id = artifact.id;
    let (of_session, of_channel) = (GroupId::of_session(session_id), GroupId::of_channel(main));
    let of_project = GroupId::of_project(session.session().project);
    let read_by = |group| AclEntry { subject: Subject::Group(group), grant: Grant::Read };
    assert_eq!(artifact.security_attributes.acl, [read_by(of_channel), read_by(of_project)], "bob reads it as a member of the channel");

    let mut bobs = world.runtime.event_bus().subscribe_as(World::as_user(world.bob));
    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    hears(&mut bobs, [GlobalEvent::deleted::<Artifact>(&id), GlobalEvent::deleted::<Group>(&of_session), GlobalEvent::deleted::<Group>(&of_channel)]).await;

    world.manager.restore(&alice, session_id).await.unwrap();
    hears(&mut bobs, [GlobalEvent::restored::<Artifact>(&id), GlobalEvent::restored::<Group>(&of_session), GlobalEvent::restored::<Group>(&of_channel)]).await;
}

/// Issues 23, 316: a version is read by the participants of the channel it was
/// published in, of those who read the artifact - publishing lets them read
/// it -, so drafts published where alice works alone reach nobody who
/// reads the version she published to the team, nor does what she said of
/// them.
#[tokio::test]
async fn drafts_of_an_artifact_stay_with_the_channel_they_were_published_in() {
    let world = world().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let drafts = session.create_channel(&alice, ChannelSpec { name: "drafts".to_string(), components: quiet(), participants: vec![], classification: None, language: None, uses_workspace: None, internal_actors: Vec::new() }).await.expect("hers alone").id();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let write_in = |channel, text| {
        let (alice, session) = (&alice, &session);
        async move {
            let (_, terminal) = session.channel(channel).expect("the channel").workspace_terminal(alice, Grant::Write).await.expect("hers to use");
            terminal.write(&WorkspacePath::parse("report.md").unwrap(), file(text), 100).await.unwrap();
        }
    };
    let at = WorkspacePath::parse("report.md").unwrap();
    write_in(drafts, "rough").await;
    let (artifact, _) = publish(&world, &alice, session_id, drafts, &at, "report.md", Some("Still wrong about Q3")).await.expect("a draft");
    write_in(main, "final").await;
    let (_, final_one) = publish(&world, &alice, session_id, main, &at, "report.md", Some("The report")).await.expect("the final one");
    assert_eq!(final_one, 2);

    let his = world.manager.views(Viewer::new(bob.clone())).artifact(artifact.id).await.expect("bob reads what reached the team").snapshot_with_publications().await;
    assert_eq!(his.artifact.versions.iter().map(|v| v.number).collect::<Vec<_>>(), [2], "the final one alone");
    assert_eq!(his.artifact.versions[0].description.as_ref().map(|d| d.to_string()), Some("The report".to_string()));
    assert!(his.artifact.description.is_none() && his.access.is_empty() && his.version_access.is_empty(), "nothing said of the draft, nor whom it reaches");
    let where_published = |publication: &PublicationSnapshot| (publication.channel_name.clone(), publication.published_by.clone());
    assert_eq!(his.publications.values().map(where_published).collect::<Vec<_>>(), [("main".to_string(), Some("alice".to_string()))], "where it was published and by whom, as a member of the channel");
    assert_eq!(read_version(&world, &bob, artifact.id, 1).await, Read::NoVersion, "not the draft");
    assert_eq!(read_version(&world, &bob, artifact.id, 2).await, Read::Content(5, "final".to_string()), "the final one");

    let hers = world.manager.views(Viewer::new(alice.clone())).artifact(artifact.id).await.unwrap().snapshot_with_publications().await;
    assert_eq!(hers.artifact.versions.len(), 2, "its owner reads both");
    let names: Vec<Option<String>> = hers.version_access.values().map(|access| access[0].name.clone()).collect();
    assert_eq!(names, [Some("drafts".to_string()), Some("main".to_string())], "and sees where each reaches");
    assert_eq!(hers.publications.values().map(|p| p.channel_name.clone()).collect::<Vec<_>>(), ["drafts", "main"], "and where each was published");

    // Shared by name with carol, who takes part in neither channel: she
    // reads both, and learns nothing of where they were published.
    let carol = World::as_user(world.carol);
    let shared = [hers.artifact.security_attributes.acl.clone(), vec![AclEntry { subject: Subject::Actor(ActorId::User(world.carol)), grant: Grant::Read }]].concat();
    manager(&world).set_access(&alice, artifact.id, shared, true).await.unwrap();
    let theirs = world.manager.views(Viewer::new(carol.clone())).artifact(artifact.id).await.expect("shared with carol").snapshot_with_publications().await;
    assert_eq!(theirs.artifact.versions.len(), 2, "carol reads both");
    assert!(theirs.publications.is_empty(), "but not where they were published, nor by whom");
    world.manager.stop(&alice, session_id).await.unwrap();
}

/// An artifact of a lifted project (issues 347, 467) outlives the session
/// it was published in, and names the channel, in its list and in its
/// version's, until the session is purged: then the entries go as the
/// artifact is next stored, since they name nobody; the project's members
/// read it still.
#[tokio::test]
async fn an_artifact_of_a_lifted_project_drops_its_channel_once_the_session_is_purged() {
    let world = world().await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let session_id = session.id();
    let main = session.channels()[0].id();
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.expect("hers to use");
    let at = WorkspacePath::parse("report.md").unwrap();
    terminal.write(&at, file("final"), 100).await.unwrap();
    let (artifact, _) = publish(&world, &alice, session_id, main, &at, "report.md", None).await.unwrap();
    let project = session.session().project;
    world.runtime.controller_manager().project_controller(project).await.unwrap().lift(&alice, "reports", Scope::UserScope { user_id: world.alice }).await.expect("kept as hers");
    let read_by = |group| AclEntry { subject: Subject::Group(group), grant: Grant::Read };
    let (channel, members) = (read_by(GroupId::of_channel(main)), read_by(GroupId::of_project(project)));
    assert_eq!((artifact.security_attributes.acl.as_slice(), artifact.versions[0].acl.as_slice()), ([channel, members].as_slice(), [channel].as_slice()), "read by the channel's members, and the project's");

    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    world.manager.purge(&World::system(), session_id).await.unwrap();
    let described = manager(&world).describe(&alice, artifact.id, Some("The report")).await.expect("still hers, in the lifted project");
    assert_eq!((described.security_attributes.acl.as_slice(), described.versions[0].acl.as_slice()), ([members].as_slice(), [].as_slice()), "the channel's entries go as it is stored: {described:?}");
}

/// Issue 467: what is published in a session that lies in a shared project lies in the project
/// and outlives the session. A member of the project who took part in none of its channels finds
/// the artifact through the project, and reads no version: those are the channels'; a member who
/// took part in the channel reads the version published there.
#[tokio::test]
async fn an_artifact_of_a_shared_project_outlives_the_session_it_was_published_in() {
    let world = world().await;
    let (alice, carol) = (World::as_user(world.alice), World::as_user(world.carol));
    let project = world.projects.create(&alice, "report", Scope::UserScope { user_id: world.alice }, None).await.unwrap();
    world.runtime.controller_manager().project_controller(project.id).await.unwrap().update_members(&alice, vec![world.bob, world.carol]).await.unwrap();
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.update_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    session.update_project(&alice, Some(project.id)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = session.channel(main).expect("the channel").workspace_terminal(&alice, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("report.md").unwrap();
    terminal.write(&at, file("final"), 100).await.unwrap();
    let (artifact, _) = publish(&world, &alice, session_id, main, &at, "report.md", None).await.unwrap();
    assert_eq!(artifact.project(), Some(project.id), "in the shared project");

    let in_project = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.project(project.id).await.unwrap().artifacts().await.unwrap() }
    };
    let hers = in_project(&carol).await;
    assert_eq!(hers.iter().map(|a| a.artifact().id).collect::<Vec<_>>(), [artifact.id], "a member finds it through the project");
    assert!(hers[0].artifact().versions.is_empty(), "but reads no version of a channel she was not in");
    assert_eq!(read_version(&world, &carol, artifact.id, 1).await, Read::NoVersion);
    let his = in_project(&World::as_user(world.bob)).await;
    assert_eq!(his[0].artifact().versions.iter().map(|v| v.number).collect::<Vec<_>>(), [1], "a member who took part reads the version of his channel");

    world.manager.stop(&alice, session_id).await.unwrap();
    world.manager.delete(&alice, session_id).await.unwrap();
    world.manager.purge(&World::system(), session_id).await.unwrap();
    assert!(store(&world).get(&alice, artifact.id).await.is_ok(), "it outlives the session");
    assert_eq!(read_version(&world, &alice, artifact.id, 1).await, Read::Content(5, "final".to_string()), "with its content");
    assert_eq!(in_project(&alice).await.len(), 1, "found through the project still");
}

/// Issue 467: an artifact is published into a project its publisher reads - asked in their name
/// -, and under a name in a project's scope alone, which the caller vouches for.
#[tokio::test]
async fn publishing_into_a_project_needs_reading_it() {
    let world = world().await;
    let alice = World::as_user(world.alice);
    let carols = world.projects.create(&World::as_user(world.carol), "hers", Scope::UserScope { user_id: world.carol }, None).await.unwrap();
    let internal = Classification::general(Confidentiality::Internal);
    let channel = (SessionId::new(), ChannelId::new());
    let into = |scope: Scope| ScopedName::new("r.csv", scope).unwrap();
    let (refused, asked) = world.asked(manager(&world).publish(&alice, channel, (3, file("a,b")), &internal, &into(carols.holds()), None)).await;
    assert!(matches!(refused, Err(ArtifactError::Store(StoreError::NotFound { .. }))), "not into a project she does not read: {refused:?}");
    assert_eq!(asked, checks(&["alice: GetProject(carol's) no"]), "asked in her name, before anything else");
    let refused = manager(&world).publish(&alice, channel, (3, file("a,b")), &internal, &into(Scope::SessionScope { session_id: channel.0 }), None).await;
    assert!(matches!(refused, Err(ArtifactError::InvalidScope)), "nor anywhere but in a project: {refused:?}");
}

/// An agent's publication of an artifact (issue 271) asks what a call on
/// the workspace asks, and makes the artifact its owner's - asked in the
/// same context, `CreateArtifact` for alice.
#[tokio::test]
async fn an_agents_artifact_is_published_for_its_owner() {
    let call = calling(PUBLISH_ARTIFACT, json!({ "path": "notes/a.md" }));
    let world = World::with(&[&TestAgents, &ArtifactsPlugin], |registry| register_scripted(registry, ONCE_MODEL_KIND, vec![call])).await;
    let kept = |files: &Path, _: &Path| fs::read_dir(files.join("content").join("artifacts")).is_ok_and(|mut entries| entries.next().is_some());
    let asked = asked_for_a_call(&world, ARTIFACT_TOOL_KIND, &["notes/a.md"], kept, world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: CreateArtifact(alice's) yes".to_string()), "its owner's: {asked:?}");
}
