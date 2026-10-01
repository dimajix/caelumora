//! Error responses of the REST API (issue 02).
//!
//! Every error is answered with an `application/problem+json` body as defined
//! by RFC 9457, extended by two members: `code`, a stable machine-readable
//! identifier (also the last segment of `type`), and `request_id`, which is
//! the `request_id` field of the request's log span and the `x-request-id`
//! response header - the way from a failure in the UI to its details in the
//! log.
//!
//! How much is disclosed: client errors (4xx) state their cause. Server errors
//! (5xx) state it only if the error is one written to be read by users (see
//! `caelumora_runtime::error::user_message`) - naming a component, an endpoint
//! or an entity, never file paths or secret values. Everything else gets a
//! generic `detail`; the full error chain goes to the log only.

use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

tokio::task_local! {
    static REQUEST_ID: Uuid;
}

/// Name of the response header carrying the request id.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Middleware giving each request an id: recorded on the request's log span,
/// available to `ApiError` while the request is handled, and returned in the
/// `x-request-id` header.
pub async fn request_id(request: Request, next: Next) -> Response {
    let id = Uuid::new_v4();
    tracing::Span::current().record("request_id", tracing::field::display(id));
    let mut response = REQUEST_ID.scope(id, next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

/// An error answered as a problem details document.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct ProblemDetails<'a> {
    #[serde(rename = "type")]
    problem_type: String,
    title: &'a str,
    status: u16,
    detail: &'a str,
    code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<Uuid>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        Self { status, code, detail: detail.into() }
    }

    pub fn not_found(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, detail)
    }

    pub fn unauthorized(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, detail)
    }

    pub fn conflict(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, detail)
    }

    /// The caller is logged in, but may not do this.
    pub fn forbidden(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, detail)
    }

    /// A server error caused by `error` while doing `action` (e.g. "loading
    /// the session"). Logs the full error chain; the response discloses only
    /// what `user_message` allows.
    pub fn internal(code: &'static str, action: &str, error: &anyhow::Error) -> Self {
        tracing::error!("{} failed: {:#}", action, error);
        let detail = match caelumora_runtime::error::user_message(error) {
            Some(message) => format!("{} failed: {}", capitalize(action), message),
            None => format!("{} failed; the details are in the server log", capitalize(action)),
        };
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, code, detail)
    }

    /// Like `internal`, but for a backend that is unavailable rather than
    /// broken.
    pub fn unavailable(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, detail)
    }

    /// The status it answers with - for tests; a handler returns the error.
    #[cfg(test)]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Its code - for tests, like `status`.
    #[cfg(test)]
    pub fn code(&self) -> &'static str {
        self.code
    }
}

/// What a request acted on: names the codes of its refusal and its failure
/// (`session_forbidden`, `command_failed`, ...) in [`ApiError::from_runtime`].
#[derive(Debug, Clone, Copy)]
pub enum Subject {
    Session,
    Participant,
    Cancel,
    Command,
    Context,
}

impl Subject {
    fn forbidden(self) -> &'static str {
        match self {
            Subject::Session => "session_forbidden",
            Subject::Participant => "participant_forbidden",
            Subject::Cancel => "cancel_forbidden",
            Subject::Command => "command_forbidden",
            Subject::Context => "context_forbidden",
        }
    }

    fn failed(self) -> &'static str {
        match self {
            Subject::Session => "session_failed",
            Subject::Participant => "participant_failed",
            Subject::Cancel => "cancel_failed",
            Subject::Command => "command_failed",
            Subject::Context => "context_failed",
        }
    }
}

