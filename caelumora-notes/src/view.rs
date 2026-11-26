//! How agents' notes are reached (issue 607): by an agent's work, through its reader and its writer
//! (`crate::extension`), made by the plugin from the session of every piece of work as the work
//! reads it; and by whoever reads a channel or an agent, through the extensions of the kernel's
//! views - the channel's board (`AgentNotesBoard`), an agent's private notes (`SessionAgentNotes`).
//!
//! The board of a channel holds the notes of the agents taking part in it
//! ([581](../../issues/581-the-notes-of-an-agent-that-left-fall-off-the-board.md)), read as the
//! channel's: whoever reads the channel reads them, of a channel read entry by entry those released
//! to where they read from. An agent's private notes are read by itself - of the session's other
//! channels as far as each may be copied into where it reads from, reached through itself rather
//! than through the channels - and by whoever may read what it saw (`ReadAgentContext`), through
//! the channels they read. The documents are read in the kernel's name once the view decided; a
//! change is committed in the name of the work, checked: of the chains of the work's agent in the
//! work's channel alone, a new note of a number never given, a note changed one that stands, each
//! classified within the work, a reclassification as the policy lets the author.

use crate::extension::{NotesReader, NotesWriter};
use crate::manager::NotesManager;
use crate::note::{
    AgentNotes, Note, NoteChain, NoteChange, NoteId, NoteKind, PrivateNotesElsewhere,
    PublishedNotes,
};
use anyhow::anyhow;
use async_trait::async_trait;
use caelumora_api::reader::{ViewError, ViewResult};
use caelumora_api::staging::{CommitError, CommitResult, Committing};
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::confidentiality::{Classification, Place, Reading};
use caelumora_domain::digest::Digest;
use caelumora_domain::ledger::Change;
use caelumora_domain::message::MessageId;
use caelumora_domain::store::StoreResult;
use caelumora_kernel::view::{ChannelView, SessionView, Views};
use caelumora_security::{ActionType, Enforcement, Reclassified};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

/// The notes the plugin keeps, as the views reach them.
fn manager(views: &Views) -> ViewResult<&Arc<NotesManager>> {
    views.service::<NotesManager>().ok_or_else(|| {
        ViewError::Failed(anyhow!("no notes are kept: the kernel was built without their plugin"))
    })
}

/// What a channel's view lists of the notes its agents published: their board.
#[async_trait]
pub trait AgentNotesBoard {
    /// The notes the agents taking part published, as far as the viewer reads the channel - of a
    /// channel read entry by entry, those released to where the viewer reads from -, one entry for
    /// every agent taking part, in the channel's order.
    ///
    /// # Errors
    /// `Failed` if what an agent noted cannot be read; `NoChannelNamed` if nothing of a channel
    /// read entry by entry can be.
    async fn notes(&self) -> ViewResult<Vec<PublishedNotes>>;
}

#[async_trait]
impl AgentNotesBoard for ChannelView {
    async fn notes(&self) -> ViewResult<Vec<PublishedNotes>> {
        let manager = manager(self.views())?;
        let mut published = Vec::new();
        for participant in self.participants().await {
            let ActorId::Agent(agent) = participant.id() else { continue };
            let stored = manager.stored(agent, self.id()).await.map_err(|e| {
                ViewError::failed(e, format!("reading the notes of {agent} in {}", self.id()))
            })?;
            let notes = released(self, stored.public.current());
            published.push(PublishedNotes {
                agent: participant.id(),
                name: participant.name().map(ToString::to_string).unwrap_or_default(),
                notes,
                digest: stored.public.digest(),
            });
        }
        if *self.reading() != Reading::Whole && published.iter().all(|p| p.notes.is_empty()) {
            return Err(self.nothing_released());
        }
        Ok(published)
    }
}

/// An agent's private notes in one channel, as a viewer who may read what it saw reads them.
#[derive(Debug, Clone)]
pub struct PrivateNotesIn {
    pub channel: ChannelView,
    pub notes: Vec<Note>,
    /// The digest of the agent's private chain there.
    pub digest: Option<Digest>,
}

/// What a session's view lists of an agent's private notes, for whoever may read them.
#[async_trait]
pub trait SessionAgentNotes {
    /// The private notes `agent` wrote in the channels of the session the viewer reaches
    /// (`all_channels`, issues 51 and 233), sorted by the channel's name, channels without any left
    /// out: for a viewer who may read what the agent saw (`ReadAgentContext`), of a channel read
    /// entry by entry what was released. A channel whose notes cannot be read is left out, so that
    /// one broken document does not hide the others.
    ///
    /// # Errors
    /// `Forbidden` if the viewer may not read what the agent saw.
    async fn private_notes_of(&self, agent: AgentId) -> ViewResult<Vec<PrivateNotesIn>>;
}

