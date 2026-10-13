//! Artifacts through the whole runtime (issues 271, 23, 316, 328): the
//! plugin joined to the world of the security suite - the ownership
//! policy, recording what it is asked, and alice, bob and carol
//! (`caelumora_runtime::testing`), with a storage of artifacts beside the
//! workspaces' files. The tests of an action that acts in someone's name
//! assert what it asks, as the suite does for the core's.

use axum::body::{to_bytes, Body};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use caelumora_artifacts::artifact::{Artifact, ArtifactId, PublicationSnapshot};
use caelumora_artifacts::component::{ArtifactTool, ARTIFACT_PUBLISH_PRESENTATION, ARTIFACT_TOOL_KIND, PUBLISH_ARTIFACT};
use caelumora_artifacts::manager::{ArtifactError, ArtifactManager};
use caelumora_artifacts::plugin::ArtifactsPlugin;
use caelumora_artifacts::view::{ArtifactChannelViews, ArtifactViews};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::confidentiality::{Classification, Confidentiality};
use caelumora_model::digest;
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId};
use caelumora_model::removal::Stored;
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, Grant, Refused, SecurityAttributes, SecurityContext, Subject};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::config::{AppConfig, AuthConfig};
use caelumora_model::store::{EntityStore, EntityStoreFactory, StoreError};
use caelumora_model::user_provider::accept_any::AcceptAnyUserProvider;
use caelumora_model::user_provider::UserManager;
use caelumora_model::workspace::Workspace;
use caelumora_rest_server::auth::{Auth, TokenService};
use caelumora_rest_server::error::ApiError;
use caelumora_rest_server::plugin::RestPlugin;
use caelumora_rest_server::routes::api;
use caelumora_rest_server::state::AppState;
use caelumora_runtime::event::{EventBus, EventSubscription};
use caelumora_runtime::runtime::RuntimeBuilder;
use caelumora_runtime::session::{ChannelSpec, SessionError, SessionLoop};
use caelumora_runtime::terminal::{ByteStream, WorkspacePath};
use caelumora_runtime::testing::{asked_as_a_call_on_the_workspace, calling, quiet, register_once, World, ONCE_MODEL_KIND, STAFF};
use caelumora_runtime::tool::{ToolError, ToolProvider};
use caelumora_runtime::view::Viewer;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// The section `artifacts` of the world: a storage "kept" in `artifacts`
/// beside where the workspaces' files lie.
fn kept_beside(files: &Path) -> BTreeMap<String, Value> {
    [("artifacts".to_string(), json!({ "storage": { "kept": { "type": "local", "root": files.join("artifacts") } } }))].into()
}

/// The world with artifacts.
async fn world() -> World {
    World::configured(&[&ArtifactsPlugin], kept_beside, |_| {}).await
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
    Box::pin(futures::stream::iter([Ok(bytes::Bytes::from_static(text.as_bytes()))]))
}

/// Waits until `events` has received each of `wanted`, in any order -
/// compared as they are shown, since an event has no equality.
async fn hears<const N: usize>(events: &mut EventSubscription, wanted: [GlobalEvent; N]) {
    let mut missing: Vec<String> = wanted.iter().map(|event| format!("{event:?}")).collect();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
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
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let tool = ArtifactTool::new(&params, Arc::clone(world.runtime.workspace_manager()), manager(world)).expect("parameters it takes");
    (session, helper, docs, tool)
}

