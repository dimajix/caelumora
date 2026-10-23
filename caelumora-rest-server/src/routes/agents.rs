//! The agents a user may read, on their own (issues 164, 227): listing and
//! reading them, with the channels they take part in; making one of a
//! personality, renaming, deleting, restoring and purging it. And the
//! personalities an agent can be made of.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Query};
use caelumora_rest_api::agent::{AgentChannelResponse, AgentResponse, CreateAgentRequest, PersonalityResponse, RenameAgentRequest};
use caelumora_rest_api::trash::{DeletedResponse, ListOptions};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use caelumora_model::agent::AgentId;
use std::sync::Arc;
use uuid::Uuid;
use crate::routes::views;

/// The agents the user may read - or, with `?deleted=true`, those they
/// deleted and may read as such, the trash.
pub(super) async fn list_agents(State(state): State<Arc<AppState>>, Query(options): Query<ListOptions>, user: AuthenticatedUser) -> Result<Response, ApiError> {
    let views = views(&state, &user);
    if options.deleted {
        let deleted = views.deleted_agents().await?;
        return Ok(Json(deleted.into_iter().map(DeletedResponse::from).collect::<Vec<_>>()).into_response());
    }
    let agents = views.agents().await?;
    let mut listed = Vec::with_capacity(agents.len());
    for agent in agents {
        listed.push(AgentResponse::from(agent.snapshot().await?));
    }
    Ok(Json(listed).into_response())
}

/// Makes an agent of the personality the body names, the user's, in their
/// scope, the global one, or a session's or channel's they may read.
pub(super) async fn create_agent(State(state): State<Arc<AppState>>, user: AuthenticatedUser, Json(request): Json<CreateAgentRequest>) -> Result<(StatusCode, Json<AgentResponse>), ApiError> {
    let scope = request.scope.place(user.id, request.session_id, request.channel_id)
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_scope", "An agent of a session's scope names its session, and only it names a session or a channel"))?;
    let made = state.runtime_manager.agent_manager()
        .create(&user.security_context(), &request.name, scope, request.personality_id).await?;
    Ok((StatusCode::CREATED, Json(readable(&state, &user, made.id).await?)))
}

/// Agent `id`, with the channels it takes part in that the user may read.
pub(super) async fn get_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<AgentResponse>, ApiError> {
    let agent = views(&state, &user).agent(AgentId(id)).await?;
    let channels = agent.channels().await?.into_iter().map(|channel| AgentChannelResponse {
        session_id: channel.session().id(),
        session_name: channel.session().name().to_string(),
        channel_id: channel.id(),
        channel_name: channel.name().to_string(),
    }).collect();
    Ok(Json(AgentResponse { channels: Some(channels), ..agent.snapshot().await?.into() }))
}

/// Renames agent `id`, through the controller this instance shares: an
/// agent that runs takes the new name at once.
pub(super) async fn rename_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser, Json(request): Json<RenameAgentRequest>) -> Result<Json<AgentResponse>, ApiError> {
    let id = AgentId(id);
    let agent = state.runtime_manager.controller_manager().agent_controller(id).await?;
    agent.update_name(&user.security_context(), &request.name).await?;
    Ok(Json(readable(&state, &user, id).await?))
}

/// Deletes agent `id`: it rests in the channels it takes part in, as a
/// former participant, and is kept marked until it is purged.
pub(super) async fn delete_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    let deleted = state.runtime_manager.agent_manager().delete(&user.security_context(), agent_id).await?;
    match deleted {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("agent_not_found", format!("There is no agent {agent_id}"))),
    }
}

/// Restores the deleted agent `id`; it takes part again in the channels it
/// rested in. `409` if another agent took its name meanwhile.
pub(super) async fn restore_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<Json<AgentResponse>, ApiError> {
    let agent_id = AgentId(id);
    state.runtime_manager.agent_manager().restore(&user.security_context(), agent_id).await?;
    Ok(Json(readable(&state, &user, agent_id).await?))
}

/// Removes the deleted agent `id` for good, with its state - as the policy
/// allows, by default the system's alone.
pub(super) async fn purge_agent(State(state): State<Arc<AppState>>, Path(id): Path<Uuid>, user: AuthenticatedUser) -> Result<StatusCode, ApiError> {
    let agent_id = AgentId(id);
    let purged = state.runtime_manager.agent_manager().purge(&user.security_context(), agent_id).await?;
    match purged {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("agent_not_found", format!("There is no deleted agent {agent_id}"))),
    }
}

