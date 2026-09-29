use crate::auth::AuthenticatedUser;
use crate::models::{ChannelMemberResponse, ChannelResponse, GlobalSnapshot, ParticipantKind, ParticipantResponse, SessionSnapshot, StreamOptions};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::mention::handle_of;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::Session;
use caelumora_model::store::EntityStore;
use futures::stream::Stream;
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
    let snapshot = move || {
        let (state, user) = (state.clone(), user);
        async move { global_snapshot(&state, &user).await }
    };
    // Open until the token that authenticated it expires (issue 33).
    Sse::new(crate::sse::stream(&user, options.snapshot, receiver, "the global stream".to_string(), snapshot, std::future::pending()).await)
}

/// Every session the user may read, with its channels, as the sessions'
/// events refer to them by id alone. A running session is described by its
/// live channels, a stopped one by what the store holds.
async fn global_snapshot(state: &Arc<AppState>, user: &AuthenticatedUser) -> GlobalSnapshot {
    let sessions = match state.runtime_manager.entity_store().list_sessions(&user.security_context()).await {
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
                // Only the channels the viewer may read (issue 70).
                if crate::routes::may_read_channel(state, &channel, viewer).await {
                    channels.push(crate::routes::channels::channel_response(state, &channel, viewer).await);
                }
            }
            session_snapshot(state, (**running.get_session()).clone(), channels).await
        }
        None => {
            // The channels are documents of their own (issue 70): those the
            // viewer may read.
            let mut channels = Vec::new();
            for id in &session.channels {
                if let Ok(channel) = state.runtime_manager.entity_store().get_channel(viewer, *id).await {
                    channels.push(stored_channel_response(state.runtime_manager.entity_store().as_ref(), &channel).await);
                }
            }
            session_snapshot(state, session, channels).await
        }
    }
}

async fn session_snapshot(state: &AppState, session: Session, channels: Vec<ChannelResponse>) -> SessionSnapshot {
    let mut participants = Vec::new();
    for participant in &session.participants {
        let (id, name, kind) = describe_participant(state.runtime_manager.entity_store().as_ref(), participant).await;
        participants.push(ParticipantResponse { id, name: name.unwrap_or_else(|| UNKNOWN.to_string()), kind });
    }
    SessionSnapshot {
        id: session.id,
        name: session.metadata.name,
        channels,
        participants,
    }
}

/// What a participant is called whose user or agent no longer exists.
const UNKNOWN: &str = "(unknown)";

/// The id, name and kind of a participant, as the API shows them. The name
/// is read from the store, as the runtime: taking part in a session makes
/// a name known to everybody in it - it stands at every message - even
/// where the record itself is not theirs to read (issue 120). `None` for a
/// user or agent that no longer exists.
async fn describe_participant(store: &dyn EntityStore, participant: &Participant) -> (ActorId, Option<String>, ParticipantKind) {
    let system = SecurityContext::system();
    match participant {
        Participant::User(id) => (ActorId::User(*id), store.get_user(&system, *id).await.ok().map(|u| u.username), ParticipantKind::User),
        Participant::Agent(id) => (ActorId::Agent(*id), store.get_agent(&system, *id).await.ok().map(|a| a.metadata.name), ParticipantKind::Agent),
        Participant::Approver(a) => (ActorId::Approver(a.id), Some("Approver".to_string()), ParticipantKind::Approver),
    }
}

/// A channel of a session that is not running, described by the store rather
/// than by its actors.
async fn stored_channel_response(store: &dyn EntityStore, channel: &Channel) -> ChannelResponse {
    let mut members = Vec::new();
    for participant in &channel.participants {
        let (id, name, kind) = describe_participant(store, participant).await;
        let handle = name.as_deref().map(handle_of).filter(|h| !h.is_empty() && kind != ParticipantKind::Approver);
        // Not running: there is no actor to ask; opening the channel starts
        // it, and its own snapshot says.
        members.push(ChannelMemberResponse { id, name: name.unwrap_or_else(|| UNKNOWN.to_string()), handle, kind, can_read_context: false });
    }

    ChannelResponse { id: channel.id, name: channel.name.clone(), members, moderator: channel.moderator.clone(), confidentiality: channel.confidentiality, can_change: false }
}


#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::session::SessionId;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::{User, UserId};
    use uuid::Uuid;

    #[tokio::test]
    async fn a_stopped_session_is_described_by_the_store() {
        let user = User {
            id: UserId::new(),
            username: "Alice Example".to_string(),
            email: String::new(),
            roles: Vec::new(),
        };
        // Refuses everything but the system: the names are read as the
        // runtime, whoever asks.
        let store = MemoryStore::with_security_manager(Arc::new(caelumora_model::security::OwnershipSecurityManager::new()));
        store.create_user(&SecurityContext::system(), &user).await.unwrap();
        let approver = Approver { id: ApproverId(Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let gone = UserId::new();
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            vec![Participant::User(user.id), Participant::User(gone), Participant::Approver(approver)],
            SecurityAttributes::default(),
        );

        let response = stored_channel_response(&store, &channel).await;

        assert_eq!(response.name, "main");
        let alice = response.members.iter().find(|m| m.name == "Alice Example").expect("the user is a member");
        assert_eq!(alice.handle.as_deref(), Some("alice-example"), "mentionable like in a running channel");
        assert_eq!(alice.kind, ParticipantKind::User);
        let approver = response.members.iter().find(|m| m.kind == ParticipantKind::Approver).expect("the approver is a member");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
        let gone = response.members.iter().find(|m| m.id == ActorId::User(gone)).expect("a deleted user is still listed");
        assert_eq!(gone.name, UNKNOWN);
        assert!(gone.handle.is_none(), "and cannot be mentioned");
    }
}