#[tokio::test]
async fn a_file_is_published_as_versions_of_an_artifact_kept_apart_from_the_workspace() {
    let world = world().await;
    let (session, helper, docs, tool) = with_the_tool(&world, json!({})).await;
    let mut state = AgentChannelState::default();
    let mut turn = World::turn(&session, helper, world.alice, &mut state);
    let dir = world.files.path().join(docs.id.to_string());
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("out/report.csv"), "a,b").unwrap();

    let first = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "description": "The figures" }), None, &mut turn).await.unwrap();
    assert_eq!(first.classification.as_ref(), Some(&docs.classification), "classified as the workspace");
    let first = first.value;
    assert_eq!((first["name"].as_str(), first["version"].as_u64(), first["size"].as_u64(), first["description"].as_str()), (Some("report.csv"), Some(1), Some(3), Some("The figures")));
    assert_eq!(first["digest"], digest::content(b"a,b").to_string());

    std::fs::write(dir.join("out/report.csv"), "a,b\n1,2").unwrap();
    let second = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv", "name": "report.csv" }), None, &mut turn).await.unwrap().value;
    assert_eq!((second["artifact"].clone(), second["version"].as_u64()), (first["artifact"].clone(), Some(2)), "a version of the same artifact");
    let kept = world.files.path().join("artifacts").join(first["artifact"].as_str().unwrap());
    assert_eq!(std::fs::read_to_string(kept.join("1")).unwrap(), "a,b", "the first as it was");
    assert_eq!(std::fs::read_to_string(kept.join("2")).unwrap(), "a,b\n1,2");
    std::fs::remove_file(dir.join("out/report.csv")).unwrap();
    assert!(kept.join("2").is_file(), "kept beyond the file");

    let missing = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/report.csv" }), None, &mut turn).await;
    assert!(matches!(missing, Err(ToolError::InvalidParameter(_))), "{missing:?}");
    let directory = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out" }), None, &mut turn).await;
    assert!(matches!(directory, Err(ToolError::InvalidParameter(_))), "{directory:?}");
    std::fs::write(dir.join("out/x.csv"), "x").unwrap();
    let long = tool.execute(PUBLISH_ARTIFACT, &json!({ "path": "out/x.csv", "name": "x".repeat(101) }), None, &mut turn).await;
    assert!(matches!(long, Err(ToolError::InvalidParameter(_))), "a name of at most 100 characters: {long:?}");
    drop(turn);
    world.manager.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}

#[tokio::test]
async fn a_publication_is_approved_as_a_change_and_presented_as_one() {
    let world = world().await;
    let tool = |params: Value| ArtifactTool::new(&params, Arc::clone(world.runtime.workspace_manager()), manager(&world)).unwrap();
    let asks = |mode: Value| tool(json!({ "requires_approval": mode })).tools()[0].requires_approval;
    assert!(asks(json!(true)) && asks(json!("unless_read_only")), "it changes what is kept");
    assert!(!asks(json!(false)));
    assert!(tool(json!({})).tools()[0].requires_approval, "unless configured");
    assert_eq!(tool(json!({})).tools()[0].presentation, ARTIFACT_PUBLISH_PRESENTATION);
    assert_eq!(tool(json!({})).describe(PUBLISH_ARTIFACT, &json!({ "path": "out/r.csv", "name": "Report.csv" })).unwrap(), "Publish 'out/r.csv' in the workspace as the artifact 'Report.csv'");
    assert!(ArtifactTool::new(&json!({ "requires": true }), Arc::clone(world.runtime.workspace_manager()), manager(&world)).is_err(), "nor parameters it does not take");
}

// --- The plugin -----------------------------------------------------------

/// The storages are the section's, made by the backends registered for
/// them - `local` the plugin's own (issue 328) -, and what does not fit is
/// said naming the storage.
#[tokio::test]
async fn the_storages_of_the_section_are_made_when_the_plugin_registers() {
    let world = world().await;
    assert!(manager(&world).is_configured(), "kept, the one there is");
    let without = World::with(&[&ArtifactsPlugin], |_| {}).await;
    assert!(!manager(&without).is_configured(), "no storage unless configured");

    let refused = |section: Value| {
        let config = AppConfig { plugins: [("artifacts".to_string(), section)].into(), ..Default::default() };
        let built = RuntimeBuilder::from_config(&config, &[&ArtifactsPlugin]).map(|_| ());
        format!("{:#}", built.expect_err("refused"))
    };
    let message = refused(json!({ "storage": { "kept": { "type": "local" } } }));
    assert!(message.contains("the storage of artifacts 'kept' (local)") && message.contains("root"), "{message}");
    let message = refused(json!({ "storage": { "kept": { "type": "s3" } } }));
    assert!(message.contains("known are: local"), "{message}");
    let message = refused(json!({ "storage": { "a": { "type": "local", "root": "a" }, "b": { "type": "local", "root": "b" } } }));
    assert!(message.contains("publish_to"), "which one, among several, must be said: {message}");
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
    assert_eq!(answer(ArtifactError::Unavailable("broken".to_string())).await.0, 500, "the storage's failure is not the client's");
    assert_eq!(answer(ArtifactError::NotJoined).await.0, 500, "nor a runtime without the plugin");
}

// --- The routes -----------------------------------------------------------