#[async_trait]
impl SessionAgentNotes for SessionView {
    async fn private_notes_of(&self, agent: AgentId) -> ViewResult<Vec<PrivateNotesIn>> {
        let manager = manager(self.views())?;
        let mut found = Vec::new();
        for channel in self.all_channels().await {
            // Not a participant there: nothing noted there.
            let Ok(participant) = channel.participant(ActorId::Agent(agent)).await else {
                continue;
            };
            match participant.require_read_context().await {
                Ok(()) => {}
                Err(e @ ViewError::Forbidden { .. }) => return Err(e),
                Err(_) => continue,
            }
            match manager.stored(agent, channel.id()).await {
                Ok(stored) => {
                    let notes = released(&channel, stored.private.current());
                    if !notes.is_empty() {
                        let digest = stored.private.digest();
                        found.push(PrivateNotesIn { channel, notes, digest });
                    }
                }
                Err(e) => warn!(
                    "Reading the private notes of agent {} in channel {} failed: {}",
                    agent,
                    channel.id(),
                    e
                ),
            }
        }
        found.sort_by(|l, r| l.channel.name().as_str().cmp(r.channel.name().as_str()));
        Ok(found)
    }
}

/// Of `notes` of `channel`, those its viewer may read (`ChannelView::may_read_entry`).
fn released(channel: &ChannelView, notes: Vec<Note>) -> Vec<Note> {
    notes.into_iter().filter(|note| channel.may_read_entry(&note.classification)).collect()
}

/// The agent whose work reads `session`, and where the work reads from; `None` for a work of no
/// agent's, or that reads from no place.
fn work_of(session: &SessionView) -> Option<(AgentId, Place)> {
    let views = session.views();
    let ActorId::Agent(agent) = views.context().caller_id else { return None };
    Some((agent, views.viewer().place()?.clone()))
}

/// What a work reads of notes: the reader.
pub(crate) struct NotesRead {
    session: SessionView,
    agent: AgentId,
    here: ChannelId,
    /// The agent's notes in the work's channel, as they were stored when the reader was made.
    own: AgentNotes,
    manager: Arc<NotesManager>,
}

impl NotesRead {
    /// What the work that reads `session` reads of notes; `None` for a work of no agent's, that
    /// reads from no place, or whose agent's notes cannot be read - which is logged.
    pub(crate) async fn of_work(session: SessionView, manager: Arc<NotesManager>) -> Option<Self> {
        let (agent, here) = work_of(&session)?;
        let own = match manager.notes_of(session.views().context(), agent, here.channel).await {
            Ok(own) => own,
            Err(e) => {
                warn!(agent = %agent, "The notes of an agent could not be read: {e}");
                return None;
            }
        };
        Some(Self { session, agent, here: here.channel, own, manager })
    }
}

#[async_trait]
impl NotesReader for NotesRead {
    fn own(&self, kind: NoteKind) -> &NoteChain {
        self.own.chain(kind)
    }

    async fn published_in(&self, channel: ChannelId) -> ViewResult<Vec<PublishedNotes>> {
        self.session.channel(channel).await?.notes().await
    }

    async fn own_private_elsewhere(&self) -> Vec<PrivateNotesElsewhere> {
        let mut found = Vec::new();
        for channel in self.session.channels_taken_part_in(ActorId::Agent(self.agent)).await {
            if channel.id == self.here {
                continue;
            }
            let stored = match self.manager.stored(self.agent, channel.id).await {
                Ok(stored) => stored,
                Err(e) => {
                    warn!(
                        "Reading the private notes of agent {} in channel {} failed: {}",
                        self.agent, channel.id, e
                    );
                    continue;
                }
            };
            let views = self.session.views();
            let notes: Vec<Note> = stored
                .private
                .current()
                .into_iter()
                .filter(|note| {
                    let from = Place {
                        channel: channel.id,
                        classification: note.classification.clone(),
                        session: self.session.privacy(),
                    };
                    views.may_copy_from(&from)
                })
                .collect();
            if !notes.is_empty() {
                found.push(PrivateNotesElsewhere {
                    channel: channel.id,
                    name: channel.name.to_string(),
                    notes,
                    digest: stored.private.digest(),
                });
            }
        }
        found.sort_by(|l, r| l.name.cmp(&r.name));
        found
    }
}

/// What a turn of a work changes of its agent's notes, committed in the work's context: the
/// writer.
pub(crate) struct NotesUse {
    session: SessionView,
    agent: AgentId,
    /// Where the work reads from, whose channel's notes it changes.
    here: Place,
    manager: Arc<NotesManager>,
}

impl NotesUse {
    /// The notes the work that reads `session` changes, of its agent in its channel; `None` as for
    /// `NotesRead::of_work`.
    pub(crate) fn of_work(session: SessionView, manager: Arc<NotesManager>) -> Option<Self> {
        let (agent, here) = work_of(&session)?;
        Some(Self { session, agent, here, manager })
    }

