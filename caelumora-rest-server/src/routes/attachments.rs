//! The attachments of messages (issue 282): a file uploaded for a message the user is about to say,
//! and an attachment of a message downloaded.

use crate::auth::AuthenticatedUser;
use crate::error::{ApiError, Json, Path, Query};
use crate::routes::{channel_of, content_response, open_channel};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use caelumora_model::confidentiality::{Classification, ClassificationError, Compartment};
use caelumora_model::content::{ContentId, ContentRef};
use caelumora_model::message::MessageId;
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_rest_api::channel::AttachmentQuery;
use futures::TryStreamExt;
use std::io::Error;
use std::sync::Arc;
use uuid::Uuid;

/// Keeps the request's body as the file the query names, uploaded by the user for a message they
/// will say in channel `cid` - classified as the query says, the channel's if it names no level -,
/// and answers it as a message names it (`MessageRequest::attachments`); `201`. Its size is the
/// request's `Content-Length`, which is required (`411`).
pub(super) async fn upload_attachment(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Query(query): Query<AttachmentQuery>,
    user: AuthenticatedUser,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<ContentRef>), ApiError> {
    let size = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|length| length.to_str().ok()?.parse::<u64>().ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::LENGTH_REQUIRED,
                "length_required",
                "An attachment is uploaded with its Content-Length",
            )
        })?;
    let classification = classification_of(&query)?;
    let (_, channel_loop) = open_channel(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let channel = Arc::clone(&channel_loop.channel());
    let content = Box::pin(body.into_data_stream().map_err(Error::other));
    let uploaded = state
        .runtime_manager
        .attachment_manager()
        .create(&user.security_context(), &channel, &query.name, classification, (size, content))
        .await?;
    Ok((StatusCode::CREATED, Json(uploaded)))
}

/// The attachment `aid` of message `mid`, for whoever sees the message in the channel's history:
/// an image a browser shows without running anything inline, anything else as a download.
pub(super) async fn download_attachment(
    State(state): State<Arc<AppState>>,
    Path((id, cid, mid, aid)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    user: AuthenticatedUser,
) -> Result<Response, ApiError> {
    let channel = channel_of(&state, &user, SessionId(id), ChannelId(cid)).await?;
    let (attachment, size, content) =
        channel.attachment_content(MessageId(mid), ContentId(aid)).await?;
    Ok(content_response(
        size,
        content,
        attachment.name.as_str(),
        attachment.is_image().then_some(attachment.media_type.as_str()),
    ))
}

/// How the upload `query` describes is classified, its compartments separated by commas; `None`
/// for the channel's.
///
/// # Errors
/// `ClassificationError` if `Classification::given` refuses it.
fn classification_of(
    query: &AttachmentQuery,
) -> Result<Option<Classification>, ClassificationError> {
    let compartments: Vec<Compartment> = match query.compartments.as_str() {
        "" => Vec::new(),
        names => names.split(',').map(Compartment::new).collect(),
    };
    Classification::given(query.level, compartments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::api;
    use crate::routes::tests::{login, state_with};
    use axum::body::to_bytes;
    use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE};
    use axum::http::Request;
    use caelumora_model::config::{AppConfig, BackendConfig, ContentConfig};
    use caelumora_model::security::SecurityContext;
    use caelumora_model::session::Participant;
    use caelumora_model::user::UserId;
    use caelumora_runtime::session::testing::{create, make_session};
    use caelumora_runtime::view::Viewer;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::time;
    use tower::ServiceExt;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nand the rest of it";

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "to be split, longer since rustfmt (issue 490)")]
    async fn a_file_uploaded_for_a_message_is_downloaded_from_it_an_image_shown_inline() {
        let files = tempfile::tempdir().unwrap();
        let content = ContentConfig {
            storage: [(
                "kept".to_string(),
                BackendConfig::new("local", json!({ "root": files.path() })),
            )]
            .into(),
        };
        let state = state_with(AppConfig { content, ..Default::default() });
        let api = api(Arc::clone(&state), &[]);
        let cookie = login(&api, "alice").await;
        let me = api
            .clone()
            .oneshot(Request::get("/auth/me").header(COOKIE, &cookie).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let me: Value =
            serde_json::from_slice(&to_bytes(me.into_body(), usize::MAX).await.unwrap()).unwrap();
        let alice = UserId(me["id"].as_str().unwrap().parse().unwrap());
        let mut talk = make_session("talk", alice);
        talk.participants = vec![Participant::User(alice)];
        let (session, channel) = (talk.id, talk.channels[0]);
        create(&state.session_manager, &SecurityContext::system(), talk).await.unwrap();
        let channel_uri = format!("/sessions/{}/channels/{}", session.0, channel.0);

        let upload = |name: &str, content: &'static [u8], length: bool| {
            let request = Request::post(format!("{channel_uri}/attachments?name={name}"))
                .header(COOKIE, &cookie);
            let request = if length {
                request.header(header::CONTENT_LENGTH, content.len())
            } else {
                request
            };
            api.clone().oneshot(request.body(Body::from(content)).unwrap())
        };
        let unsized_upload = upload("chart.png", PNG, false).await.unwrap();
        assert_eq!(unsized_upload.status(), StatusCode::LENGTH_REQUIRED);
        let uploaded = |response: Response| async move {
            assert_eq!(response.status(), StatusCode::CREATED);
            serde_json::from_slice::<ContentRef>(
                &to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            )
            .unwrap()
        };
        let chart = uploaded(upload("chart.png", PNG, true).await.unwrap()).await;
        let notes =
            uploaded(upload("notes.html", b"<script>alert(1)</script>", true).await.unwrap()).await;
        assert_eq!(
            (chart.media_type.as_str(), notes.media_type.as_str()),
            ("image/png", "text/plain"),
            "as their bytes say"
        );

        let body = json!({ "attachments": [chart.id, notes.id] }).to_string();
        let sent = api
            .clone()
            .oneshot(
                Request::post(format!("{channel_uri}/messages"))
                    .header(COOKIE, &cookie)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sent.status(), StatusCode::OK, "a message of attachments alone");
        let message =
            time::timeout(Duration::from_secs(5), async {
                loop {
                    let views = state.session_manager.views(Viewer::new(SecurityContext::system()));
                    let history = views
                        .session(session)
                        .await
                        .unwrap()
                        .channel(channel)
                        .await
                        .unwrap()
                        .history_page(None, 10)
                        .await
                        .unwrap();
                    if let Some(message) = history.items.iter().find_map(|(_, entry)| {
                        entry.said().filter(|said| !said.attachments.is_empty())
                    }) {
                        break message.id;
                    }
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("said");

        let download = |id: ContentId| {
            api.clone().oneshot(
                Request::get(format!("{channel_uri}/messages/{}/attachments/{}", message.0, id.0))
                    .header(COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let shown = download(chart.id).await.unwrap();
        assert_eq!(shown.status(), StatusCode::OK);
        assert_eq!(shown.headers()[CONTENT_TYPE], "image/png");
        assert!(shown.headers()[CONTENT_DISPOSITION].to_str().unwrap().starts_with("inline;"));
        assert_eq!(to_bytes(shown.into_body(), usize::MAX).await.unwrap(), PNG);
        let downloaded = download(notes.id).await.unwrap();
        assert_eq!(
            downloaded.headers()[CONTENT_TYPE],
            "application/octet-stream",
            "never run in the web UI's origin"
        );
        assert!(downloaded.headers()[CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;"));
        assert_eq!(
            download(ContentId::new()).await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "none the message carries"
        );
    }
}
