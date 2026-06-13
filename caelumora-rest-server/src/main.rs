use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use caelumora_model::driver::memory::MemoryStore;
use caelumora_model::message::{Message, MessageRole};
use caelumora_model::agent::{Personality, PersonalityId};
use caelumora_model::component::{
    ComponentConfig, ComponentFailPolicy, ComponentNodeConfig, STAGE_PERCEIVE,
};
use caelumora_model::meta::{Metadata, Scope};
use caelumora_model::session::{SessionContext, SessionId};
use caelumora_model::store::{StoreBackedEntityResolver, TemplateStore};
use caelumora_model::template::{
    ChannelTemplate, ParticipantTemplate, ScopeTemplate, SessionTemplate, SessionTemplateId,
};
use caelumora_model::user::UserId;
use caelumora_runtime::actor::ActorEvent;
use caelumora_runtime::component::{
    STATE_CONTEXT_APPENDER_KIND, STATE_CONTEXT_PROVIDER_KIND, STATE_HISTORY_WRITER_KIND,
    STATIC_SYSTEM_PROMPT_PROVIDER_KIND,
};
use caelumora_runtime::runtime::RuntimeManager;
use caelumora_runtime::session::SessionManager;
use dashmap::DashMap;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

struct AppState {
    #[allow(dead_code)]
    runtime_manager: Arc<RuntimeManager>,
    session_manager: Arc<SessionManager>,
    memory_store: Arc<MemoryStore>,
    // Map of session ID to join handles of running session loops
    running_sessions: DashMap<SessionId, tokio::task::JoinHandle<()>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let memory_store = Arc::new(MemoryStore::new());
    let runtime_manager = Arc::new(RuntimeManager::new(
        memory_store.clone(),
        memory_store.clone(),
    ));
    let session_manager = Arc::new(SessionManager::new(runtime_manager.clone()));

    setup_initial_data(&memory_store)?;

    let state = Arc::new(AppState {
        runtime_manager,
        session_manager,
        memory_store,
        running_sessions: DashMap::new(),
    });

