use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, GlobalSnapshot, SessionSnapshot, StreamOptions};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::mention::handle_of;
use caelumora_model::session::Session;
use futures::stream::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

pub async fn global_events(
    State(state): State<Arc<AppState>>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading the state, so that nothing that happens while
    // the snapshot is assembled is lost (issue 34).
    let receiver = state.runtime_manager.event_bus().subscribe_all();
    let snapshot = if options.snapshot { Some(snapshot_event(&state, &user).await) } else { None };

    let updates = ReceiverStream::new(receiver).map(|event| {
        let event_json = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(event_json))
    });

    Sse::new(futures::stream::iter(snapshot.into_iter().map(Ok)).chain(updates))
}

/// Every session with its channels, as the sessions' events refer to them by
/// id alone.
///
/// Unfiltered: every user sees every session until issue 11 decides who may
/// see what. A running session is described by its live channels, a stopped
/// one by what the store holds.
async fn global_snapshot(state: &Arc<AppState>, user: &AuthenticatedUser) -> GlobalSnapshot {
    let sessions = match state.entity_store.list_sessions(&user.security_context()).await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::error!("Cannot assemble the snapshot of the global event stream: {}", e);
            Vec::new()
        }
    };

    let sessions = sessions.into_iter().map(|session| {
        let channels = match state.session_manager.get_session(session.id) {
            Some(running) => running.channels().iter().map(|c| crate::routes::channels::channel_response(c)).collect(),
            None => session.channels.iter().map(stored_channel_response).collect(),
        };
        session_snapshot(session, channels)
    }).collect();

    GlobalSnapshot { sessions }
}

async fn snapshot_event(state: &Arc<AppState>, user: &AuthenticatedUser) -> Event {
    let snapshot = global_snapshot(state, user).await;
    Event::default().event("snapshot").data(serde_json::to_string(&snapshot).unwrap_or_default())
}

fn session_snapshot(session: Session, channels: Vec<ChannelResponse>) -> SessionSnapshot {
    SessionSnapshot {
        id: session.id,
        name: session.metadata.name.clone(),
        metadata: session.metadata,
        channels,
    }
}

/// A channel of a session that is not running, described by the store rather
/// than by its actors.
fn stored_channel_response(channel: &Channel) -> ChannelResponse {
    let members = channel.participants.iter().map(|participant| {
        let (id, name, kind) = match participant {
            Participant::User(u) => (caelumora_model::actor::ActorId::User(u.id), u.username.clone(), "user"),
            Participant::Agent(a) => (caelumora_model::actor::ActorId::Agent(a.id), a.metadata.name.clone(), "agent"),
            Participant::Approver(a) => (caelumora_model::actor::ActorId::Approver(a.id.0), "Approver".to_string(), "approver"),
        };
        let handle = Some(handle_of(&name)).filter(|h| !h.is_empty() && kind != "approver");
        ChannelMemberResponse { id, name, handle, kind: kind.to_string() }
    }).collect();

    ChannelResponse { id: channel.id, name: channel.name.clone(), members }
}


#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
    use caelumora_model::security::{SecurityAttributes, SecurityContext};
    use caelumora_model::session::SessionId;
    use caelumora_model::user::{User, UserId};
    use uuid::Uuid;

    #[test]
    fn a_stopped_session_is_described_by_the_store() {
        let user = User {
            id: UserId::new(),
            username: "Alice Example".to_string(),
            email: String::new(),
            roles: Vec::new(),
            security: SecurityContext::default(),
        };
        let approver = Approver { id: ApproverId(Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            vec![Participant::User(user.clone()), Participant::Approver(approver)],
            SecurityAttributes::default(),
        );

        let response = stored_channel_response(&channel);

        assert_eq!(response.name, "main");
        let alice = response.members.iter().find(|m| m.name == "Alice Example").expect("the user is a member");
        assert_eq!(alice.handle.as_deref(), Some("alice-example"), "mentionable like in a running channel");
        assert_eq!(alice.kind, "user");
        let approver = response.members.iter().find(|m| m.kind == "approver").expect("the approver is a member");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
    }
}
