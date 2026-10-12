//! A session as its viewer reached it: its document, and its loop if it
//! runs.

use caelumora_model::store::query::{Filter, Query};
use super::{ChannelView, ViewError, ViewResult, Views};
use crate::channel::ChannelLoop;
use crate::session::{Found, SessionLoop};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::{Channel, ChannelField, Participant};
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::confidentiality::Place;
use caelumora_model::security::{AccessQuestions, SecurityContext};
use super::{channel_reading, ChannelReader, ChannelReading};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::snapshot::{DeletedSnapshot, Detail, ParticipantSnapshot, SessionSnapshot};
use std::sync::Arc;
use tracing::warn;
use caelumora_model::removal::Stored;

/// An agent's private notes in another channel of its session, as far as
/// they may be read where it reads from (`SessionView::own_private_notes`):
/// what of that channel it knows itself - no view of the channel, which it
/// may no longer reach.
#[derive(Debug, Clone)]
pub struct PrivateNotesElsewhere {
    pub channel: ChannelId,
    pub name: String,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}

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

    /// The document, as it was when the session was reached.
    pub(super) fn as_session(&self) -> &Arc<Session> {
        &self.session
    }

    pub fn id(&self) -> SessionId {
        self.session.id
    }

    pub fn name(&self) -> &str {
        self.session.name.as_str()
    }

    /// What the session is for, in a line (issue 245); `None` if nobody said.
    pub fn summary(&self) -> Option<&Summary> {
        self.session.summary.as_ref()
    }

    /// What an agent should know before it works in the session (issue 245);
    /// `None` if nobody wrote it.
    pub fn briefing(&self) -> Option<&Briefing> {
        self.session.briefing.as_ref()
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
            if self.readable(&channel).await == Some(ChannelReading::Whole) {
                readable.push(ChannelView::of(self.clone(), channel, running, ChannelReading::Whole));
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
    /// not at all (`None`) - asked as `channel_reading` asks it, an agent
    /// reading from its place, a user by the roles of their context.
    /// Whoever may not read the channel - its owner, its participants, an
    /// agent's owner or the agent itself (issue 264) - reads none of it
    /// (`GetChannel`).
    pub(super) async fn readable(&self, channel: &Channel) -> Option<ChannelReading> {
        let at = Place { channel: channel.id, classification: channel.classification.clone(), session: self.privacy() };
        let reader = match self.views.viewer().place() {
            Some(from) => ChannelReader::From(from),
            None => ChannelReader::Cleared,
        };
        channel_reading(self.views.security().as_ref(), self.views.confidentiality().as_ref(), self.views.context(), &channel.security_attributes, &at, reader).await
    }

    /// The document of channel `id`, if the session has it, with its loop
    /// if it runs. Read in the runtime's name: reaching the session was
    /// permitted, and whether the channel may be read is asked of it.
    async fn document(&self, id: ChannelId) -> ViewResult<Option<Document>> {
        match &self.running {
            Some(session_loop) => Ok(session_loop.channel_by_id(id).map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop)))),
            None if self.session.channels.contains(&id) => {
                let channel = self.views.stores().channels.get(&SecurityContext::system(), id).await
                    .map_err(|e| ViewError::failed(e, format!("reading channel {id}")))?;
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
                let store = &self.views.stores().channels;
                let system = SecurityContext::system();
                let mut channels = Vec::with_capacity(self.session.channels.len());
                for id in &self.session.channels {
                    match store.get(&system, *id).await {
                        Ok(channel) => channels.push((Arc::new(channel), None)),
                        Err(e) => warn!("Cannot read channel {} of session {}: {}", id, self.session.id, e),
                    }
                }
                channels
            }
        }
    }

    /// The private notes the viewing agent wrote in the session's other
    /// channels (issues 51, 264), each with its channel and the digest of
    /// its chain, sorted by the channel's name: its own knowledge, reached
    /// through itself rather than through the channels - no `GetChannel` is
    /// asked, so that it keeps them also of a channel it left and its owner
    /// may not read. Each note is read as far as it may be copied into the
    /// viewer's place. Of every channel it takes or took part in; channels
    /// without any are left out, as is its own. None for a viewer that is
    /// no agent reading from a place; a state that cannot be read is
    /// logged and left out.
    pub async fn own_private_notes(&self) -> Vec<PrivateNotesElsewhere> {
        let ActorId::Agent(agent) = self.views.context().caller_id else { return Vec::new() };
        let Some(here) = self.views.viewer().place().map(|place| place.channel) else { return Vec::new() };
        let mut found = Vec::new();
        for (channel, _) in self.channel_documents().await {
            let actor = ActorId::Agent(agent);
            let took_part = channel.participants.iter().any(|p| p.actor_id() == actor)
                || channel.former.iter().any(|f| f.participant.actor_id() == actor);
            if channel.id == here || !took_part {
                continue;
            }
            let state = match self.views.states().get_agent_channel_state(agent, channel.id).await {
                Ok(state) => state,
                Err(e) => {
                    warn!("Reading the private notes of agent {} in channel {} failed: {}", agent, channel.id, e);
                    continue;
                }
            };
            let mut notes = Vec::new();
            for note in state.private_notes.current() {
                let from = Place { channel: channel.id, classification: note.classification.clone(), session: self.privacy() };
                if self.views.may_copy_from(&from) {
                    notes.push(note);
                }
            }
            if !notes.is_empty() {
                found.push(PrivateNotesElsewhere { channel: channel.id, name: channel.name.to_string(), notes, digest: state.private_notes.digest() });
            }
        }
        found.sort_by(|l, r| l.name.cmp(&r.name));
        found
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
        let failed = |e| ViewError::failed(e, format!("listing the deleted channels of session {}", self.id()));
        let of_this = Query::filtered(Filter::eq(ChannelField::Session, self.id()));
        let channels = views.stores().channels.list_deleted(context, &of_this).await.map_err(failed)?.items;
        let may_change = self.may_change().await;
        let mut deleted = Vec::new();
        for channel in channels {
            let Some(removal) = channel.deleted.clone() else { continue };
            deleted.push(DeletedSnapshot {
                id: channel.id,
                name: channel.name.to_string(),
                deleted: removal,
                can_restore: may_change && views.security().may_restore(context, Channel::KIND, &channel.security_attributes).await,
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
            let name = self.views.name_of(participant.actor_id()).await;
            participants.push(ParticipantSnapshot { id: participant.actor_id(), name });
        }
        // Absent, too, where it is gone or the viewer may not read it.
        let workspace = match self.session.workspace {
            Some(id) => match self.views.workspace(id).await {
                Ok(workspace) => Some(workspace.snapshot().await),
                Err(_) => None,
            },
            None => None,
        };
        SessionSnapshot {
            id: self.session.id,
            name: self.session.name.to_string(),
            summary: self.session.summary.clone(),
            briefing: self.session.briefing.clone().filter(|_| detail.contains(Detail::BRIEFING)),
            privacy: self.session.privacy,
            approval_rules: self.session.approval_rules.clone(),
            can_change: self.may_change().await,
            participants,
            channels,
            workspace,
        }
    }
}