/// The API of `world` with the plugin's routes, which lets anybody log in
/// - as one of the staff.
fn api_of(world: &World) -> Router {
    let users = Arc::clone(&world.runtime.entity_stores().users);
    let provider = AcceptAnyUserProvider::with_roles(Arc::clone(&users), BTreeMap::new(), vec![*STAFF]);
    let auth = Arc::new(Auth {
        tokens: TokenService::new(b"0123456789abcdef0123456789abcdef", &AuthConfig::default()).unwrap(),
        users: Arc::new(UserManager::new(vec![Arc::new(provider)], users, Arc::<EventBus>::clone(world.runtime.event_bus()))),
        controllers: Arc::clone(world.runtime.controller_manager()),
    });
    let state = Arc::new(AppState { auth, runtime_manager: Arc::clone(&world.runtime), session_manager: Arc::clone(&world.manager), compartments: Default::default() });
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
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = world.runtime.workspace_manager().terminal_in(&alice, session_id, main, Grant::Write).await.unwrap();
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
    let (status, body) = get(&api, &his, &format!("/artifacts/{}", ArtifactId::new())).await;
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!((status, problem["code"].clone()), (StatusCode::NOT_FOUND, json!("artifact_not_found")));
    world.manager.stop_session(&alice, session_id).await.unwrap();
}

// --- Who may do what (issues 271, 23, 316, the security suite) -------------

/// Publishes the file `at` of the workspace of channel `channel` as a
/// version of the artifact `name`, in `ctx`'s name, as `artifact_tool` does
/// (issue 296): the workspace reached there for reading, the file read, and
/// what was read handed to the `ArtifactManager`.
async fn publish(world: &World, ctx: &SecurityContext, session: SessionId, channel: ChannelId, at: &WorkspacePath, name: &str, description: Option<&str>) -> Result<(Artifact, u32), ArtifactError> {
    let (workspace, terminal) = world.runtime.workspace_manager().terminal_in(ctx, session, channel, Grant::Read).await.expect("the workspace, to read");
    let content = terminal.read(at).await.expect("the file");
    manager(world).publish(ctx, content, &workspace.classification, name, description, session, channel).await
}

