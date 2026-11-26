//! The `openapi_server` component against a server of the test's own, which serves its OpenAPI
//! document and answers its operations.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use axum::body::Bytes;
use axum::extract::RawQuery;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, patch};
use axum::{Json, Router};
use caelumora_agents::library::AgentsLibrary;
use caelumora_agents::testing::{described_tools, executed_anywhere, offered_anywhere};
use caelumora_agents::tool::{ToolError, ToolProvider};
use caelumora_domain::agent::Personality;
use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
use caelumora_domain::confidentiality::{Classification, Confidentiality};
use caelumora_domain::tool::ToolDescription;
use caelumora_kernel::component::testing::{built, descriptor};
use caelumora_kernel::kernel::Kernel;
use caelumora_kernel::testing::{asked_for, TestKernel, World};
use caelumora_store::Persistence;
use caelumora_tool_openapi::component::OPENAPI_SERVER_KIND;
use caelumora_tool_openapi::library::OpenApiToolLibrary;
use serde_json::{json, Value};
use std::fs;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::net::TcpListener;

/// The size of the log the test's server answers, above the bound of the tests.
const LOG_BYTES: usize = 10_000;

/// The document of the test's server, of Kimai's shape: absolute paths, a query parameter named
/// with `[]`, a body, an operation answering nothing, text, a long text, a failure and a PDF.
fn document() -> Value {
    let users = json!({
        "name": "users[]", "in": "query", "schema": { "type": "array", "items": [] }
    });
    let id =
        json!({ "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } });
    let form = json!({ "application/json": { "schema": { "type": "object" } } });
    json!({
        "openapi": "3.0.0",
        "info": { "title": "Tracker", "description": "Tracks the time." },
        "paths": {
            "/api/timesheets": {
                "get": {
                    "operationId": "get_timesheets",
                    "summary": "Returns timesheets",
                    "parameters": [users]
                },
                "post": {
                    "operationId": "post_timesheet",
                    "requestBody": { "required": true, "content": form }
                }
            },
            "/api/timesheets/{id}/stop": {
                "patch": { "operationId": "patch_stop_timesheet", "parameters": [id] }
            },
            "/api/version": { "get": { "operationId": "get_version" } },
            "/api/log": { "get": { "operationId": "get_log" } },
            "/api/fail": { "get": { "operationId": "get_fail" } },
            "/api/export": { "get": { "operationId": "get_export" } }
        }
    })
}

/// Serves the test's server on a port of its own, its document at `/openapi.json`; its URL.
async fn start_server() -> String {
    async fn timesheets(RawQuery(query): RawQuery) -> Json<Value> {
        Json(json!({ "query": query }))
    }
    async fn created(body: Bytes) -> Json<Value> {
        Json(json!({ "created": serde_json::from_slice::<Value>(&body).unwrap() }))
    }
    async fn failing() -> impl IntoResponse {
        (StatusCode::INTERNAL_SERVER_ERROR, "the database is down")
    }
    async fn exported() -> impl IntoResponse {
        ([(header::CONTENT_TYPE, "application/pdf")], "%PDF-1.7")
    }
    let router = Router::new()
        .route("/openapi.json", get(|| async { Json(document()) }))
        .route("/api/timesheets", get(timesheets).post(created))
        .route("/api/timesheets/{id}/stop", patch(|| async { StatusCode::NO_CONTENT }))
        .route("/api/version", get(|| async { "2.40" }))
        .route("/api/log", get(|| async { "x".repeat(LOG_BYTES) }))
        .route("/api/fail", get(failing))
        .route("/api/export", get(exported));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

/// A kernel over `store` with the library of OpenAPI servers, as the programs build theirs.
fn kernel(store: &Persistence) -> Arc<Kernel> {
    TestKernel {
        store: store.clone(),
        libraries: vec![&AgentsLibrary, &OpenApiToolLibrary],
        ..Default::default()
    }
    .build()
}

/// The tool provider of the one component, `tracker`, configured with `params`.
async fn provider(params: Value) -> Arc<dyn ToolProvider> {
    let kernel = kernel(&Persistence::in_memory());
    built(&kernel, descriptor("tracker", OPENAPI_SERVER_KIND, params)).await.unwrap()
}

/// The params of `tracker` at `url`, offering every operation of its document.
fn every_operation(url: &str) -> Value {
    json!({
        "endpoint": { "url": url },
        "requires_approval": "unless_read_only",
        "classification": { "level": "confidential" },
        "operations": {
            "get_timesheets": {},
            "post_timesheet": {},
            "patch_stop_timesheet": { "requires_approval": false },
            "get_version": {},
            "get_log": {},
            "get_fail": {},
            "get_export": {}
        },
    })
}

/// The names of `tools` with whether each asks for approval.
fn asking(tools: &[ToolDescription]) -> Vec<(&str, bool)> {
    tools.iter().map(|tool| (tool.name.as_str(), tool.requires_approval)).collect()
}

/// What the tool `name` of `provider` answers to `params`.
async fn answered(provider: &dyn ToolProvider, name: &str, params: Value) -> Value {
    executed_anywhere(provider, name, params).await.unwrap().value
}

#[tokio::test]
async fn the_operations_chosen_are_read_of_the_server_when_offered_and_called() {
    let url = start_server().await;
    let provider = provider(every_operation(&url)).await;
    assert!(provider.tools().is_empty(), "nothing read while built (issue 126)");
    assert_eq!(provider.description(), "The operations of the OpenAPI server 'tracker'.");

    let offered = offered_anywhere(provider.as_ref()).await.unwrap();
    assert_eq!(
        asking(&offered),
        [
            ("tracker__get_export", false),
            ("tracker__get_fail", false),
            ("tracker__get_log", false),
            ("tracker__get_timesheets", false),
            ("tracker__get_version", false),
            ("tracker__patch_stop_timesheet", false),
            ("tracker__post_timesheet", true),
        ],
        "as chosen; GET trusted to read, as configured, the stop by its own"
    );
    let confidential = Classification::general(Confidentiality::Confidential);
    assert!(
        offered.iter().all(|tool| tool.classification.as_ref() == Some(&confidential)),
        "the configured classification reaches every tool: {offered:?}"
    );
    assert_eq!(described_tools(provider.as_ref()).len(), 7, "kept");
    assert_eq!(provider.description(), "Tracks the time.", "as the document says");

    let provider = provider.as_ref();
    let found = answered(provider, "tracker__get_timesheets", json!({ "users": [1, 2] })).await;
    assert_eq!(found, json!({ "query": "users%5B%5D=1&users%5B%5D=2" }), "JSON as JSON");
    let body = json!({ "body": { "project": 3 } });
    let created = answered(provider, "tracker__post_timesheet", body).await;
    assert_eq!(created, json!({ "created": { "project": 3 } }), "the body sent as JSON");
    let stopped = answered(provider, "tracker__patch_stop_timesheet", json!({ "id": 7 })).await;
    assert_eq!(stopped, json!({ "status": 204 }), "nothing answered, as its status");
    let version = answered(provider, "tracker__get_version", json!({})).await;
    assert_eq!(version, json!("2.40"), "text as text");
}

#[tokio::test]
async fn a_failure_or_what_cannot_be_given_fails_the_call() {
    let url = start_server().await;
    let provider = provider(every_operation(&url)).await;
    offered_anywhere(provider.as_ref()).await.unwrap();
    let failed = executed_anywhere(provider.as_ref(), "tracker__get_fail", json!({})).await;
    let Err(ToolError::ExecutionFailed(failed)) = failed else { panic!("it failed: {failed:?}") };
    assert!(failed.contains("500") && failed.contains("the database is down"), "{failed}");
    let export = executed_anywhere(provider.as_ref(), "tracker__get_export", json!({})).await;
    let Err(ToolError::ExecutionFailed(export)) = export else { panic!("refused: {export:?}") };
    assert!(export.contains("application/pdf"), "{export}");
}

#[tokio::test]
async fn an_operation_the_document_lacks_leaves_the_tools_unavailable() {
    let url = start_server().await;
    let operations = json!({ "get_timesheets": {}, "delete_everything": {} });
    let provider = provider(json!({ "endpoint": { "url": url }, "operations": operations })).await;
    let unavailable = offered_anywhere(provider.as_ref()).await.unwrap_err();
    assert_eq!(unavailable, "its document does not offer the operations it is configured with");
}

#[tokio::test]
async fn a_server_that_does_not_give_its_document_leaves_the_tools_unavailable() {
    let unreachable = json!({ "url": "http://user:secret@127.0.0.1:1" });
    let unreached = provider(json!({ "endpoint": unreachable, "operations": { "a": {} } })).await;
    let unavailable = offered_anywhere(unreached.as_ref()).await.unwrap_err();
    assert_eq!(unavailable, "its server cannot be reached");
    let url = start_server().await;
    let provider = provider(json!({
        "endpoint": { "url": url }, "spec_path": "api/doc.json", "operations": { "a": {} }
    }))
    .await;
    let unavailable = offered_anywhere(provider.as_ref()).await.unwrap_err();
    assert_eq!(unavailable, "its server did not give its document", "a status of failure");
}

#[tokio::test]
async fn a_document_in_a_file_is_read_once_and_the_server_called() {
    let url = start_server().await;
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), document().to_string()).unwrap();
    let provider = provider(json!({
        "endpoint": { "url": url },
        "spec_file": file.path(),
        "requires_approval": false,
        "operations": { "get_version": {} },
    }))
    .await;
    let offered = offered_anywhere(provider.as_ref()).await.unwrap();
    assert_eq!(asking(&offered), [("tracker__get_version", false)]);
    fs::write(file.path(), "no JSON").unwrap();
    let offered = offered_anywhere(provider.as_ref()).await.unwrap();
    assert_eq!(offered.len(), 1, "read once");
    let version = answered(provider.as_ref(), "tracker__get_version", json!({})).await;
    assert_eq!(version, json!("2.40"));
}

#[tokio::test]
async fn a_file_that_holds_no_document_is_told_without_what_it_holds() {
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), r#"{ "secret": "hunter2" }"#).unwrap();
    let provider = provider(json!({
        "endpoint": { "url": "http://127.0.0.1:1" },
        "spec_file": file.path(),
        "operations": { "a": {} },
    }))
    .await;
    let unavailable = offered_anywhere(provider.as_ref()).await.unwrap_err();
    assert_eq!(unavailable, "its document cannot be read");
}