    /// Refuses `classification` for what the work writes if it may not flow into the work's place.
    fn require_within(&self, classification: &Classification) -> CommitResult<()> {
        let views = self.session.views();
        let within = &self.here.classification;
        if views.confidentiality().may_publish(views.context(), classification, within) {
            return Ok(());
        }
        Err(CommitError::Invalid(format!("a note classified {classification} is above the work")))
    }

    /// The work's channel as the work reads it, running or not.
    ///
    /// # Errors
    /// `Unreachable` if it cannot be read.
    async fn channel(&self) -> CommitResult<ChannelView> {
        self.session.channel(self.here.channel).await.map_err(CommitError::Unreachable)
    }

    /// Refuses a note to be reclassified as `classification` unless the policy lets its author -
    /// the work's agent - (`Reclassified::Note`), with the attributes of `channel`, the work's,
    /// and the classification is within the work's.
    async fn require_reclassify(
        &self,
        channel: &ChannelView,
        classification: &Classification,
    ) -> CommitResult<()> {
        let views = self.session.views();
        let attributes = &channel.document().security_attributes;
        let author = ActorId::Agent(self.agent);
        let reclassify = ActionType::Reclassify(Reclassified::Note { author, attributes });
        let what = || format!("reclassifying a note as {classification}");
        views
            .access_policy()
            .require(views.context(), reclassify, |refused| CommitError::Refused {
                what: what(),
                refused,
            })
            .await?;
        self.require_within(classification)
    }

    /// Refuses `changes` unless every one is the work's to make on `notes` (see `commit`).
    async fn check(
        &self,
        channel: &ChannelView,
        notes: &AgentNotes,
        changes: &[NoteChange],
    ) -> CommitResult<()> {
        let mut standing: HashSet<(NoteKind, NoteId)> = HashSet::new();
        for kind in [NoteKind::Public, NoteKind::Private] {
            standing.extend(notes.chain(kind).current().into_iter().map(|note| (kind, note.id)));
        }
        let mut added: HashSet<(NoteKind, NoteId)> = HashSet::new();
        let changing = |standing: &HashSet<(NoteKind, NoteId)>, kind, id: &NoteId| {
            if standing.contains(&(kind, *id)) {
                return Ok(());
            }
            Err(CommitError::Invalid(format!("there is no note #{id} to change")))
        };
        for NoteChange { kind, change } in changes {
            let kind = *kind;
            match change {
                Change::Add { id, classification, .. } => {
                    let at = (self.agent, self.here.channel);
                    let reserved = self.manager.is_reserved(at, kind, *id).await?;
                    if notes.chain(kind).has_given(*id) || !added.insert((kind, *id)) || !reserved {
                        return Err(CommitError::Invalid(format!("note #{id} is no new one")));
                    }
                    self.require_within(classification)?;
                    standing.insert((kind, *id));
                }
                Change::Update { id, classification, .. } => {
                    changing(&standing, kind, id)?;
                    self.require_within(classification)?;
                }
                Change::Remove { id } => {
                    changing(&standing, kind, id)?;
                    standing.remove(&(kind, *id));
                }
                Change::Reclassify { id, classification } => {
                    changing(&standing, kind, id)?;
                    self.require_reclassify(channel, classification).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Committing<NoteChange> for NotesUse {
    /// Applies `changes` to the notes of the work's agent in the work's channel as they are stored
    /// now, if every one is the work's to make: a new note of a number never given in its chain,
    /// nor added before in `changes`; a note changed that stands or was added before in `changes`;
    /// each classified within the work, and reclassified as the policy lets its author. Judged
    /// under the lock of the notes, on the notes as `NotesManager::apply` reads them there - again,
    /// where another process changed them meanwhile. Announced to the readers of the channel.
    async fn commit(&self, changes: Vec<NoteChange>, after: MessageId) -> CommitResult<()> {
        let views = self.session.views();
        let channel = self.channel().await?;
        let at = (self.agent, self.here.channel);
        self.manager
            .apply(views.context(), at, after, channel.document(), |notes| {
                let (channel, changes) = (&channel, changes.clone());
                async move {
                    self.check(channel, &notes, &changes).await?;
                    Ok(changes)
                }
            })
            .await
    }
}

#[async_trait]
impl NotesWriter for NotesUse {
    async fn reserve_id(&self, kind: NoteKind) -> StoreResult<NoteId> {
        self.manager.reserve_id(self.agent, self.here.channel, kind).await
    }

    async fn may_reclassify(&self, classification: &Classification) -> bool {
        match self.channel().await {
            Ok(channel) => self.require_reclassify(&channel, classification).await.is_ok(),
            Err(_) => false,
        }
    }
}
