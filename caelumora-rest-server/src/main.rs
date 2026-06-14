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
use caelumora_model::session::{ChannelId, SessionContext, SessionId};
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
use caelumora_runtime::session::{SessionManager};
use dashmap::DashMap;
use futures::stream::Stream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

struct AppState {
    #[allow(dead_code)]
    runtime_manager: Arc<RuntimeManager>,
    session_manager: Arc<SessionManager>,
    memory_store: Arc<MemoryStore>,
    // Map of session ID to join handles of running session loops
    running_sessions: DashMap<SessionId, tokio::task::JoinHandle<()>>,
}

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=debug,axum::rejection=trace".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

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
        .route("/sessions/{id}", delete(terminate_session))
        .route("/sessions/{id}/channels", get(list_channels))
        .route("/sessions/{id}/channels/{cid}", get(get_channel))
        .route("/sessions/{id}/channels/{cid}/messages", get(get_channel_messages).post(send_message))
        .route("/sessions/{id}/channels/{cid}/cancel", post(cancel_channel))
        .route("/sessions/{id}/channels/{cid}/events", get(join_channel))
        .layer(TraceLayer::new_for_http())
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
        "Your name is Jonathan Heart. You are a self-made millionaire. Your butler is called Max and your fluffy dog is called Freeway.",
    );
    store.put_personality(&personality_a)?;

    let personality_b = create_personality(
        "sassy-assistant",
        "A sassy and witty assistant",
        "Your name is Sassy Sarah. You are a witty and slightly sarcastic assistant who loves to play quiz games.",
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

async fn list_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionResponse>> {
    let sessions: Vec<SessionResponse> = state.session_manager.sessions.iter().map(|r| {
        let session_loop = r.value();
        SessionResponse {
            id: session_loop.id(),
            name: session_loop.session.metadata.name.clone(),
        }
    }).collect();
    Json(sessions)
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

#[derive(Serialize)]
struct ChannelResponse {
    id: ChannelId,
    name: String,
    participants: Vec<String>,
}

async fn list_channels(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ChannelResponse>>, axum::http::StatusCode> {
    let session_id = SessionId(id);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let channels = session_loop.channels()
        .iter()
        .map(|c| ChannelResponse {
            id: c.id(),
            name: c.name(),
            participants: c.actor_names(),
        })
        .collect();

    Ok(Json(channels))
}

async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<ChannelResponse>, axum::http::StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let channel = session_loop.channel_by_id(channel_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(ChannelResponse {
        id: channel.id(),
        name: channel.name(),
        participants: channel.actor_names(),
    }))
}

async fn get_channel_messages(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<caelumora_model::message::Message>>, axum::http::StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let history = session_loop.get_channel_history(channel_id).await
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(history))
}

#[derive(Deserialize)]
struct SendMessageRequest {
    content: String,
}

async fn send_message(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(payload): Json<SendMessageRequest>,
) -> axum::http::StatusCode {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session_loop = match state.session_manager.get_session(session_id) {
        Some(s) => s,
        None => return axum::http::StatusCode::NOT_FOUND,
    };

    let channel = match session_loop.channel_by_id(channel_id) {
        Some(c) => c,
        None => return axum::http::StatusCode::NOT_FOUND,
    };

    let user_actor = channel.user_actor();

    if let Some(user_actor) = user_actor {
        let wakeup = ActorEvent::Wakeup;
        let message = ActorEvent::FullResponseGenerated(vec![Message {
            role: MessageRole::User,
            content: Some(payload.content),
            ..Message::default()
        }]);
        let _ = user_actor.send_message(wakeup).await;
        match user_actor.send_message(message).await {
            Ok(_) => axum::http::StatusCode::OK,
            Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }
    } else {
        axum::http::StatusCode::BAD_REQUEST
    }
}

async fn cancel_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> axum::http::StatusCode {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);

    let session_loop = match state.session_manager.get_session(session_id) {
        Some(s) => s,
        None => return axum::http::StatusCode::NOT_FOUND,
    };

    match session_loop.cancel_channel(channel_id).await {
        Ok(_) => axum::http::StatusCode::OK,
        Err(_) => axum::http::StatusCode::NOT_FOUND,
    }
}


async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, axum::http::StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);
    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let channel = session_loop.channel_by_id(channel_id)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let receiver = channel.subscribe();
    let stream = BroadcastStream::new(receiver);

    let sse_stream = stream.map(|res| {
        match res {
            Ok(envelope) => {
                let event_json = serde_json::to_string(&envelope).unwrap_or_default();
                Ok(Event::default().data(event_json))
            }
            Err(e) => {
                tracing::error!("SSE stream error: {}", e);
                // In SSE, we can just skip or send a comment. Here we just continue or the stream might close.
                // For now, we return a default event or just map it.
                Ok(Event::default().comment("stream error"))
            }
        }
    });

    Ok(Sse::new(sse_stream))
}
