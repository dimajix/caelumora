use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, GlobalSnapshot, ParticipantResponse, SessionSnapshot, StreamOptions};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::mention::handle_of;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::Session;
use futures::stream::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;

pub async fn global_events(
    State(state): State<Arc<AppState>>,
    Query(options): Query<StreamOptions>,
    user: AuthenticatedUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before reading the state, so that nothing that happens while
    // the snapshot is assembled is lost (issue 34).
    // Only the events about what this user may read (issue 39), decided by
    // the same policy as reading it through the store.
    let receiver = state.runtime_manager.event_bus().subscribe_as(user.security_context()).into_receiver();
    let snapshot = if options.snapshot { Some(snapshot_event(&state, &user).await) } else { None };

    // A client that falls behind gets a fresh snapshot, as on a channel's
    // stream (issue 38).
    let resync = options.snapshot.then(|| {
        let state = state.clone();
        move || {
            let state = state.clone();
            async move { snapshot_event(&state, &user).await }
        }
    });
    let updates = crate::sse::resync_on_lag(receiver, "the global stream".to_string(), resync);

    let sse_stream = futures::stream::iter(snapshot.into_iter().map(Ok)).chain(updates);
    // The stream ends with the session token that authenticated it (issue 33).
    Sse::new(crate::sse::bounded_by_token(&user, sse_stream))
}

/// Every session the user may read, with its channels, as the sessions'
/// events refer to them by id alone. A running session is described by its
/// live channels, a stopped one by what the store holds.
async fn global_snapshot(state: &Arc<AppState>, user: &AuthenticatedUser) -> GlobalSnapshot {
    let sessions = match state.entity_store.list_sessions(&user.security_context()).await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::error!("Cannot assemble the snapshot of the global event stream: {}", e);
            Vec::new()
        }
    };

    let viewer = user.security_context();
    let mut described = Vec::new();
    for session in sessions {
        described.push(describe_session(state, session, &viewer).await);
    }
    let sessions = described;

    GlobalSnapshot { sessions }
}

/// A session with its channels, as the global snapshot and `GET
/// /sessions/{id}` describe it. A running session is described by its live
/// state, a stopped one by what the store holds - describing it does not
/// start it.
pub(crate) async fn describe_session(state: &AppState, session: Session, viewer: &SecurityContext) -> SessionSnapshot {
    match state.session_manager.get_session(session.id) {
        Some(running) => {
            let mut channels = Vec::new();
            for channel in running.channels() {
                channels.push(crate::routes::channels::channel_response(&channel, viewer).await);
            }
            session_snapshot((**running.get_session()).clone(), channels)
        }
        None => {
            // The channels are documents of their own (issue 70): those the
            // viewer may read.
            let mut channels = Vec::new();
            for id in &session.channels {
                if let Ok(channel) = state.entity_store.get_channel(viewer, *id).await {
                    channels.push(stored_channel_response(&channel));
                }
            }
            session_snapshot(session, channels)
        }
    }
}

async fn snapshot_event(state: &Arc<AppState>, user: &AuthenticatedUser) -> Event {
    let snapshot = global_snapshot(state, user).await;
    Event::default().event("snapshot").data(serde_json::to_string(&snapshot).unwrap_or_default())
}

fn session_snapshot(session: Session, channels: Vec<ChannelResponse>) -> SessionSnapshot {
    let participants = session.participants.iter().map(|participant| {
        let (id, name, kind) = describe_participant(participant);
        ParticipantResponse { id, name, kind: kind.to_string() }
    }).collect();
    SessionSnapshot {
        id: session.id,
        name: session.metadata.name.clone(),
        metadata: session.metadata,
        channels,
        participants,
    }
}

/// The id, name and kind of a participant, as the API shows them.
fn describe_participant(participant: &Participant) -> (caelumora_model::actor::ActorId, String, &'static str) {
    match participant {
        Participant::User(u) => (caelumora_model::actor::ActorId::User(u.id), u.username.clone(), "user"),
        Participant::Agent(a) => (caelumora_model::actor::ActorId::Agent(a.id), a.metadata.name.clone(), "agent"),
        Participant::Approver(a) => (caelumora_model::actor::ActorId::Approver(a.id.0), "Approver".to_string(), "approver"),
    }
}

/// A channel of a session that is not running, described by the store rather
/// than by its actors.
fn stored_channel_response(channel: &Channel) -> ChannelResponse {
    let members = channel.participants.iter().map(|participant| {
        let (id, name, kind) = describe_participant(participant);
        let handle = Some(handle_of(&name)).filter(|h| !h.is_empty() && kind != "approver");
        // Not running: there is no actor to ask; opening the channel starts
        // it, and its own snapshot says.
        ChannelMemberResponse { id, name, handle, kind: kind.to_string(), can_read_context: false }
    }).collect();

    ChannelResponse { id: channel.id, name: channel.name.clone(), members, moderator: channel.moderator.clone(), confidentiality: channel.confidentiality }
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