impl ApiError {
    /// The answer for `error`, reported by the runtime while `action` (e.g.
    /// "renaming session …") on `subject` (issue 103) - the one place a
    /// domain error becomes a status and a code, so that a new variant has
    /// one place to go:
    ///
    /// - a request that cannot be met is the client's: `404` for a session,
    ///   template, channel, participant or context that is not there (or may
    ///   not be seen, which is answered the same), `409` for one that is
    ///   there already, `400` for any other `SessionError`, `ChannelError`,
    ///   `CommandError` or `TemplateError`;
    /// - a refusal (`is_forbidden`, or a refused context) is `403`,
    ///   `<subject>_forbidden`, saying what was refused and not why;
    /// - anything else is a server error, `<subject>_failed`.
    pub fn from_runtime(error: anyhow::Error, subject: Subject, action: &str) -> Self {
        use caelumora_runtime::channel::ChannelError;
        use caelumora_runtime::command::CommandError;
        use caelumora_runtime::error::{is_forbidden, is_not_found, user_message};
        use caelumora_runtime::session::SessionError;
        use caelumora_runtime::view::{ContextError, ViewError};
        use caelumora_model::template::TemplateError;

        let detail = error.to_string();
        let bad_request = |code| Self::new(StatusCode::BAD_REQUEST, code, detail.clone());
        if let Some(e) = error.downcast_ref::<ContextError>() {
            match e {
                ContextError::NotRecorded { .. } => return Self::not_found("context_not_recorded", detail),
                ContextError::Unavailable { .. } => return Self::not_found("context_unavailable", detail),
            }
        }
        if let Some(e) = error.downcast_ref::<ViewError>() {
            match e {
                ViewError::SessionNotFound(_) => return Self::not_found("session_not_found", detail),
                ViewError::ChannelNotFound { .. } | ViewError::NoChannelNamed(_) => return Self::not_found("channel_not_found", detail),
                ViewError::MemberNotFound { .. } => return Self::not_found("actor_not_found", detail),
                ViewError::NoContext { .. } => return Self::not_found("context_not_recorded", detail),
                ViewError::Forbidden { .. } => return Self::forbidden(Subject::Context.forbidden(), detail),
                ViewError::Failed(_) => {}
            }
        }
        if let Some(e) = error.downcast_ref::<SessionError>() {
            match e {
                SessionError::ChannelNotFound { .. } => return Self::not_found("channel_not_found", detail),
                SessionError::DuplicateChannelName { .. } => return Self::conflict("channel_exists", detail),
                SessionError::InvalidModerator(_) | SessionError::NotAParticipant { .. }
                | SessionError::ClassificationRefused(_) => return bad_request("invalid_channel"),
                // A caller that is no user: not the client's request.
                SessionError::OwnerNotAUser(_) => {}
            }
        }
        if let Some(e) = error.downcast_ref::<ChannelError>() {
            match e {
                ChannelError::AlreadyParticipant { .. } => return Self::conflict("already_participant", detail),
                ChannelError::NotAParticipant { .. } | ChannelError::ParticipantGone { .. } => return Self::not_found("participant_not_found", detail),
                ChannelError::NotInSession { .. } | ChannelError::ApproverStays { .. } => return bad_request("invalid_participant"),
                ChannelError::ClassificationRefused { .. } => return bad_request("invalid_classification"),
                ChannelError::MessageNotFound { .. } => return Self::not_found("message_not_found", detail),
                ChannelError::NotCleared { .. } => return Self::forbidden("not_cleared", detail),
                ChannelError::Forbidden { .. } => {}
            }
        }
        if let Some(e) = error.downcast_ref::<CommandError>() {
            match e {
                CommandError::Empty | CommandError::Unknown(_) => return bad_request("unknown_command"),
                CommandError::InvalidArguments { .. } => return bad_request("invalid_command_arguments"),
                _ => {}
            }
        }
        if error.downcast_ref::<TemplateError>().is_some() {
            return bad_request("template_invalid");
        }
        if is_not_found(&error, "session") {
            return Self::not_found("session_not_found", user_message(&error).unwrap_or(detail));
        }
        if is_not_found(&error, "session_template") {
            return Self::not_found("template_not_found", user_message(&error).unwrap_or(detail));
        }
        if is_forbidden(&error) {
            let detail = user_message(&error).unwrap_or_else(|| format!("{} is not permitted", capitalize(action)));
            return Self::forbidden(subject.forbidden(), detail);
        }
        Self::internal(subject.failed(), action, &error)
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ProblemDetails {
            problem_type: format!("urn:caelumora:problem:{}", self.code),
            title: self.status.canonical_reason().unwrap_or("Error"),
            status: self.status.as_u16(),
            detail: &self.detail,
            code: self.code,
            request_id: REQUEST_ID.try_with(|id| *id).ok(),
        };
        let json = serde_json::to_vec(&body).unwrap_or_default();
        let mut response = (self.status, json).into_response();
        response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
        response
    }
}

/// `axum::Json`, but rejecting malformed bodies with a problem details
/// document instead of plain text.
#[derive(axum::extract::FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path`, but rejecting malformed path parameters (e.g. an
/// invalid UUID) with a problem details document.
#[derive(axum::extract::FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
pub struct Path<T>(pub T);

impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(rejection: axum::extract::rejection::JsonRejection) -> Self {
        Self::new(rejection.status(), "invalid_request_body", rejection.body_text())
    }
}

