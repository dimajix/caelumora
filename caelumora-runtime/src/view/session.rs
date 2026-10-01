//! A session as its viewer reached it: its document, and its loop if it
//! runs.

use super::{ChannelView, ViewError, ViewResult, Views};
use crate::session::{Found, SessionLoop};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::confidentiality::Classification;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityContext};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::snapshot::{ParticipantSnapshot, SessionSnapshot};
use std::sync::Arc;
use tracing::warn;

/// A session the viewer may read.
#[derive(Debug, Clone)]
pub struct SessionView {
    views: Views,
    /// The document as it was when the session was reached.
    session: Arc<Session>,
    /// Its loop, if it runs: where its channels are reached through.
    running: Option<Arc<SessionLoop>>,
}

impl SessionView {
    pub(super) fn of(views: Views, found: Found) -> Self {
        match found {
            Found::Running(session_loop) => {
                let session = Arc::clone(&session_loop.session());
                Self { views, session, running: Some(session_loop) }
            }
            Found::Stored(session) => Self { views, session: Arc::new(session), running: None },
        }
    }

    pub(super) fn views(&self) -> &Views {
        &self.views
    }

    pub fn id(&self) -> SessionId {
        self.session.id
    }

    pub fn name(&self) -> &str {
        &self.session.metadata.name
    }

    /// The session as privacy sees it (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        SessionPrivacy::of(&self.session)
    }

    /// Everyone who takes part in the session - whom a channel can be made
    /// of (issue 09).
    pub fn participants(&self) -> &[Participant] {
        &self.session.participants
    }

    /// Whether the viewer may change the session - rename it, change its
    /// privacy (`UpdateSession`).
    pub async fn may_change(&self) -> bool {
        self.views.runtime().security_manager().may_change_session(self.views.viewer().context(), &self.session.security_attributes).await
    }

    /// The channels the viewer may read (`GetChannel`, and cleared for
    /// their classification, issues 70, 133), in the order of the session.
    /// A channel whose document cannot be read is left out, so that one
    /// broken channel does not hide its session.
    pub async fn channels(&self) -> Vec<ChannelView> {
        let security = self.views.runtime().security_manager();
        let context = self.views.viewer().context();
        let roles = self.views.roles().await;
        let mut readable = Vec::new();
        for (channel, running) in self.channel_documents().await {
            if security.may_read_channel(context, &channel.security_attributes).await
                && security.may_see_channel(context, &roles, &channel.classification).await
            {
                readable.push(ChannelView::of(self.clone(), channel, running));
            }
        }
        readable
    }

    /// Channel `id` of the session, if the viewer may read it.
    ///
    /// # Errors
    /// `ChannelNotFound` if the session has no such channel, and equally if
    /// the viewer may not read it, so that channels cannot be probed.
    pub async fn channel(&self, id: ChannelId) -> ViewResult<ChannelView> {
        let not_found = || ViewError::ChannelNotFound { session: self.id(), channel: id };
        let (channel, running) = match &self.running {
            Some(session_loop) => session_loop.channel_by_id(id).map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop))),
            None if self.session.channels.contains(&id) => {
                let channel = self.views.runtime().entity_store().get_channel(&SecurityContext::system(), id).await
                    .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("reading channel {id}"))))?;
                Some((Arc::new(channel), None))
            }
            None => None,
        }
        .ok_or_else(not_found)?;
        self.views.runtime().security_manager()
            .require(self.views.viewer().context(), ActionType::GetChannel(&channel.security_attributes), not_found).await
            .map_err(|_| not_found())?;
        if !self.cleared_for(&channel.classification).await {
            return Err(not_found());
        }
        Ok(ChannelView::of(self.clone(), channel, running))
    }

    /// Whether the viewer is cleared for `classification` (issue 133):
    /// reading a channel one is not cleared for is the same as reading one
    /// that does not exist.
    pub(super) async fn cleared_for(&self, classification: &Classification) -> bool {
        let roles = self.views.roles().await;
        self.views.runtime().security_manager().may_see_channel(self.views.viewer().context(), &roles, classification).await
    }

    /// The session's channel documents, in its order, each with its loop if
    /// it runs. Read in the runtime's name: reaching the session was
    /// permitted, and whether each channel may be read is asked of each.
    async fn channel_documents(&self) -> Vec<(Arc<Channel>, Option<Arc<crate::channel::ChannelLoop>>)> {
        match &self.running {
            Some(session_loop) => session_loop.channels().into_iter()
                .map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop)))
                .collect(),
            None => {
                let store = self.views.runtime().entity_store();
                let system = SecurityContext::system();
                let mut channels = Vec::with_capacity(self.session.channels.len());
                for id in &self.session.channels {
                    match store.get_channel(&system, *id).await {
                        Ok(channel) => channels.push((Arc::new(channel), None)),
                        Err(e) => warn!("Cannot read channel {} of session {}: {}", id, self.session.id, e),
                    }
                }
                channels
            }
        }
    }

    /// The session as the viewer sees it at this moment (issue 80): with
    /// its participants and the channels they may read.
    pub async fn snapshot(&self) -> SessionSnapshot {
        let mut channels = Vec::new();
        for channel in self.channels().await {
            channels.push(channel.snapshot().await);
        }
        let mut participants = Vec::with_capacity(self.session.participants.len());
        for participant in &self.session.participants {
            let (name, _) = self.views.look_up(participant).await;
            participants.push(ParticipantSnapshot { id: participant.actor_id(), name });
        }
        SessionSnapshot {
            id: self.session.id,
            name: self.session.metadata.name.clone(),
            privacy: self.session.privacy,
            can_change: self.may_change().await,
            participants,
            channels,
        }
    }
}
