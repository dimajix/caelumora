use crate::state::AppState;
use axum::{
    extract::State
    ,
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

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