impl From<axum::extract::rejection::PathRejection> for ApiError {
    fn from(rejection: axum::extract::rejection::PathRejection) -> Self {
        Self::new(rejection.status(), "invalid_path", rejection.body_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::store::StoreError;

    async fn body_of(error: ApiError) -> (StatusCode, String, serde_json::Value) {
        let response = REQUEST_ID.scope(Uuid::nil(), async { error.into_response() }).await;
        let content_type = response.headers()[CONTENT_TYPE].to_str().unwrap().to_string();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        (status, content_type, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn errors_are_problem_details_with_code_and_request_id() {
        let (status, content_type, json) = body_of(ApiError::not_found("session_not_found", "Session 42 does not exist")).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type, "application/problem+json");
        assert_eq!(json["type"], "urn:caelumora:problem:session_not_found");
        assert_eq!(json["title"], "Not Found");
        assert_eq!(json["status"], 404);
        assert_eq!(json["detail"], "Session 42 does not exist");
        assert_eq!(json["code"], "session_not_found");
        assert_eq!(json["request_id"], Uuid::nil().to_string());
    }

    #[tokio::test]
    async fn malformed_bodies_and_paths_are_problem_details() {
        use axum::body::Body;
        use axum::routing::post;
        use tower::ServiceExt;

        #[derive(serde::Deserialize)]
        struct Payload { #[allow(dead_code)] name: String }
        async fn handler(Path(_id): Path<Uuid>, Json(_p): Json<Payload>) -> StatusCode { StatusCode::OK }
        let app = axum::Router::new().route("/items/{id}", post(handler));

        let request = |uri: &str, body: &str| axum::http::Request::post(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.clone().oneshot(request("/items/not-a-uuid", r#"{"name":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");

        let response = app.oneshot(request(&format!("/items/{}", Uuid::nil()), r#"{"nome":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], "invalid_request_body");
        assert!(json["detail"].as_str().unwrap().contains("name"), "the detail says what is wrong: {}", json["detail"]);
    }

    #[tokio::test]
    async fn internal_errors_disclose_only_user_facing_messages() {
        let safe = anyhow::Error::new(StoreError::NotFound { entity: "personality", id: "p1".to_string() });
        let (_, _, json) = body_of(ApiError::internal("session_load_failed", "loading the session", &safe)).await;
        assert_eq!(json["detail"], "Loading the session failed: not found: personality p1");

        let unsafe_error = anyhow::Error::new(StoreError::Backend("/srv/data/sessions: permission denied".to_string()));
        let (status, _, json) = body_of(ApiError::internal("session_load_failed", "loading the session", &unsafe_error)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        let detail = json["detail"].as_str().unwrap();
        assert!(!detail.contains("/srv"), "internal paths must not be disclosed: {detail}");
        assert!(detail.contains("server log"));
    }

    #[test]
    fn runtime_errors_map_to_status_and_code_in_one_place() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::session::{ChannelId, SessionId};
        use caelumora_runtime::channel::ChannelError;
        use caelumora_runtime::session::SessionError;
        let (session, channel, someone) = (SessionId::new(), ChannelId::new(), ActorId::None);
        let answer = |error: anyhow::Error, subject| {
            let e = ApiError::from_runtime(error, subject, "doing it");
            (e.status().as_u16(), e.code())
        };
        let not_found = |entity| anyhow::Error::new(StoreError::NotFound { entity, id: "1".to_string() });

        assert_eq!(answer(not_found("session"), Subject::Session), (404, "session_not_found"));
        assert_eq!(answer(not_found("session_template"), Subject::Session), (404, "template_not_found"));
        assert_eq!(answer(anyhow::Error::new(SessionError::ChannelNotFound { session, channel }), Subject::Session), (404, "channel_not_found"));
        assert_eq!(answer(anyhow::Error::new(SessionError::DuplicateChannelName { session, name: caelumora_model::name::Name::new("x").unwrap() }), Subject::Session), (409, "channel_exists"));
        assert_eq!(answer(anyhow::Error::new(ChannelError::AlreadyParticipant { participant: someone, channel }), Subject::Participant), (409, "already_participant"));
        assert_eq!(answer(anyhow::Error::new(ChannelError::ParticipantGone { participant: someone, channel }), Subject::Participant), (404, "participant_not_found"));
        assert_eq!(answer(anyhow::Error::new(ChannelError::ApproverStays { channel }), Subject::Participant), (400, "invalid_participant"));
        let classification = caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT;
        assert_eq!(answer(anyhow::Error::new(ChannelError::ClassificationRefused { classification, channel }), Subject::Session), (400, "invalid_classification"));
        let message = caelumora_model::message::MessageId::new();
        assert_eq!(answer(anyhow::Error::new(ChannelError::MessageNotFound { message, channel }), Subject::Session), (404, "message_not_found"));
        assert_eq!(answer(anyhow::Error::new(ChannelError::NotCleared { participant: someone, channel }), Subject::Participant), (403, "not_cleared"));
        assert_eq!(answer(anyhow::Error::new(caelumora_model::template::TemplateError::UnsupportedScope("group")), Subject::Session), (400, "template_invalid"));

        // A refusal is named after what was refused, whichever layer refused it.
        let refused = || anyhow::Error::new(ChannelError::Forbidden { action: "adding a participant to", channel });
        assert_eq!(answer(refused(), Subject::Participant), (403, "participant_forbidden"));
        assert_eq!(answer(anyhow::Error::new(StoreError::PermissionDenied), Subject::Participant), (403, "participant_forbidden"));
        assert_eq!(answer(refused(), Subject::Cancel), (403, "cancel_forbidden"));
        assert_eq!(answer(anyhow::Error::new(StoreError::PermissionDenied).context("renaming"), Subject::Session), (403, "session_forbidden"));

        // Anything else is the server's, and named after the subject too.
        assert_eq!(answer(anyhow::anyhow!("broken"), Subject::Session), (500, "session_failed"));
        assert_eq!(answer(not_found("personality"), Subject::Session), (500, "session_failed"), "a missing personality is no missing session");
    }
}