/// G4 (issue 308): an artifact shared with a team is among the artifacts
/// of its members while they are members - with its versions where it was
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
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = world.runtime.workspace_manager().terminal_in(&alice, session_id, main, Grant::Write).await.expect("hers to use");
    let at = WorkspacePath::parse("report.md").unwrap();
    terminal.write(&at, file("final"), 100).await.unwrap();
    let (whole, _) = publish(&world, &alice, session_id, main, &at, "report.md", None).await.expect("one to share with its versions");
    let (alone, _) = publish(&world, &alice, session_id, main, &at, "notes.md", None).await.expect("one to share without them");
    let listed = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move {
            let mut ids = views.artifacts().await.unwrap().iter().map(|a| a.artifact().id).collect::<Vec<_>>();
            ids.sort_by_key(|id| id.to_string());
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
    both.sort_by_key(|id| id.to_string());
    assert_eq!(listed(&carol).await, both, "a member finds both among hers");
    assert_eq!(read_version(&world, &carol, whole.id, 1).await, Read::Content(5, "final".to_string()), "and reads the version of the one shared with its versions");
    assert_eq!(read_version(&world, &carol, alone.id, 1).await, Read::NoVersion, "not that of the other, whose own list does not name the team");
    assert!(listed(&bob).await.is_empty(), "who is not in the team does not");
    groups.set_members(&alice, project.id, Vec::new()).await.unwrap();
    assert!(listed(&carol).await.is_empty(), "who left it no longer does");
    world.manager.stop_session(&alice, session_id).await.unwrap();
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
/// (`WriteArtifact`, issue 23) -, into the session's scope. Its owner reads
/// every version; a version is read by the members of the channel it was
/// published in, by the artifact, wherever they act; only its owner shares
/// and keeps it beyond the session, and what is not kept goes with the
/// session, its content too.
#[tokio::test]
async fn an_artifact_is_published_in_a_channel_for_its_publisher_and_read_by_who_reads_the_channel() {
    let world = world().await;
    let artifacts = manager(&world);
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let in_main = |ctx: &SecurityContext| {
        let views = world.manager.views(Viewer::new(ctx.clone()));
        async move { views.session(session_id).await.unwrap().channel(main).await.unwrap() }
    };
    let (_, terminal) = world.runtime.workspace_manager().terminal_in(&alice, session_id, main, Grant::Write).await.expect("hers to use");
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();

    let (published, asked) = world.asked(publish(&world, &alice, session_id, main, &at, "report.csv", Some("The figures"))).await;
    let (artifact, first) = published.expect("published");
    assert_eq!((first, &artifact.name.scope, artifact.owner()), (1, &Scope::SessionScope { session_id }, Some(world.alice)));
    assert!(asked.contains(&"alice: CreateArtifact(alice's) yes".to_string()), "made in her name: {asked:?}");
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
    assert!(matches!(artifacts.keep(&bob, artifact.id, Scope::UserScope { user_id: world.bob }).await, Err(ArtifactError::InvalidScope)), "not in his scope, whose it is not");
    assert!(matches!(artifacts.keep(&bob, artifact.id, Scope::GlobalScope).await, Err(ArtifactError::Store(StoreError::PermissionDenied(_)))), "nor keeps it at all");
    let kept_one = artifacts.keep(&alice, artifact.id, Scope::UserScope { user_id: world.alice }).await.expect("its owner keeps it");
    assert_eq!(kept_one.name.scope, Scope::UserScope { user_id: world.alice });
    let (left, _) = publish(&world, &alice, session_id, main, &at, "draft.csv", None).await.unwrap();

    world.manager.stop_session(&alice, session_id).await.unwrap();
    world.manager.delete_session(&alice, session_id).await.unwrap();
    world.manager.purge_session(&World::system(), session_id).await.unwrap();
    assert!(store(&world).get(&alice, artifact.id).await.is_ok(), "what was kept stays");
    assert!(world.files.path().join("artifacts").join(artifact.id.to_string()).is_dir(), "with its content");
    assert!(store(&world).get_deleted(&World::system(), left.id).await.is_err(), "what was not kept went with the session");
    assert!(!world.files.path().join("artifacts").join(left.id.to_string()).exists(), "its content too");
}

/// A session of alice's with her workspace attached for writing and a file
/// in it, and an artifact published from it in its channel "main" by each
/// of alice and bob - each his publisher's (issue 271), in the session's
/// scope.
async fn session_with_artifacts(world: &World) -> (Arc<SessionLoop>, Artifact, Artifact) {
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = world.runtime.workspace_manager().terminal_in(&alice, session_id, main, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();
    let (hers, _) = publish(world, &alice, session_id, main, &at, "hers.csv", None).await.unwrap();
    let (his, _) = publish(world, &bob, session_id, main, &at, "his.csv", None).await.unwrap();
    assert_eq!((hers.owner(), his.owner()), (Some(world.alice), Some(world.bob)));
    (session, hers, his)
}

/// Issue 320: what lies in a session's scope goes with it in the name of
/// whoever deletes it, as far as they may. An artifact another participant
/// published in the session is his: deleting the session deletes hers, in
/// her name, and keeps the session, telling her what is left - until he
/// keeps his elsewhere.
#[tokio::test]
async fn an_artifact_goes_with_its_session_only_as_far_as_whoever_deletes_it_may() {
    let world = world().await;
    let (alice, bob) = (World::as_user(world.alice), World::as_user(world.bob));
    let (session, hers, his) = session_with_artifacts(&world).await;
    let session_id = session.id();

    let (refused, asked) = world.asked(world.manager.delete_session(&alice, session_id)).await;
    let refused = refused.expect_err("bob's artifact is left");
    assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ScopeNotEmpty { left, .. }) if *left == [(Artifact::KIND, 1)]), "{refused}");
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
    let removal = store(&world).get_deleted(&World::system(), hers.id).await.unwrap().deleted.expect("hers went");
    assert_eq!(removal.by, ActorId::User(world.alice), "in her name");

    manager(&world).keep(&bob, his.id, Scope::UserScope { user_id: world.bob }).await.expect("bob keeps his");
    assert!(world.manager.delete_session(&alice, session_id).await.expect("nothing of anybody else's is left"));
    assert!(store(&world).get(&bob, his.id).await.is_ok(), "what bob kept stays");
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
    world.manager.delete_session(&World::system(), session_id).await.expect("the system's to delete, all of it");

    let (restored, asked) = world.asked(world.manager.restore_session(&alice, session_id)).await;
    restored.expect("restored all the same");
    assert!(asked.contains(&"alice: ReadDeletedArtifact(bob's) no".to_string()), "asked in her name: {asked:?}");
    assert!(store(&world).get(&alice, hers.id).await.is_ok(), "hers comes back with it");
    assert!(store(&world).get_deleted(&World::system(), his.id).await.is_ok(), "bob's stays deleted: not hers to restore");
    manager(&world).restore(&bob, his.id).await.expect("bob restores his");
}