    let app = Router::new()
        .route("/templates", get(list_templates))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/:id", delete(terminate_session))
        .route("/sessions/:id/messages", post(send_message))
        .route("/sessions/:id/events", get(join_session))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    tracing::info!("Caelumora REST server listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

fn setup_initial_data(store: &Arc<MemoryStore>) -> anyhow::Result<()> {
    let personality_a = create_personality(
        "jonathan-assistant",
        "A helpful and polite assistant",
        "Your name is Jonathan Heart. You are a helpful assistant.",
    );
    store.put_personality(&personality_a)?;

    let personality_b = create_personality(
        "sassy-assistant",
        "A sassy and witty assistant",
        "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant.",
    );
    store.put_personality(&personality_b)?;

    let template = SessionTemplate {
        id: SessionTemplateId(Uuid::new_v4()),
        metadata: Metadata {
            name: "simple-session".to_string(),
            scope: Scope::GlobalScope,
            description: "Simple user+assistant session".to_string(),
            ..Metadata::default()
        },
        scope: ScopeTemplate::GlobalScope,
        channels: vec![ChannelTemplate {
            name: "main".to_string(),
            moderator: "round_robin".to_string(),
            participants: vec![
                ParticipantTemplate::User,
                ParticipantTemplate::Agent {
                    name: "Jonathan".to_string(),
                    scope: ScopeTemplate::GlobalScope,
                    personality_id: personality_a.id,
                },
                ParticipantTemplate::Agent {
                    name: "Sarah".to_string(),
                    scope: ScopeTemplate::GlobalScope,
                    personality_id: personality_b.id,
                },
            ],
        }],
    };
    store.put_session_template(&template)?;

    Ok(())
}

fn create_personality(id: &str, description: &str, system_prompt: &str) -> Personality {
    Personality {
        id: PersonalityId(Uuid::new_v4()),
        metadata: Metadata {
            name: id.to_string(),
            scope: Scope::GlobalScope,
            description: description.to_string(),
            ..Metadata::default()
        },
        components: create_component_config(system_prompt),
    }
}

fn create_component_config(system_prompt: &str) -> ComponentConfig {
    ComponentConfig {
        component_nodes: vec![
            ComponentNodeConfig {
                id: "system-prompt".to_string(),
                kind: STATIC_SYSTEM_PROMPT_PROVIDER_KIND.to_string(),
                params: serde_json::json!({ "prompt": system_prompt }),
                deps: vec![],
                tags: vec![],
                fail_policy: ComponentFailPolicy::FailOpen,
            },
            ComponentNodeConfig {
                id: "history-writer".to_string(),
                kind: STATE_HISTORY_WRITER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec![],
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailOpen,
            },
            ComponentNodeConfig {
                id: "context-provider".to_string(),
                kind: STATE_CONTEXT_PROVIDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec!["system-prompt".to_string()],
                tags: vec![],
                fail_policy: ComponentFailPolicy::FailOpen,
            },
            ComponentNodeConfig {
                id: "context-appender".to_string(),
                kind: STATE_CONTEXT_APPENDER_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: vec![],
                tags: vec![STAGE_PERCEIVE.to_string()],
                fail_policy: ComponentFailPolicy::FailOpen,
            },
        ],
    }
}

// --- Handlers ---

async fn list_templates(State(state): State<Arc<AppState>>) -> Json<Vec<SessionTemplate>> {
    let templates = state.memory_store.list_session_templates().unwrap_or_default();
    Json(templates)
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    template_id: Uuid,
    name: String,
}

#[derive(Serialize)]
struct SessionResponse {
    id: SessionId,
    name: String,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, axum::http::StatusCode> {
    let template_id = SessionTemplateId(payload.template_id);
    let templates = state.memory_store.list_session_templates().map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let template = templates.into_iter().find(|t| t.id == template_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let session_id = SessionId(Uuid::new_v4());
    let owner = UserId(Uuid::new_v4()); // In a real app, this comes from auth

    let context = SessionContext {
        id: session_id,
        owner,
        resolver: Box::new(StoreBackedEntityResolver::new(
            state.memory_store.clone(),
            state.memory_store.clone(),
        )),
    };

    let session = template.instantiate(payload.name.clone(), &context)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_loop = state.session_manager.create_session(session)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_loop_clone = session_loop.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = session_loop_clone.run().await {
            tracing::error!("Session loop error: {}", e);
        }
    });

    state.running_sessions.insert(session_id, handle);

    Ok(Json(SessionResponse {
        id: session_id,
        name: payload.name,
    }))
}

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionId>> {
    let session_ids: Vec<SessionId> = state.running_sessions.iter().map(|r| *r.key()).collect();
    Json(session_ids)
}

async fn terminate_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> axum::http::StatusCode {
    let session_id = SessionId(id);
    if let Some((_, handle)) = state.running_sessions.remove(&session_id) {
        handle.abort();
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(payload): Json<SendMessageRequest>,
) -> axum::http::StatusCode {
    let session_id = SessionId(id);
    let session_loop = match state.session_manager.get_session(session_id) {
        Some(s) => s,
        None => return axum::http::StatusCode::NOT_FOUND,
    };

    // Find the UserActor in the main channel (or any channel for now)
    // Simplified: just use the first channel's user actor
    let user_actor = session_loop.channel_by_name("main")
        .and_then(|c| c.user_actor());

    if let Some(user_actor) = user_actor {
        let message = ActorEvent::CompleteMessageGenerated(Message {
            role: MessageRole::User,
            content: Some(payload.content),
            ..Message::default()
        });
        if user_actor.send_message(message).await.is_ok() {
            axum::http::StatusCode::OK
        } else {
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }
    } else {
        axum::http::StatusCode::BAD_REQUEST
    }
}

async fn join_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, axum::http::StatusCode> {
    let session_id = SessionId(id);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let user_actor = session_loop.channel_by_name("main")
        .and_then(|c| c.user_actor())
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

    // This is a bit tricky because user_actor.lock_receiver() returns a MutexGuard.
    // We need to stream events from the user actor to the SSE.
    // However, the UserActor's actor_rx is intended for ONE consumer (the TUI or here the SSE).

    let (tx, rx) = mpsc::unbounded_channel();
    
    tokio::spawn(async move {
        let mut receiver = user_actor.lock_receiver().await;
        while let Some(event) = receiver.recv().await {
            let event_json = serde_json::to_string(&event).unwrap_or_default();
            if tx.send(Result::<Event, Infallible>::Ok(Event::default().data(event_json))).is_err() {
                break;
            }
        }
    });

    Ok(Sse::new(UnboundedReceiverStream::new(rx)))
}
