//! Error responses of the REST API (issue 02).
//!
//! Every error is answered with an `application/problem+json` body as defined
//! by RFC 9457, extended by two participants: `code`, a stable machine-readable
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

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use caelumora_model::confidentiality::ClassificationError;
use caelumora_model::question::QuestionError;
use caelumora_model::security::Refused;
use caelumora_model::store::StoreError;
use caelumora_model::template::TemplateError;
use caelumora_model::value::ValueError;
use caelumora_runtime::agent::AgentError;
use caelumora_runtime::attachment::AttachmentError;
use caelumora_runtime::backend::StorageGone;
use caelumora_runtime::content::ContentError;
use caelumora_runtime::channel::ChannelError;
use caelumora_runtime::command::CommandError;
use caelumora_runtime::error::{refusal, user_message};
use caelumora_runtime::group::GroupError;
use caelumora_runtime::session::SessionError;
use caelumora_runtime::terminal::TerminalError;
use caelumora_runtime::view::{AgentContextError, ViewError};
use caelumora_runtime::workspace::WorkspaceError;
use serde::Serialize;
use std::borrow::Cow;
use uuid::Uuid;
use anyhow::Error;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use tracing::Span;
use axum::extract;
use tracing::field;

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
    Span::current().record("request_id", field::display(id));
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
    code: Cow<'static, str>,
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
    pub fn new(status: StatusCode, code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self { status, code: code.into(), detail: detail.into() }
    }

    pub fn not_found(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, detail)
    }

    pub fn unauthorized(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, detail)
    }

    pub fn conflict(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, detail)
    }

    /// A text of the request that does not fit the field it was to be
    /// written into (issue 248): `400 invalid_value`, saying which field and
    /// why.
    pub fn invalid_value(error: &ValueError) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_value", error.to_string())
    }

    /// The caller is logged in, but may not do this.
    pub fn forbidden(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, detail)
    }

    /// A server error caused by `error` while doing `action` (e.g. "loading
    /// the session"). Logs the full error chain; the response discloses only
    /// what `user_message` allows.
    pub fn internal(code: impl Into<Cow<'static, str>>, action: &str, error: &Error) -> Self {
        tracing::error!("{} failed: {:#}", action, error);
        let detail = match user_message(error) {
            Some(message) => format!("{} failed: {}", capitalize(action), message),
            None => format!("{} failed; the details are in the server log", capitalize(action)),
        };
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, code, detail)
    }

    /// Like `internal`, but for a backend that is unavailable rather than
    /// broken.
    pub fn unavailable(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, detail)
    }

    /// The status it answers with - for tests; a handler returns the error.
    #[cfg(test)]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Its code - for tests, like `status`.
    #[cfg(test)]
    pub fn code(&self) -> &str {
        &self.code
    }
}

// Every error a handler meets answers itself (issue 323): what was refused,
// what was not found, what conflicts, by its type and its variant - not by
// what the handler says it was doing. An error type that reaches a handler
// implements `From<…> for ApiError` here, or, for a plugin's, in its crate.

impl From<Refused> for ApiError {
    /// `403 <subject>_forbidden`, saying what was refused and not why.
    fn from(refused: Refused) -> Self {
        Self::refused(&refused, refused.to_string())
    }
}

impl ApiError {
    /// `refused`, told as `detail` - the words of the error that carries it.
    fn refused(refused: &Refused, detail: String) -> Self {
        Self::new(StatusCode::FORBIDDEN, format!("{}_forbidden", refused.subject), detail)
    }

    pub fn bad_request(code: impl Into<Cow<'static, str>>, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, detail)
    }
}

impl From<ValueError> for ApiError {
    fn from(error: ValueError) -> Self {
        Self::invalid_value(&error)
    }
}

impl From<ClassificationError> for ApiError {
    /// A classification the request gives that is none (issue 282).
    fn from(error: ClassificationError) -> Self {
        Self::bad_request("invalid_classification", error.to_string())
    }
}

impl From<StoreError> for ApiError {
    /// A store's error as an operation passed it on: not found is what the
    /// request named, unlike one met on the way (`From<anyhow::Error>`).
    fn from(error: StoreError) -> Self {
        let detail = error.to_string();
        match error {
            StoreError::NotFound { entity, .. } => Self::not_found(format!("{entity}_not_found"), detail),
            error => Self::of_store(error),
        }
    }
}