/// Issue 552: a component with a bound keeps what lies above it, text beside a preview, JSON
/// without one.
#[tokio::test]
async fn an_answer_above_the_bound_is_kept_whole() {
    let url = start_server().await;
    let params = json!({
        "endpoint": { "url": url },
        "keep_whole_above_bytes": 4096,
        "operations": { "get_timesheets": {}, "get_log": {} },
    });
    let provider = provider(params).await;
    offered_anywhere(provider.as_ref()).await.unwrap();
    let many: Vec<u32> = (0..2000).collect();
    let users = json!({ "users": many });
    let found = executed_anywhere(provider.as_ref(), "tracker__get_timesheets", users).await;
    let found = found.unwrap();
    let [kept] = found.files.as_slice() else { panic!("one file: {:?}", found.files) };
    assert_eq!(kept.name, "tracker.json");
    assert!(found.value.get("content").is_none(), "JSON is not cut in the middle");
    assert!(found.value["next"].is_string(), "says where it is: {}", found.value);

    let log = executed_anywhere(provider.as_ref(), "tracker__get_log", json!({})).await.unwrap();
    let [kept] = log.files.as_slice() else { panic!("one file: {:?}", log.files) };
    let whole = "x".repeat(LOG_BYTES);
    assert_eq!((kept.name.as_str(), kept.bytes.as_ref()), ("tracker.txt", whole.as_bytes()));
    let preview = log.value["content"].as_str().unwrap();
    assert!(preview.len() < LOG_BYTES, "a preview of the text");
}