/// The personalities the user may read - what they may make an agent of -,
/// by name.
pub(super) async fn list_personalities(State(state): State<Arc<AppState>>, user: AuthenticatedUser) -> Result<Json<Vec<PersonalityResponse>>, ApiError> {
    let personalities = views(&state, &user).personalities().await?;
    Ok(Json(personalities.into_iter().map(PersonalityResponse::from).collect()))
}

/// Agent `id` as the list shows it to the user: what they may do with it
/// is the views' to say.
async fn readable(state: &Arc<AppState>, user: &AuthenticatedUser, id: AgentId) -> Result<AgentResponse, ApiError> {
    let agent = views(state, user).agent(id).await?;
    Ok(agent.snapshot().await?.into())
}

#[cfg(test)]
mod tests {
    use crate::routes::api;
    use crate::routes::tests::{login, state};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::header::{CONTENT_TYPE, COOKIE};
    use axum::http::{Method, Request, StatusCode};
    use caelumora_model::agent::{AgentId, Personality, PersonalityId};
    use caelumora_model::confidentiality::Visibility;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::value::name::ScopedName;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// `method` on `uri` as whoever `cookie` logs in, with `body` as JSON if
    /// given: the status and the JSON answered, `null` for none.
    async fn call(api: &Router, cookie: &str, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder().method(method).uri(uri).header(COOKIE, cookie);
        let request = match body {
            Some(body) => request.header(CONTENT_TYPE, "application/json").body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        };
        let response = api.clone().oneshot(request.unwrap()).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Issue 227: an agent is made of a personality the user may read, in
    /// their own scope, read with the channels it takes part in, and
    /// renamed; one of a session's scope names its session.
    #[tokio::test]
    async fn an_agent_is_made_of_a_personality_read_and_renamed() {
        let state = state();
        let plain = Personality {
            description: None,
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Scope::GlobalScope).unwrap(),
            kind: Default::default(),
            roles: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Visibility::Channel,
            hears_during_turn: Default::default(),
            deleted: None,
        };
        state.runtime_manager.entity_stores().personalities.create(&SecurityContext::system(), &plain).await.unwrap();
        let api = api(Arc::clone(&state), &[]);
        let alice = login(&api, "alice").await;

        let (status, personalities) = call(&api, &alice, Method::GET, "/personalities", None).await;
        assert_eq!((status, personalities[0]["name"].clone(), personalities[0]["scope"].clone()), (StatusCode::OK, json!("plain"), json!("global")));

        let (status, made) = call(&api, &alice, Method::POST, "/agents", Some(json!({"name": "helper", "personality_id": plain.id}))).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        assert_eq!((made["name"].clone(), made["scope"].clone(), made["personality"].clone(), made["can_change"].clone()), (json!("helper"), json!("user"), json!("plain"), json!(true)));
        assert!(made.get("channels").is_none(), "where it is made or listed, not its channels");
        let id = made["id"].as_str().unwrap().to_string();

        let (status, refused) = call(&api, &alice, Method::POST, "/agents", Some(json!({"name": "other", "personality_id": plain.id, "scope": "session"}))).await;
        assert_eq!((status, refused["code"].clone()), (StatusCode::BAD_REQUEST, json!("invalid_scope")));

        let (status, renamed) = call(&api, &alice, Method::PATCH, &format!("/agents/{id}"), Some(json!({"name": "assistant"}))).await;
        assert_eq!((status, renamed["name"].clone()), (StatusCode::OK, json!("assistant")));
        let (status, read) = call(&api, &alice, Method::GET, &format!("/agents/{id}"), None).await;
        assert_eq!((status, read["name"].clone(), read["channels"].clone()), (StatusCode::OK, json!("assistant"), json!([])));
        let (_, listed) = call(&api, &alice, Method::GET, "/agents", None).await;
        assert_eq!(listed[0]["name"], "assistant");

        let unknown = format!("/agents/{}", AgentId::new().0);
        let (status, missing) = call(&api, &alice, Method::GET, &unknown, None).await;
        assert_eq!((status, missing["code"].clone()), (StatusCode::NOT_FOUND, json!("agent_not_found")));
        let (status, missing) = call(&api, &alice, Method::PATCH, &unknown, Some(json!({"name": "x"}))).await;
        assert_eq!((status, missing["code"].clone()), (StatusCode::NOT_FOUND, json!("agent_not_found")));
    }
}