impl ApiError {
    /// A store's error other than not found, wherever it was met.
    fn of_store(error: StoreError) -> Self {
        let detail = error.to_string();
        match error {
            StoreError::PermissionDenied(refused) => refused.into(),
            StoreError::NameTaken { .. } => Self::conflict("name_taken", detail),
            StoreError::Conflict(_) => Self::conflict("already_exists", detail),
            StoreError::Validation(_) => Self::bad_request("validation_failed", detail),
            StoreError::Invalid(e) => e.into(),
            StoreError::NotFound { .. } | StoreError::Backend(_) => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

impl From<TemplateError> for ApiError {
    fn from(error: TemplateError) -> Self {
        Self::bad_request("template_invalid", error.to_string())
    }
}

impl From<AgentContextError> for ApiError {
    fn from(error: AgentContextError) -> Self {
        let detail = error.to_string();
        match error {
            AgentContextError::NotRecorded { .. } => Self::not_found("context_not_recorded", detail),
            AgentContextError::Unavailable { .. } => Self::not_found("context_unavailable", detail),
            failed @ AgentContextError::Failed { .. } => Self::internal("context_failed", "reading what an agent saw", &Error::new(failed)),
            unworded @ AgentContextError::Unworded { .. } => Self::internal("context_failed", "wording what an agent saw", &Error::new(unworded)),
        }
    }
}

impl From<ViewError> for ApiError {
    fn from(error: ViewError) -> Self {
        let detail = error.to_string();
        match error {
            ViewError::SessionNotFound(_) => Self::not_found("session_not_found", detail),
            ViewError::ChannelNotFound { .. } | ViewError::NoChannelNamed(_) => Self::not_found("channel_not_found", detail),
            ViewError::ParticipantNotFound { .. } => Self::not_found("actor_not_found", detail),
            ViewError::MessageNotFound { .. } => Self::not_found("message_not_found", detail),
            ViewError::WorkspaceNotFound(_) => Self::not_found("workspace_not_found", detail),
            ViewError::GroupNotFound(_) => Self::not_found("group_not_found", detail),
            ViewError::AgentNotFound(_) => Self::not_found("agent_not_found", detail),
            ViewError::NoContext { .. } => Self::not_found("context_not_recorded", detail),
            ViewError::Forbidden { ref refused, .. } | ViewError::NotPermitted(ref refused) => Self::refused(refused, detail),
            ViewError::Attachment(e) => e.into(),
            ViewError::Failed(e) => e.into(),
        }
    }
}

impl From<WorkspaceError> for ApiError {
    fn from(error: WorkspaceError) -> Self {
        let detail = error.to_string();
        match error {
            WorkspaceError::NotFound(_) | WorkspaceError::NotUsableInChannel | WorkspaceError::Store(StoreError::NotFound { .. }) => Self::not_found("workspace_not_found", detail),
            WorkspaceError::UnknownStorage(_) => Self::bad_request("unknown_storage", detail),
            WorkspaceError::StorageGone(e) => e.into(),
            WorkspaceError::StorageNotCleared(_) => Self::bad_request("storage_not_cleared", detail),
            WorkspaceError::ClassificationRefused => Self::bad_request("invalid_classification", detail),
            WorkspaceError::NotCleared => Self::forbidden("not_cleared", detail),
            WorkspaceError::ReadOnly { .. } => Self::forbidden("read_only", detail),
            WorkspaceError::Invalid(e) => e.into(),
            WorkspaceError::Terminal(e) => e.into(),
            // A scope it does not lie in, an access list that names whom it
            // may not (issue 286): the client's request.
            WorkspaceError::Store(e) => Self::of_store(e),
            // A caller that is no user: not the client's request.
            WorkspaceError::OwnerNotAUser => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

impl From<AttachmentError> for ApiError {
    /// What uploading or reading an attachment failed with (issue 282): what does not fit is the
    /// client's, a storage that failed or is no longer configured is unavailable.
    fn from(error: AttachmentError) -> Self {
        let detail = error.to_string();
        match error {
            AttachmentError::NotConfigured => Self::bad_request("attachments_not_configured", detail),
            AttachmentError::ClassificationRefused => Self::bad_request("invalid_classification", detail),
            AttachmentError::StorageNotCleared(_) => Self::bad_request("storage_not_cleared", detail),
            AttachmentError::NotAParticipant(_) => Self::forbidden("not_a_participant", detail),
            AttachmentError::NotFound(_) => Self::not_found("attachment_not_found", detail),
            // What a storage holds is not what was kept: the server's, not the request's.
            altered @ AttachmentError::Altered(_) => Self::internal("attachment_altered", "reading an attachment", &Error::new(altered)),
            AttachmentError::Invalid(e) => e.into(),
            AttachmentError::Content(e) => e.into(),
            AttachmentError::StorageGone(e) => e.into(),
        }
    }
}

impl From<StorageGone> for ApiError {
    /// A storage that what is kept names, no longer configured (issue 466), whatever is kept there:
    /// unavailable, not the request's; its name is the configuration's, for the log alone.
    fn from(error: StorageGone) -> Self {
        tracing::error!("{error}");
        Self::unavailable("storage_unavailable", "The storage of what was asked for is unavailable; the details are in the server log")
    }
}

impl From<ContentError> for ApiError {
    /// What a storage of content reported (issue 466), whatever it keeps the content for: content
    /// that does not fit is the client's; a storage that failed is unavailable; a key it does not
    /// know is one its owner made, never what the request named - the owner answers that as its
    /// own `…NotFound` -, and so a server error.
    fn from(error: ContentError) -> Self {
        let detail = error.to_string();
        match error {
            ContentError::TooLarge { .. } => Self::new(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large", detail),
            ContentError::Changed => Self::bad_request("changed", detail),
            ContentError::Unavailable(_) => Self::unavailable("storage_unavailable", detail),
            ContentError::InvalidKey(_) | ContentError::NotFound(_) => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

impl From<AgentError> for ApiError {
    /// A store's error as an operation passed it on, as `From<StoreError>`:
    /// what the request named - also the personality an agent is made of.
    fn from(error: AgentError) -> Self {
        let detail = error.to_string();
        match error {
            AgentError::NotFound(_) => Self::not_found("agent_not_found", detail),
            AgentError::PersonalityOutOfScope(_) => Self::bad_request("personality_out_of_scope", detail),
            AgentError::Invalid(e) => e.into(),
            AgentError::Store(e) => e.into(),
        }
    }
}

impl From<GroupError> for ApiError {
    fn from(error: GroupError) -> Self {
        let detail = error.to_string();
        match error {
            GroupError::NotFound(_) | GroupError::Store(StoreError::NotFound { .. }) => Self::not_found("group_not_found", detail),
            GroupError::Participants(_) => Self::conflict("group_of_participants", detail),
            GroupError::UnknownMember(_) => Self::bad_request("unknown_member", detail),
            GroupError::Invalid(e) => e.into(),
            GroupError::Store(StoreError::Validation(_)) => Self::bad_request("invalid_scope", detail),
            GroupError::Store(e) => Self::of_store(e),
            // A caller that is no user: not the client's request.
            GroupError::OwnerNotAUser => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

impl From<TerminalError> for ApiError {
    /// What working on the files of a workspace failed with (issue 253): a
    /// path or a file that does not fit is the client's; a storage that
    /// failed is unavailable. Every message names no more of a path than
    /// the client wrote.
    fn from(error: TerminalError) -> Self {
        let detail = error.to_string();
        match error {
            TerminalError::InvalidPath(_) => Self::bad_request("invalid_path", detail),
            TerminalError::NotFound(_) => Self::not_found("file_not_found", detail),
            TerminalError::Exists(_) => Self::conflict("file_exists", detail),
            TerminalError::IsADirectory(_) => Self::conflict("is_a_directory", detail),
            TerminalError::NotADirectory(_) => Self::conflict("not_a_directory", detail),
            TerminalError::NotEmpty(_) => Self::conflict("directory_not_empty", detail),
            TerminalError::TooLarge { .. } => Self::new(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large", detail),
            TerminalError::Unavailable(_) => Self::unavailable("storage_unavailable", detail),
            TerminalError::UnknownCommand(_) => Self::not_found("command_not_found", detail),
        }
    }
}

impl From<SessionError> for ApiError {
    fn from(error: SessionError) -> Self {
        let detail = error.to_string();
        match error {
            SessionError::NotFound(_) => Self::not_found("session_not_found", detail),
            SessionError::TemplateNotFound(_) => Self::not_found("template_not_found", detail),
            SessionError::ChannelNotFound { .. } => Self::not_found("channel_not_found", detail),
            // Its components cannot be read until their owner shares them again, or the channel is
            // deleted (issue 463).
            SessionError::ChannelUnavailable { .. } => Self::conflict("channel_unavailable", detail),
            // The request's fault where the runtime may show why; a store that failed while a
            // bundle was looked for is the server's, and withheld (issue 457).
            SessionError::InvalidComponents(_) => {
                let error = Error::new(error);
                match user_message(&error) {
                    Some(_) => Self::bad_request("invalid_channel", detail),
                    None => Self::internal("internal_error", "building the channel's components", &error),
                }
            }
            SessionError::NotAParticipant { .. } | SessionError::ClassificationRefused(_) => Self::bad_request("invalid_channel", detail),
            SessionError::WorkspaceOutOfScope(_) => Self::bad_request("workspace_out_of_scope", detail),
            SessionError::ScopeNotEmpty { .. } => Self::conflict("scope_not_empty", detail),
            SessionError::ShuttingDown => Self::unavailable("shutting_down", detail),
            SessionError::AlreadyParticipant { .. } => Self::conflict("already_participant", detail),
            SessionError::CannotTakeIn(_) => Self::bad_request("invalid_participant", detail),
            // A caller that is no user: not the client's request.
            SessionError::OwnerNotAUser(_) => Self::internal("internal_error", "the request", &error.into()),
        }
    }
}

impl From<ChannelError> for ApiError {
    fn from(error: ChannelError) -> Self {
        let detail = error.to_string();
        match error {
            ChannelError::Forbidden { ref refused, .. } => Self::refused(refused, detail),
            ChannelError::AlreadyParticipant { .. } => Self::conflict("already_participant", detail),
            ChannelError::StillLeaving { .. } => Self::conflict("still_leaving", detail),
            ChannelError::AlreadyResponding { .. } => Self::conflict("already_responding", detail),
            ChannelError::NudgesItself { .. } => Self::bad_request("nudges_itself", detail),
            ChannelError::NotAParticipant { .. } | ChannelError::ParticipantGone { .. } => Self::not_found("participant_not_found", detail),
            ChannelError::NotInSession { .. } | ChannelError::OutOfScope { .. } | ChannelError::UnknownAgentKind { .. } => Self::bad_request("invalid_participant", detail),
            ChannelError::ClassificationRefused { .. } => Self::bad_request("invalid_classification", detail),
            ChannelError::MessageNotFound { .. } => Self::not_found("message_not_found", detail),
            ChannelError::MessageRevoked { .. } => Self::conflict("message_revoked", detail),
            ChannelError::NotAnAgentsMessage { .. } => Self::conflict("not_an_agents_message", detail),
            ChannelError::TurnNotRetryable { .. } => Self::conflict("turn_not_retryable", detail),
            ChannelError::ToolCallNotRunning { .. } => Self::conflict("tool_call_not_running", detail),
            ChannelError::NotCleared { .. } => Self::forbidden("not_cleared", detail),
            ChannelError::Question { reason: QuestionError::NotOpen, .. } => Self::not_found("question_not_found", detail),
            ChannelError::Question { reason: QuestionError::NotAsked, .. } => Self::forbidden("not_asked", detail),
            ChannelError::Question { .. } => Self::bad_request("invalid_answer", detail),
        }
    }
}

impl From<CommandError> for ApiError {
    fn from(error: CommandError) -> Self {
        let detail = error.to_string();
        match error {
            CommandError::Empty | CommandError::Unknown(_) => Self::bad_request("unknown_command", detail),
            CommandError::InvalidArguments { .. } => Self::bad_request("invalid_command_arguments", detail),
            CommandError::Forbidden { ref refused, .. } => Self::refused(refused, detail),
            // Its reason is written to be read by users.
            CommandError::Failed { .. } => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "command_failed", detail),
        }
    }
}

impl From<Error> for ApiError {
    /// What an operation that answers `anyhow` failed with: an invalid
    /// value wherever in the chain it is; otherwise the outermost error of
    /// a type the core knows, as it answers itself; otherwise a name taken
    /// or a refusal wherever in the chain they are - also in an error of a
    /// type unknown here, such as a plugin's. A `StoreError::NotFound` met on
    /// the way - a personality missing while a session is loaded - is no
    /// answer to what the request named, and so a server error; whatever
    /// else is not known is one too.
    fn from(error: Error) -> Self {
        if let Some(e) = error.chain().find_map(|e| e.downcast_ref::<ValueError>()) {
            return Self::invalid_value(e);
        }
        macro_rules! answer_as {
            ($error:ident, $($kind:ty),*) => {$(
                let $error = match $error.downcast::<$kind>() {
                    Ok(e) => return e.into(),
                    Err(error) => error,
                };
            )*};
        }
        answer_as!(error, AgentContextError, ViewError, WorkspaceError, AttachmentError, AgentError, GroupError, TerminalError, SessionError, ChannelError, CommandError, TemplateError);
        if error.chain().any(|e| matches!(e.downcast_ref(), Some(StoreError::NameTaken { .. }))) {
            return Self::conflict("name_taken", user_message(&error).unwrap_or_else(|| error.to_string()));
        }
        if let Some(refused) = refusal(&error) {
            return Self::refused(refused, user_message(&error).unwrap_or_else(|| refused.to_string()));
        }
        match error.downcast::<StoreError>() {
            Ok(e @ StoreError::NotFound { .. }) => Self::internal("internal_error", "the request", &e.into()),
            Ok(e) => Self::of_store(e),
            Err(error) => Self::internal("internal_error", "the request", &error),
        }
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
            code: &self.code,
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
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
pub struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path`, but rejecting malformed path parameters (e.g. an
/// invalid UUID) with a problem details document.
#[derive(FromRequestParts)]
#[from_request(via(extract::Path), rejection(ApiError))]
pub struct Path<T>(pub T);

/// `axum::extract::Query`, but rejecting a malformed query string (e.g.
/// `?limit=abc`) with a problem details document (issue 384).
#[derive(FromRequestParts)]
#[from_request(via(extract::Query), rejection(ApiError))]
pub struct Query<T>(pub T);

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::new(rejection.status(), "invalid_request_body", rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::new(rejection.status(), "invalid_path", rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::new(rejection.status(), "invalid_query", rejection.body_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::ActorId;
    use serde::Deserialize;
    use caelumora_model::agent::Agent;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::content::ContentId;
    use caelumora_model::message::MessageId;
    use caelumora_model::question::QuestionId;
    use caelumora_model::store::Stored;
    use caelumora_model::scope::Scope;
    use caelumora_model::session::{ChannelId, SessionId};
    use caelumora_model::template::SessionTemplateId;
    use caelumora_model::user::UserId;
    use caelumora_model::work::TurnId;
    use caelumora_model::workspace::WorkspaceId;
    use caelumora_runtime::component::ComponentError;
    use axum::Router;
    use axum::http::Request;
    use serde_json::Value;
    use std::fmt;
    use axum::body;
    use std::error;
    use std::sync::Arc;

    async fn body_of(error: ApiError) -> (StatusCode, String, Value) {
        let response = REQUEST_ID.scope(Uuid::nil(), async { error.into_response() }).await;
        let content_type = response.headers()[CONTENT_TYPE].to_str().unwrap().to_string();
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), 4096).await.unwrap();
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

        #[derive(Deserialize)]
        struct Payload { #[expect(dead_code, reason = "only its deserializing is tested")] name: String }
        async fn handler(Path(_id): Path<Uuid>, Json(_p): Json<Payload>) -> StatusCode { StatusCode::OK }
        let app = Router::new().route("/items/{id}", post(handler));

        let request = |uri: &str, body: &str| Request::post(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();

        let response = app.clone().oneshot(request("/items/not-a-uuid", r#"{"name":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");

        let response = app.oneshot(request(&format!("/items/{}", Uuid::nil()), r#"{"nome":"x"}"#)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["code"], "invalid_request_body");
        assert!(json["detail"].as_str().unwrap().contains("name"), "the detail says what is wrong: {}", json["detail"]);
    }

    #[tokio::test]
    async fn internal_errors_disclose_only_user_facing_messages() {
        let safe = Error::new(StoreError::NotFound { entity: "personality", id: "p1".to_string() });
        let (_, _, json) = body_of(ApiError::internal("session_load_failed", "loading the session", &safe)).await;
        assert_eq!(json["detail"], "Loading the session failed: not found: personality p1");

        let unsafe_error = Error::new(StoreError::Backend("/srv/data/sessions: permission denied".to_string()));
        let (status, _, json) = body_of(ApiError::internal("session_load_failed", "loading the session", &unsafe_error)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        let detail = json["detail"].as_str().unwrap();
        assert!(!detail.contains("/srv"), "internal paths must not be disclosed: {detail}");
        assert!(detail.contains("server log"));
    }

    fn answer(error: impl Into<ApiError>) -> (u16, String) {
        let e = error.into();
        (e.status.as_u16(), e.code.into_owned())
    }

    fn refused(subject: &'static str) -> Refused {
        Refused { subject, action: "Something".to_string() }
    }

    #[test]
    fn a_refusal_is_named_by_what_was_refused_wherever_it_is_found() {
        let channel = ChannelId::new();
        assert_eq!(answer(refused("skill")), (403, "skill_forbidden".to_string()));
        assert_eq!(answer(StoreError::PermissionDenied(refused("session"))), (403, "session_forbidden".to_string()));
        let channel_refused = ChannelError::Forbidden { action: "adding a participant to", channel, refused: refused("channel") };
        let e = ApiError::from(channel_refused);
        assert_eq!((e.status.as_u16(), e.code.as_ref()), (403, "channel_forbidden"));
        assert!(e.detail.starts_with("adding a participant to channel"), "in the runtime's words: {}", e.detail);
        assert_eq!(answer(ViewError::Forbidden { actor: ActorId::None, refused: refused("context") }), (403, "context_forbidden".to_string()), "as the web UI reads it");
        assert_eq!(answer(ViewError::Forbidden { actor: ActorId::None, refused: refused("agent") }), (403, "agent_forbidden".to_string()), "as the web UI reads it");
        // A plugin's error, unknown here, inside the core's `anyhow`, below
        // a context: found by the refusal it carries.
        #[derive(Debug)]
        struct PluginError(Refused);
        impl fmt::Display for PluginError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("not yours")
            }
        }
        impl error::Error for PluginError {
            fn source(&self) -> Option<&(dyn error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let inside = Error::new(PluginError(refused("skill"))).context("deleting what lies in the session's scope");
        assert_eq!(answer(inside), (403, "skill_forbidden".to_string()));
        assert_eq!(answer(Error::new(StoreError::PermissionDenied(refused("session"))).context("renaming")), (403, "session_forbidden".to_string()));
    }

    #[test]
    fn what_a_request_named_and_was_not_found_is_told_by_its_kind() {
        assert_eq!(answer(SessionError::NotFound(SessionId::new())), (404, "session_not_found".to_string()));
        assert_eq!(answer(Error::new(SessionError::TemplateNotFound(SessionTemplateId::new()))), (404, "template_not_found".to_string()));
        assert_eq!(answer(StoreError::NotFound { entity: "agent", id: "1".to_string() }), (404, "agent_not_found".to_string()), "as an operation passed it on");
        let personality = Error::new(StoreError::NotFound { entity: "personality", id: "p1".to_string() });
        assert_eq!(answer(personality), (500, "internal_error".to_string()), "met on the way, no answer to what was asked");
        assert_eq!(answer(anyhow::anyhow!("broken")), (500, "internal_error".to_string()));
    }

    #[test]
    fn the_core_s_errors_answer_with_status_and_code() {
        let (session, channel, someone) = (SessionId::new(), ChannelId::new(), ActorId::None);
        assert_eq!(answer(SessionError::ChannelNotFound { session, channel }), (404, "channel_not_found".to_string()));
        let gone = ComponentError::Bundle { name: "shared".to_string(), cause: StoreError::NotFound { entity: "component_bundle", id: "shared".to_string() } };
        assert_eq!(answer(SessionError::ChannelUnavailable { session, channel, cause: Arc::new(gone) }), (409, "channel_unavailable".to_string()));
        assert_eq!(answer(Error::new(StoreError::NameTaken { entity: "agent" }).context("renaming")), (409, "name_taken".to_string()), "of every kind, also below a context");
        let channel_error = |e: ChannelError| answer(e).1;
        assert_eq!(channel_error(ChannelError::AlreadyParticipant { participant: someone, channel }), "already_participant");
        assert_eq!(channel_error(ChannelError::StillLeaving { participant: someone, channel }), "still_leaving");
        assert_eq!(channel_error(ChannelError::ParticipantGone { participant: someone, channel }), "participant_not_found");
        assert_eq!(channel_error(ChannelError::OutOfScope { participant: someone, channel }), "invalid_participant");
        assert_eq!(channel_error(ChannelError::UnknownAgentKind { participant: someone, channel }), "invalid_participant");
        assert_eq!(channel_error(ChannelError::ClassificationRefused { classification: Classification::CHANNEL_DEFAULT, channel }), "invalid_classification");
        let message = MessageId::new();
        assert_eq!(channel_error(ChannelError::MessageNotFound { message, channel }), "message_not_found");
        assert_eq!(channel_error(ChannelError::MessageRevoked { message, channel }), "message_revoked");
        assert_eq!(channel_error(ChannelError::NotAnAgentsMessage { message, channel }), "not_an_agents_message");
        assert_eq!(channel_error(ChannelError::TurnNotRetryable { turn: TurnId::new(), channel }), "turn_not_retryable");
        assert_eq!(answer(ChannelError::ToolCallNotRunning { call: "1".to_string(), turn: TurnId::new(), channel }), (409, "tool_call_not_running".to_string()));
        assert_eq!(answer(ChannelError::NotCleared { participant: someone, channel }), (403, "not_cleared".to_string()));
        let question = |reason| answer(ChannelError::Question { question: QuestionId::new(), channel, reason });
        assert_eq!(question(QuestionError::NotOpen), (404, "question_not_found".to_string()));
        assert_eq!(question(QuestionError::NotAsked), (403, "not_asked".to_string()));
        assert_eq!(question(QuestionError::ResponseCount(2)), (400, "invalid_answer".to_string()));
        assert_eq!(answer(TemplateError::UnsupportedScope("group")), (400, "template_invalid".to_string()));
        assert_eq!(answer(SessionError::WorkspaceOutOfScope(WorkspaceId::new())), (400, "workspace_out_of_scope".to_string()));
        assert_eq!(answer(SessionError::AlreadyParticipant { session, participant: ActorId::User(UserId::new()) }), (409, "already_participant".to_string()));
        assert_eq!(answer(SessionError::CannotTakeIn(ActorId::System)), (400, "invalid_participant".to_string()));
        assert_eq!(answer(SessionError::ShuttingDown), (503, "shutting_down".to_string()), "for now, not for good (issue 297)");
        let left = vec![(Agent::KIND, 1)];
        assert_eq!(answer(SessionError::ScopeNotEmpty { holder: Scope::SessionScope { session_id: session }, left }), (409, "scope_not_empty".to_string()), "what is left is the others' to take away (issue 320)");
    }

    #[test]
    fn a_text_that_does_not_fit_its_field_is_invalid_wherever_it_was_found_out() {
        let blank = || ValueError::Blank { field: "summary" };
        assert_eq!(answer(blank()), (400, "invalid_value".to_string()));
        assert_eq!(answer(Error::new(StoreError::from(blank()))), (400, "invalid_value".to_string()), "by a controller, too");
        assert_eq!(ApiError::from(Error::new(StoreError::from(blank()))).detail, "the summary must not consist of whitespace alone", "naming the field");
    }

    /// Components that cannot be built are the request's fault, told as the
    /// runtime tells them - unless a store failed while a bundle was looked
    /// for, which is the server's and not shown (issue 457).
    #[test]
    fn invalid_components_are_the_requests_fault_unless_a_store_failed() {
        let bundle = |cause| SessionError::InvalidComponents(ComponentError::Build {
            component: "tools".to_string(),
            kind: "ref".to_string(),
            cause: Box::new(ComponentError::Bundle { name: "tools".to_string(), cause }),
        });
        assert_eq!(answer(bundle(StoreError::NotFound { entity: "component_bundle", id: "tools".to_string() })), (400, "invalid_channel".to_string()));
        let failed = ApiError::from(bundle(StoreError::Backend("/srv/data/component_bundle: permission denied".to_string())));
        assert_eq!((failed.status.as_u16(), failed.code.as_ref()), (500, "internal_error"));
        assert!(!failed.detail.contains("/srv"), "{}", failed.detail);
    }

    #[test]
    fn attachments_answer_with_status_and_code() {
        assert_eq!(answer(AttachmentError::NotConfigured), (400, "attachments_not_configured".to_string()));
        assert_eq!(answer(AttachmentError::ClassificationRefused), (400, "invalid_classification".to_string()));
        assert_eq!(answer(AttachmentError::NotAParticipant(ActorId::User(UserId::new()))), (403, "not_a_participant".to_string()));
        assert_eq!(answer(Error::new(AttachmentError::NotFound(ContentId::new()))), (404, "attachment_not_found".to_string()), "also as a user's actor fails with it");
        assert_eq!(answer(AttachmentError::Altered(ContentId::new())), (500, "attachment_altered".to_string()));
        let gone = ApiError::from(ViewError::Attachment(AttachmentError::StorageGone(StorageGone("old".to_string()))));
        assert_eq!((gone.status, gone.code.as_ref()), (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"), "named by what is kept, downloaded through a view");
        assert!(!gone.detail.contains("old"), "for the log alone: {}", gone.detail);
        assert_eq!(answer(ClassificationError::CompartmentsWithoutLevel), (400, "invalid_classification".to_string()));
    }

    /// A storage fails the same way whatever it keeps (issue 466): an attachment's storage of
    /// content is answered as a workspace's storage.
    #[test]
    fn a_storages_failure_is_answered_alike_whatever_it_keeps() {
        let content = |error| answer(AttachmentError::Content(error));
        let files = |error| answer(WorkspaceError::Terminal(error));
        assert_eq!(content(ContentError::TooLarge { limit: 1 }), (413, "file_too_large".to_string()));
        assert_eq!(content(ContentError::TooLarge { limit: 1 }), files(TerminalError::TooLarge { limit: 1 }));
        assert_eq!(content(ContentError::Unavailable("permission denied".to_string())), (503, "storage_unavailable".to_string()));
        assert_eq!(content(ContentError::Unavailable("permission denied".to_string())), files(TerminalError::Unavailable("permission denied".to_string())));
        assert_eq!(content(ContentError::Changed), (400, "changed".to_string()), "the request's bytes were not what it said");
        assert_eq!(content(ContentError::NotFound("attachments/a".to_string())), (500, "internal_error".to_string()), "a key its owner made");
        let failed = ApiError::from(ViewError::Attachment(AttachmentError::Content(ContentError::Unavailable("permission denied".to_string()))));
        assert_eq!(failed.detail, "the storage of content failed: permission denied", "what happened, as a workspace's storage tells it");
    }

    #[test]
    fn workspaces_and_their_files_answer_with_status_and_code() {
        let id = WorkspaceId::new();
        assert_eq!(answer(WorkspaceError::NotFound(id)), (404, "workspace_not_found".to_string()));
        assert_eq!(answer(WorkspaceError::UnknownStorage("nfs".to_string())), (400, "unknown_storage".to_string()), "named by the request");
        let gone = ApiError::from(WorkspaceError::StorageGone(StorageGone("nfs".to_string())));
        assert_eq!((gone.status, gone.code.as_ref()), (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable"), "named by the workspace, as an attachment's storage");
        assert!(!gone.detail.contains("nfs"), "for the log alone: {}", gone.detail);
        assert_eq!(answer(WorkspaceError::NotCleared), (403, "not_cleared".to_string()));
        assert_eq!(answer(WorkspaceError::Store(StoreError::NameTaken { entity: "workspace" })), (409, "name_taken".to_string()), "a store error inside, though transparent");
        assert_eq!(answer(WorkspaceError::Store(StoreError::PermissionDenied(refused("workspace")))), (403, "workspace_forbidden".to_string()));
        assert_eq!(answer(WorkspaceError::ReadOnly { id, refused: refused("workspace") }), (403, "read_only".to_string()), "a command ended where the files are only read (issue 294)");
        assert_eq!(answer(Error::new(WorkspaceError::ReadOnly { id, refused: refused("workspace") })), (403, "read_only".to_string()), "by its type before by the refusal it carries");
        assert_eq!(answer(WorkspaceError::Terminal(TerminalError::TooLarge { limit: 1 })), (413, "file_too_large".to_string()));
        assert_eq!(answer(TerminalError::InvalidPath("../x".to_string())), (400, "invalid_path".to_string()));
        assert_eq!(answer(TerminalError::NotFound("a".to_string())), (404, "file_not_found".to_string()));
        assert_eq!(answer(TerminalError::NotEmpty("a".to_string())), (409, "directory_not_empty".to_string()));
        assert_eq!(answer(TerminalError::Unavailable("permission denied".to_string())), (503, "storage_unavailable".to_string()));
        assert_eq!(answer(TerminalError::UnknownCommand("7".to_string())), (404, "command_not_found".to_string()), "nor one not started in the channel");
    }
}
