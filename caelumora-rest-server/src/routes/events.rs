use std::sync::Arc;
use std::convert::Infallible;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use futures::StreamExt;
use tokio_stream::wrappers::{ReceiverStream, BroadcastStream};
use uuid::Uuid;
use caelumora_model::session::{ChannelId, SessionId};
use crate::state::AppState;

pub async fn join_channel(
    State(state): State<Arc<AppState>>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let session_id = SessionId(id);
    let channel_id = ChannelId(cid);

    let session_loop = state.session_manager.get_session(session_id)
        .ok_or(StatusCode::NOT_FOUND)?;
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

pub async fn global_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.runtime_manager.event_bus().subscribe_all();
    let stream = ReceiverStream::new(receiver);

    let sse_stream = stream.map(|event| {
        let event_json = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(event_json))
    });

    Sse::new(sse_stream)
}