/// Issues 126, 310: the document is read in the name of whoever acts, as a call is made (a
/// component keeps no security context for later): nothing is read while the agent is built -
/// only whether its owner may use its credential is asked -, and its credential is read for
/// alice's agent, asked by bob, as its turn begins.
#[tokio::test]
async fn the_document_is_read_in_the_name_of_whoever_acts() {
    let world = World::with(&[], &[&AgentsLibrary, &OpenApiToolLibrary], |_| {}).await;
    let auth = json!({ "type": "bearer", "secret": "test/TRACKER_TOKEN" });
    let tracker = ComponentDescriptor {
        params: json!({
            "endpoint": { "url": "http://127.0.0.1:1", "auth": auth },
            "operations": { "get_timesheets": {} },
        }),
        ..ComponentDescriptor::new("tracker", OPENAPI_SERVER_KIND)
    };
    let personality = world.personality("working", None, &[]).await;
    let components = ComponentConfig { component_nodes: vec![tracker] };
    let mut working = Personality { components, ..personality };
    let personalities = &world.kernel.entity_stores().personalities;
    personalities.update(&World::system(), &mut working).await.unwrap();
    let helper = world.agent("helper", world.alice, &working).await;

    let (session, asked) = world.asked(world.shared_session(&[helper], &[])).await;
    let used: Vec<&String> = asked.iter().filter(|line| line.contains("UseSecret")).collect();
    assert_eq!(
        used,
        ["alice as helper: UseSecret(system's) yes"],
        "while built, only asked whether alice may use it (issue 310), nothing read: {asked:?}"
    );

    let main = Arc::clone(&session.channels()[0]);
    let (_, asked) = world
        .asked(async {
            main.user_actor(world.bob)
                .expect("he takes part")
                .send_message(None, Some("@helper do it".to_string()), None, Vec::new())
                .await
                .expect("sent");
            asked_for(&world, "alice as helper, asked by bob: UseSecret(system's) yes").await;
        })
        .await;
    let used: Vec<&String> = asked.iter().filter(|line| line.contains("UseSecret")).collect();
    assert_eq!(
        used,
        ["alice as helper, asked by bob: UseSecret(system's) yes"],
        "with alice's credential, asked by bob"
    );
    world.kernel.stop_session(&World::as_user(world.alice), session.id()).await.unwrap();
}
