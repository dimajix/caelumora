//! A session as its viewer reached it: its document, and its loop if it
//! runs.

use super::channel::Readable;
use super::{ChannelView, ViewError, ViewResult, Views};
use crate::channel::ChannelLoop;
use crate::session::{Found, SessionLoop};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::confidentiality::Classification;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::security::{AccessQuestions, Place, SecurityContext};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::snapshot::{DeletedSnapshot, Detail, ParticipantSnapshot, SessionSnapshot};
use caelumora_model::store::EntityKind;
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

/// A channel document of the session, with its loop if it runs.
type Document = (Arc<Channel>, Option<Arc<ChannelLoop>>);

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

    pub fn views(&self) -> &Views {
        &self.views
    }

    pub fn id(&self) -> SessionId {
        self.session.id
    }

    pub fn name(&self) -> &str {
        &self.session.name
    }

    /// The session as privacy sees it now (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        match &self.running {
            Some(session_loop) => session_loop.privacy(),
            None => SessionPrivacy::of(&self.session),
        }
    }

    /// Everyone who takes part in the session - whom a channel can be made
    /// of (issue 09).
    pub fn participants(&self) -> &[Participant] {
        &self.session.participants
    }

    /// Whether the viewer may change the session - rename it, change its
    /// privacy (`UpdateSession`).
    pub async fn may_change(&self) -> bool {
        self.views.security().may_change_session(self.views.context(), &self.session.security_attributes).await
    }

    /// The channels the viewer may read as a whole, in the order of the
    /// session: a user those they may read (`GetChannel`) and are cleared
    /// for (issue 133); an agent those its channel may read by their
    /// classifications (issue 17). A channel whose document cannot be read
    /// is left out, so that one broken channel does not hide its session.
    pub async fn channels(&self) -> Vec<ChannelView> {
        let mut readable = Vec::new();
        for (channel, running) in self.channel_documents().await {
            if self.readable(&channel).await == Some(Readable::Whole) {
                readable.push(ChannelView::of(self.clone(), channel, running, Readable::Whole));
            }
        }
        readable
    }

    /// Every channel of the session the viewer may read at least in part:
    /// for an agent, those it may read as a whole and those of which only
    /// what was released to it can be read, entry by entry; for a user,
    /// the same as `channels()`.
    pub async fn all_channels(&self) -> Vec<ChannelView> {
        let mut reachable = Vec::new();
        for (channel, running) in self.channel_documents().await {
            if let Some(readable) = self.readable(&channel).await {
                reachable.push(ChannelView::of(self.clone(), channel, running, readable));
            }
        }
        reachable
    }

    /// Channel `id` of the session, if the viewer may read it - as a whole,
    /// or, for an agent, entry by entry.
    ///
    /// # Errors
    /// `ChannelNotFound` if the session has no such channel, and equally if
    /// the viewer may not read it, so that channels cannot be probed.
    pub async fn channel(&self, id: ChannelId) -> ViewResult<ChannelView> {
        let not_found = || ViewError::ChannelNotFound { session: self.id(), channel: id };
        let (channel, running) = self.document(id).await?.ok_or_else(not_found)?;
        let readable = self.readable(&channel).await.ok_or_else(not_found)?;
        Ok(ChannelView::of(self.clone(), channel, running, readable))
    }

    /// The channel named `name`, as an agent's tools ask for one: if the
    /// viewer may read it - as a whole, or entry by entry.
    ///
    /// # Errors
    /// `NoChannelNamed` if there is no such channel, and equally if the
    /// viewer may not read it.
    pub async fn channel_named(&self, name: &str) -> ViewResult<ChannelView> {
        let not_found = || ViewError::NoChannelNamed(name.to_string());
        let (channel, running) = self.channel_documents().await.into_iter().find(|(c, _)| c.name == name).ok_or_else(not_found)?;
        let readable = self.readable(&channel).await.ok_or_else(not_found)?;
        Ok(ChannelView::of(self.clone(), channel, running, readable))
    }

    /// How the viewer may read `channel`: as a whole, entry by entry, or
    /// not at all (`None`). A user reads a channel as a whole or not at
    /// all; an agent reads what its channel may read by their
    /// classifications, and the rest entry by entry.
    async fn readable(&self, channel: &Channel) -> Option<Readable> {
        let security = self.views.security();
        let context = self.views.context();
        if self.views.viewer().place().is_some() {
            let source = Place { channel: channel.id, classification: channel.classification.clone(), session: self.privacy() };
            let whole = self.views.may_read_from(&source).await;
            return Some(if whole { Readable::Whole } else { Readable::Released });
        }
        let readable = security.may_read_channel(context, &channel.security_attributes).await
            && self.cleared_for(&channel.classification).await;
        readable.then_some(Readable::Whole)
    }

    /// Whether the viewer is cleared for `classification` (issue 133):
    /// reading a channel one is not cleared for is the same as reading one
    /// that does not exist.
    async fn cleared_for(&self, classification: &Classification) -> bool {
        let roles = self.views.roles().await;
        self.views.security().may_see_channel(self.views.context(), &roles, classification).await
    }

    /// The document of channel `id`, if the session has it, with its loop
    /// if it runs. Read in the runtime's name: reaching the session was
    /// permitted, and whether the channel may be read is asked of it.
    async fn document(&self, id: ChannelId) -> ViewResult<Option<Document>> {
        match &self.running {
            Some(session_loop) => Ok(session_loop.channel_by_id(id).map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop)))),
            None if self.session.channels.contains(&id) => {
                let channel = self.views.entities().get_channel(&SecurityContext::system(), id).await
                    .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("reading channel {id}"))))?;
                Ok(Some((Arc::new(channel), None)))
            }
            None => Ok(None),
        }
    }

    /// The session's channel documents, in its order, each with its loop if
    /// it runs. A channel whose document cannot be read is left out.
    async fn channel_documents(&self) -> Vec<Document> {
        match &self.running {
            Some(session_loop) => session_loop.channels().into_iter()
                .map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop)))
                .collect(),
            None => {
                let store = self.views.entities();
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

    /// The private notes agent `agent` wrote in the channels of the session
    /// the viewer reaches (`all_channels`, issues 51 and 233), each with
    /// its channel and the digest of its chain, sorted by the channel's
    /// name; channels without any are left out. Each is read as
    /// `MemberView::private_notes` reads it: for a viewer who may read what
    /// the agent saw, of a channel read entry by entry what was released.
    /// A channel whose notes cannot be read is left out, so that one broken
    /// state does not hide the others.
    ///
    /// # Errors
    /// `Forbidden` if the viewer may not read what the agent saw.
    pub async fn private_notes_of(&self, agent: AgentId) -> ViewResult<Vec<(ChannelView, Vec<Note>, Option<Digest>)>> {
        let mut found = Vec::new();
        for channel in self.all_channels().await {
            // Not a member there: nothing noted there.
            let Ok(member) = channel.member(ActorId::Agent(agent)).await else { continue };
            match member.private_notes().await {
                Ok((notes, digest)) if !notes.is_empty() => found.push((channel, notes, digest)),
                Ok(_) => {}
                Err(e @ ViewError::Forbidden { .. }) => return Err(e),
                Err(e) => warn!("Reading the private notes of agent {} in channel {} failed: {}", agent, channel.id(), e),
            }
        }
        found.sort_by(|(l, ..), (r, ..)| l.name().as_str().cmp(r.name().as_str()));
        Ok(found)
    }

    /// The channels of the session the viewer deleted and may read as such
    /// (issue 164): the session's trash. One may be restored by whoever may
    /// restore it and change the session (`SessionLoop::restore_channel`);
    /// none is purged but with the session.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn deleted_channels(&self) -> ViewResult<Vec<DeletedSnapshot<ChannelId>>> {
        let views = self.views();
        let context = views.context();
        let channels = views.entities().list_deleted_channels(context).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context(format!("listing the deleted channels of session {}", self.id()))))?;
        let may_change = self.may_change().await;
        let mut deleted = Vec::new();
        for channel in channels.into_iter().filter(|c| c.session_id == self.id()) {
            let Some(removal) = channel.deleted.clone() else { continue };
            deleted.push(DeletedSnapshot {
                id: channel.id,
                name: channel.name.to_string(),
                deleted: removal,
                can_restore: may_change && views.security().may_restore(context, EntityKind::Channel, &channel.security_attributes).await,
                can_purge: false,
            });
        }
        Ok(deleted)
    }

    /// The session as the viewer sees it at this moment (issue 80): with
    /// its participants, and the details `detail` asks for - the channels
    /// the viewer may read, and theirs.
    pub async fn snapshot(&self, detail: Detail) -> SessionSnapshot {
        let mut channels = Vec::new();
        if detail.contains(Detail::CHANNELS) {
            for channel in self.channels().await {
                channels.push(channel.snapshot(detail).await);
            }
        }
        let mut participants = Vec::with_capacity(self.session.participants.len());
        for participant in &self.session.participants {
            let name = self.views.name_of(participant).await;
            participants.push(ParticipantSnapshot { id: participant.actor_id(), name });
        }
        SessionSnapshot {
            id: self.session.id,
            name: self.session.name.clone(),
            privacy: self.session.privacy,
            can_change: self.may_change().await,
            participants,
            channels,
        }
    }
}
