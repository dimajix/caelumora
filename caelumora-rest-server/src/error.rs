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

    pub fn status(&self) -> StatusCode {
        self.status
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
}