/// Issue 319: what is said of an artifact as a whole reaches whoever reads
/// it, however it went or came back - here with its session, to a member of
/// the channel a version was published in, whom publishing let read it
/// (issue 316). And the groups of the participants, deleted
/// and restored with the session, are announced as a team is.
#[tokio::test]
async fn what_goes_and_comes_back_with_a_session_is_announced_to_whoever_reads_it() {
    let world = world().await;
    let alice = World::as_user(world.alice);
    let docs = world.runtime.workspace_manager().create(&alice, "docs", Scope::UserScope { user_id: world.alice }, "main", None, Classification::general(Confidentiality::Internal)).await.unwrap();
    let session = world.shared_session(&[], &[]).await;
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let (_, terminal) = world.runtime.workspace_manager().terminal_in(&alice, session_id, main, Grant::Write).await.unwrap();
    let at = WorkspacePath::parse("out/report.csv").unwrap();
    terminal.write(&at, file("a,b"), 100).await.unwrap();
    let (artifact, _) = publish(&world, &alice, session_id, main, &at, "report.csv", None).await.unwrap();
    let id = artifact.id;
    let (of_session, of_channel) = (GroupId::of_session(session_id), GroupId::of_channel(main));
    assert_eq!(artifact.security_attributes.acl, [AclEntry { subject: Subject::Group(of_channel), grant: Grant::Read }], "bob reads it as a member of the channel");

    let mut bobs = world.runtime.event_bus().subscribe_as(World::as_user(world.bob));
    world.manager.stop_session(&alice, session_id).await.unwrap();
    world.manager.delete_session(&alice, session_id).await.unwrap();
    hears(&mut bobs, [GlobalEvent::deleted::<Artifact>(&id), GlobalEvent::deleted::<Group>(&of_session), GlobalEvent::deleted::<Group>(&of_channel)]).await;

    world.manager.restore_session(&alice, session_id).await.unwrap();
    hears(&mut bobs, [GlobalEvent::restored::<Artifact>(&id), GlobalEvent::restored::<Group>(&of_session), GlobalEvent::restored::<Group>(&of_channel)]).await;
}

/// Issues 23, 316: a version is read by the members of the channel it was
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
    session.set_workspace(&alice, Some(docs.id), Some(Grant::Write)).await.unwrap();
    let drafts = session.create_channel(&alice, ChannelSpec { name: "drafts".to_string(), components: quiet(), participants: vec![], classification: None, language: None, uses_workspace: None }).await.expect("hers alone").id();
    let (session_id, main) = (session.id(), session.channels()[0].id());
    let workspaces = world.runtime.workspace_manager();
    let write_in = |channel, text| {
        let alice = &alice;
        async move {
            let (_, terminal) = workspaces.terminal_in(alice, session_id, channel, Grant::Write).await.expect("hers to use");
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
    world.manager.stop_session(&alice, session_id).await.unwrap();
}

/// An agent's publication of an artifact (issue 271) asks what a call on
/// the workspace asks, and makes the artifact its owner's - asked in the
/// same context, `CreateArtifact` for alice.
#[tokio::test]
async fn an_agents_artifact_is_published_for_its_owner() {
    let call = calling(PUBLISH_ARTIFACT, json!({ "path": "notes/a.md" }));
    let world = World::configured(&[&ArtifactsPlugin], kept_beside, |registry| register_once(registry, ONCE_MODEL_KIND, call)).await;
    let kept = |files: &Path, _: &Path| std::fs::read_dir(files.join("artifacts")).is_ok_and(|mut entries| entries.next().is_some());
    let asked = world.asked_for_a_call(ARTIFACT_TOOL_KIND, &["notes/a.md"], kept, world.alice).await;
    asked_as_a_call_on_the_workspace(&asked);
    assert!(asked.contains(&"alice as helper, asked by bob: CreateArtifact(alice's) yes".to_string()), "its owner's: {asked:?}");
}
