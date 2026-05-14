// mod engine;

use axum::async_trait;
use axum::{
    routing::{get, post},
    Router,
    Json,
    extract::{Path, State},
};
use std::sync::Arc;
use uuid::Uuid;
use caelumora_runtime::session::SessionManager;
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_model::agent::{Session, MessageRole, AgentSnapshot, SessionTemplate};
use serde::Deserialize;

struct AppState {
    runtime_manager: Arc<RuntimeManager>,
    session_manager: SessionManager,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let runtime_manager = Arc::new(RuntimeManager::new(Arc::new(NoopToolExecutor)));
    let session_manager = SessionManager::new(runtime_manager.clone());

    let state = Arc::new(AppState {
        runtime_manager,
        session_manager,
    });

    let app = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/:id", get(get_session))
        .route("/sessions/:id/messages", post(add_message))
        .route("/agents", post(create_agent))
        .route("/agents/:id", get(get_agent))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    println!("Caelumora Core Engine Prototype listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<Session>> {
    Json(state.session_manager.list_sessions())
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    name: String,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSessionRequest>,
) -> Json<Session> {
    let template = SessionTemplate {
        id: Uuid::new_v4(),
        name: payload.name,
        participants: Vec::new(),
    };
    Json(state.session_manager.create_session_loop(&template).unwrap())
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Session>, axum::http::StatusCode> {
    state.session_manager.get_session(id)
        .map(Json)
        .ok_or(axum::http::StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct AddMessageRequest {
    sender_id: Uuid,
    role: MessageRole,
    content: String,
}

async fn add_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<AddMessageRequest>,
) -> Result<Json<caelumora_model::agent::Message>, axum::http::StatusCode> {
    state.session_manager.add_message(id, payload.sender_id, payload.role, payload.content)
        .map(Json)
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
struct CreateAgentRequest {
    name: String,
    template_id: Uuid,
}

struct NoopToolExecutor;
#[async_trait]
impl caelumora_model::traits::ToolExecutor for NoopToolExecutor {
    fn execute<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _tool: &'life1 caelumora_model::agent::Tool,
        _input: serde_json::Value,
        _execution: &'life2 mut caelumora_model::agent::Execution,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = anyhow::Result<serde_json::Value>>
                + core::marker::Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(serde_json::Value::Null) })
    }
}

async fn create_agent(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateAgentRequest>,
) -> Json<Uuid> {
    // In a real application, we would fetch the template from a TemplateManager or database.
    // For now, we'll create a dummy template with the provided ID.
    let template = caelumora_model::agent::Personality {
        id: payload.template_id,
        name: format!("Template for {}", payload.name),
        description: "Auto-generated template".to_string(),
        capabilities: Vec::new(),
    };
    Json(state.runtime_manager
        .agent_manager()
        .create_agent(payload.name, &template, state.runtime_manager.clone()))
}

async fn get_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentSnapshot>, axum::http::StatusCode> {
    state.runtime_manager.agent_manager().get_agent(id)
        .map(Json)
        .ok_or(axum::http::StatusCode::NOT_FOUND)
}
