//! A session as its viewer reached it: its document, and its loop if it
//! runs.

use caelumora_model::store::query::{Filter, Query};
use super::{ChannelView, ViewError, ViewResult, Views};
use crate::channel::ChannelLoop;
use crate::session::{Found, SessionLoop};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::{Channel, ChannelField, Participant};
use caelumora_model::value::language::Language;
use caelumora_model::value::time_zone::TimeZone;
use caelumora_model::value::text::{Briefing, Summary};
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::confidentiality::{Reading, Place};
use caelumora_model::security::{AccessQuestions, ActionType, SecurityContext};
use super::{channel_reading, ChannelReader};
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::snapshot::{CandidateRefusal, CandidateSnapshot, DeletedSnapshot, Detail, ParticipantSnapshot, SessionSnapshot};
use caelumora_model::scope::ScopeField;
use caelumora_model::store::{StoreError, Stored};
use caelumora_model::user::UserField;
use crate::channel::ChannelError;
use std::sync::Arc;
use tracing::warn;
use anyhow::Error;

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

/// The least of a name one searches by (`SessionView::candidates`): fewer
/// characters would list nearly everyone.
const MIN_PREFIX: usize = 2;

/// The most users, and the most agents, a search finds.
const CANDIDATES: usize = 20;

/// Why someone cannot be taken in, as `error` of `resolve_to_take_in` says it;
/// `None` for an error that says nothing about them - a store that failed -,
/// which a search does not show.
fn refusal_of(error: &Error) -> Option<CandidateRefusal> {
    match (error.downcast_ref::<ChannelError>(), error.downcast_ref::<StoreError>()) {
        (Some(ChannelError::NotCleared { .. }), _) => Some(CandidateRefusal::NotCleared),
        (Some(ChannelError::OutOfScope { .. }), _) => Some(CandidateRefusal::OutOfScope),
        (_, Some(StoreError::PermissionDenied(_))) => Some(CandidateRefusal::CannotRun),
        _ => None,
    }
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

    /// The language its channels are held in where they hold none of their
    /// own (issue 425), if any.
    pub fn language(&self) -> Option<&Language> {
        self.session.language.as_ref()
    }

    /// The time zone the session is held in (issue 424), `None` for UTC.
    pub fn time_zone(&self) -> Option<TimeZone> {
        self.session.time_zone
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
        self.views.access_policy().may_change_session(self.views.context(), &self.session.security_attributes).await
    }

    /// Whom the viewer could take into the session and, with `channel`,
    /// into that channel of it (issue 221): the users and the agents they
    /// may read whose names begin with `prefix` - as names are compared, at
    /// least `MIN_PREFIX` characters of it, at most `CANDIDATES` of each -,
    /// each with whether they take part already and why they could not be
    /// taken in, as `SessionLoop::resolve_to_take_in` decides. Who may not
    /// change the session searches nobody: the search tells who exists.
    /// Nobody either for a session that does not run.
    ///
    /// # Errors
    /// `NotPermitted` if the viewer may not change the session;
    /// `ChannelNotFound` for a channel they may not read; `Failed` if the
    /// stores cannot list them.
    pub async fn candidates(&self, prefix: &str, channel: Option<ChannelId>) -> ViewResult<Vec<CandidateSnapshot>> {
        if !self.may_change().await {
            return Err(ViewError::NotPermitted(ActionType::update(Session::KIND, &self.session.security_attributes).refused()));
        }
        let channel = match channel {
            Some(id) => Some(self.channel(id).await?),
            None => None,
        };
        let Some(session_loop) = &self.running else { return Ok(Vec::new()) };
        if prefix.trim().chars().count() < MIN_PREFIX {
            return Ok(Vec::new());
        }
        let context = self.views.context();
        let stores = self.views.stores();
        let users = stores.users.list(context, &Query::filtered(Filter::name_prefix(UserField::Username, prefix)).limited(CANDIDATES)).await
            .map_err(|e| ViewError::failed(e, "searching users"))?.items;
        let agents = stores.agents.list(context, &Query::filtered(Filter::name_prefix(ScopeField::Name, prefix)).limited(CANDIDATES)).await
            .map_err(|e| ViewError::failed(e, "searching agents"))?.items;
        let found = users.into_iter().map(|user| (Participant::User(user.id), user.username.to_string()))
            .chain(agents.into_iter().map(|agent| (Participant::Agent(agent.id), agent.name.name.to_string())));
        let running = channel.as_ref().and_then(|channel| channel.running().cloned());
        let mut candidates = Vec::new();
        for (participant, name) in found {
            let id = participant.actor_id();
            let in_session = self.session.participants.iter().any(|p| p.actor_id() == id);
            let in_channel = channel.as_ref().is_some_and(|channel| channel.lists(id));
            let refusal = match in_channel {
                true => None,
                false => session_loop.resolve_to_take_in(context, &participant, running.as_ref()).await.err().and_then(|error| refusal_of(&error)),
            };
            candidates.push(CandidateSnapshot { id, name, in_session, in_channel, refusal });
        }
        Ok(candidates)
    }

    /// The channels the viewer may read as a whole, in the order of the
    /// session: a user those they may read (`GetChannel`) and are cleared
    /// for (issue 133); an agent those its channel may read by their
    /// classifications (issue 17). A channel whose document cannot be read
    /// is left out, so that one broken channel does not hide its session.
    pub async fn channels(&self) -> Vec<ChannelView> {
        let mut readable = Vec::new();
        for (channel, running) in self.channel_documents().await {
            if self.reading(&channel).await == Some(Reading::Whole) {
                readable.push(ChannelView::of(self.clone(), channel, running, Reading::Whole));
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
            if let Some(reading) = self.reading(&channel).await {
                reachable.push(ChannelView::of(self.clone(), channel, running, reading));
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
        let reading = self.reading(&channel).await.ok_or_else(not_found)?;
        Ok(ChannelView::of(self.clone(), channel, running, reading))
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
        let reading = self.reading(&channel).await.ok_or_else(not_found)?;
        Ok(ChannelView::of(self.clone(), channel, running, reading))
    }

    /// How the viewer may read `channel`: as a whole, entry by entry, or
    /// not at all (`None`) - asked as `channel_reading` asks it, an agent
    /// reading from its place, a user by the roles of their context.
    /// Whoever may not read the channel - its owner, its participants, an
    /// agent's owner or the agent itself (issue 264) - reads none of it
    /// (`GetChannel`).
    pub(super) async fn reading(&self, channel: &Channel) -> Option<Reading> {
        let at = Place { channel: channel.id, classification: channel.classification.clone(), session: self.privacy() };
        let reader = match self.views.viewer().place() {
            Some(from) => ChannelReader::From(from),
            None => ChannelReader::Cleared,
        };
        channel_reading(self.views.access_policy().as_ref(), self.views.confidentiality().as_ref(), self.views.context(), &channel.security_attributes, &at, reader).await
    }

    /// The document of channel `id`, if the session has it, with its loop
    /// if it runs. Read in the runtime's name: reaching the session was
    /// permitted, and whether the channel may be read is asked of it.
    async fn document(&self, id: ChannelId) -> ViewResult<Option<Document>> {
        match &self.running {
            Some(session_loop) => Ok(session_loop.channel(id).map(|channel_loop| (Arc::clone(&channel_loop.channel()), Some(channel_loop)))),
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
    /// `ParticipantView::private_notes` reads it: for a viewer who may read what
    /// the agent saw, of a channel read entry by entry what was released.
    /// A channel whose notes cannot be read is left out, so that one broken
    /// state does not hide the others.
    ///
    /// # Errors
    /// `Forbidden` if the viewer may not read what the agent saw.
    pub async fn private_notes_of(&self, agent: AgentId) -> ViewResult<Vec<(ChannelView, Vec<Note>, Option<Digest>)>> {
        let mut found = Vec::new();
        for channel in self.all_channels().await {
            // Not a participant there: nothing noted there.
            let Ok(participant) = channel.participant(ActorId::Agent(agent)).await else { continue };
            match participant.private_notes().await {
                Ok((notes, digest)) if !notes.is_empty() => found.push((channel, notes, digest)),
                Ok(_) => {}
                Err(e @ ViewError::Forbidden { .. }) => return Err(e),
                Err(e) => warn!("Reading the private notes of agent {} in channel {} failed: {}", agent, channel.id(), e),
            }
        }
        found.sort_by(|(l, ..), (r, ..)| l.name().as_str().cmp(r.name().as_str()));
        Ok(found)
    }

    /// The channels of the session the viewer deleted and may read as such (issue 164). One may be
    /// restored by whoever may restore it and change the session (`SessionLoop::restore_channel`);
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
                can_restore: may_change && views.access_policy().may_restore(context, Channel::KIND, &channel.security_attributes).await,
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
            language: self.session.language.clone(),
            time_zone: self.session.time_zone,
            privacy: self.session.privacy,
            approval_rules: self.session.approval_rules.clone(),
            can_change: self.may_change().await,
            participants,
            channels,
            workspace,
        }
    }
}
