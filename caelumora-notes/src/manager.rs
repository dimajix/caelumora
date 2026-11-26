//! `NotesManager`: the one keeper of agents' notes (issue 607) - it reads the notes of an agent in
//! a channel, applies the changes of a turn to them one commit after another, and removes them with
//! their agent and with their channel.
//!
//! One actor of an agent writes its notes in a channel, but its turns in the channel's threads may
//! run at the same time - one set aside, waiting for a person, while another is taken. The manager
//! keeps, per agent and channel it serves, the number the next note of each chain gets and a lock
//! under which one commit after another is checked and applied to the notes as they are stored
//! then, so that no turn overwrites another's changes. A number is handed out when a turn notes,
//! not when it completes, so that two turns never give the same; it begins from the notes as
//! stored, read in the kernel's name - a number tells nothing of what is noted -, and is raised to
//! what a commit finds stored, so that it never falls behind what another process wrote. A commit
//! stored is announced, without what it changed, to whoever may read the document - the agent's
//! owner -, as every writer of a document announces what it stored, and to whoever reads the
//! channel, as the channel's `NOTES_CHANGED`, so that whoever shows its board reads it again.

use crate::note::{AgentNotes, AgentNotesId, NoteChange, NoteId, NoteKind, NotesField};
use crate::NOTES_CHANGED;
use async_trait::async_trait;
use caelumora_domain::agent::{Agent, AgentId};
use caelumora_domain::channel::{Channel, ChannelId};
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::message::MessageId;
use caelumora_domain::query::{Filter, Query};
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_kernel::behind::KeptBehind;
use caelumora_kernel::event::EventBus;
use caelumora_kernel::lifecycle::Lifecycle;
use caelumora_security::SecurityContext;
use caelumora_store::EntityStore;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};
use tracing::debug;

/// The one keeper of agents' notes.
pub(crate) struct NotesManager {
    /// The notes made, and purged with their agents and their channels, each made announced.
    notes: Lifecycle<AgentNotes>,
    /// Where a commit is announced.
    events: Arc<EventBus>,
    /// What is kept of the notes of each agent in each channel served since the process started:
    /// their numbers and their lock.
    kept: Mutex<HashMap<AgentNotesId, Arc<Kept>>>,
}

/// What the manager keeps of the notes of one agent in one channel.
#[derive(Default)]
struct Kept {
    /// The number the next published note gets, once begun.
    next_public: AtomicU32,
    /// The number the next private note gets, once begun.
    next_private: AtomicU32,
    /// Set once the numbers were begun from the notes as stored.
    begun: OnceCell<()>,
    /// Held while a commit is checked, applied and stored.
    write: Mutex<()>,
}

impl Kept {
    /// The number the next note of `kind` gets.
    fn next(&self, kind: NoteKind) -> &AtomicU32 {
        match kind {
            NoteKind::Public => &self.next_public,
            NoteKind::Private => &self.next_private,
        }
    }

    /// Raises the numbers to what `notes` gave already.
    fn raise_to(&self, notes: &AgentNotes) {
        for kind in [NoteKind::Public, NoteKind::Private] {
            self.next(kind).fetch_max(notes.chain(kind).next_id().0, Ordering::SeqCst);
        }
    }
}

impl fmt::Debug for NotesManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotesManager").finish_non_exhaustive()
    }
}

impl NotesManager {
    /// The keeper of the notes `notes` holds, announcing what it stores on `events`.
    pub(crate) fn new(notes: &Arc<dyn EntityStore<AgentNotes>>, events: Arc<EventBus>) -> Self {
        let lifecycle = Lifecycle::new(notes, Arc::<EventBus>::clone(&events));
        Self { notes: lifecycle, events, kept: Mutex::new(HashMap::new()) }
    }

    /// The notes of `agent` in `channel` as they are stored, read in `context`; while it notes
    /// nothing, empty ones owned by the principal `context` answers to - the agent's owner -, as
    /// they are made once it notes something (`apply`).
    ///
    /// # Errors
    /// Whatever the store reports, but that there are none.
    pub(crate) async fn notes_of(
        &self,
        context: &SecurityContext,
        agent: AgentId,
        channel: ChannelId,
    ) -> StoreResult<AgentNotes> {
        let id = AgentNotesId::of(agent, channel);
        let stored = self.notes.store().get(context, id).await.optional()?;
        Ok(stored.unwrap_or_else(|| AgentNotes::new(agent, channel, context.principal_id)))
    }

    /// The notes of `agent` in `channel` as they are stored, read in the kernel's name: for the
    /// views, which decided already whom they are read for - the channel's board, the private
    /// notes its owner reads through the channels, those an agent knows of its other channels.
    ///
    /// # Errors
    /// As `notes_of`.
    pub(crate) async fn stored(
        &self,
        agent: AgentId,
        channel: ChannelId,
    ) -> StoreResult<AgentNotes> {
        self.notes_of(&SecurityContext::system(), agent, channel).await
    }

    /// A number for a new note of `kind` of `agent` in `channel`, never handed out before.
    ///
    /// # Errors
    /// Whatever the store reports as the notes are read to begin the numbers.
    pub(crate) async fn reserve_id(
        &self,
        agent: AgentId,
        channel: ChannelId,
        kind: NoteKind,
    ) -> StoreResult<NoteId> {
        let kept = self.kept(agent, channel).await?;
        Ok(NoteId(kept.next(kind).fetch_add(1, Ordering::SeqCst)))
    }

    /// Whether `id` was handed out for a new note of `kind` of `agent` in `channel`
    /// (`reserve_id`).
    ///
    /// # Errors
    /// As `reserve_id`.
    pub(crate) async fn is_reserved(
        &self,
        (agent, channel): (AgentId, ChannelId),
        kind: NoteKind,
        id: NoteId,
    ) -> StoreResult<bool> {
        let kept = self.kept(agent, channel).await?;
        Ok(id.0 < kept.next(kind).load(Ordering::SeqCst))
    }

    /// Has `check` judge the notes of `agent` in `channel` as they are stored now - a copy -, read
    /// in `context` under their lock, and applies the changes it answers, made in a turn that ended
    /// with message `after`, and stores them, in `context`: one commit after another. Notes not
    /// stored yet are made (`notes_of`). Changed meanwhile by another process
    /// (`StoreError::Outdated`, issue 492) - or made, where they were not stored yet -, the notes
    /// are read again and judged again, as every writer of a document writes. Nothing is changed if
    /// `check` refuses or storing fails. What is stored is announced: `created` for notes made,
    /// `updated` without what changed for notes changed, and to the readers of `in_channel` - the
    /// channel's document - as its `NOTES_CHANGED`.
    ///
    /// # Errors
    /// What `check` answers, or whatever the store reports - a refusal; `Conflict` for notes that
    /// are stored but cannot be read in `context`.
    pub(crate) async fn apply<E, Judged>(
        &self,
        context: &SecurityContext,
        (agent, channel): (AgentId, ChannelId),
        after: MessageId,
        in_channel: &Channel,
        mut check: impl FnMut(AgentNotes) -> Judged,
    ) -> Result<(), E>
    where
        E: From<StoreError>,
        Judged: Future<Output = Result<Vec<NoteChange>, E>>,
    {
        let kept = self.kept(agent, channel).await?;
        let _writing = kept.write.lock().await;
        let mut taken = false;
        loop {
            let mut notes = self.notes_of(context, agent, channel).await?;
            // What was never stored carries the revision before the first.
            let made = notes.revision == Default::default();
            if made && taken {
                let unread = format!("the notes of agent {agent} cannot be read to change them");
                return Err(StoreError::Conflict(unread).into());
            }
            kept.raise_to(&notes);
            let changes = check(notes.clone()).await?;
            if changes.is_empty() {
                return Ok(());
            }
            let count = changes.len();
            for NoteChange { kind, change } in changes {
                notes.chain_mut(kind).push(change, (), after);
            }
            // Made, they are announced by their lifecycle; changed, here, without what changed.
            let written = if made {
                self.notes.create(context, &mut notes).await.created()?
            } else {
                let written = self.notes.store().update(context, &mut notes).await.written()?;
                if written.is_some() {
                    let event = GlobalEvent::updated::<AgentNotes>(&notes.address(), &());
                    self.events.announce(event, &notes);
                }
                written
            };
            if written.is_some() {
                debug!("Applied {} changes to the notes of agent {} in {}", count, agent, channel);
                let address = (in_channel.session_id, in_channel.id);
                let changed = GlobalEvent::custom::<Channel>(&address, NOTES_CHANGED, &agent);
                self.events.announce(changed, in_channel);
                return Ok(());
            }
            taken = made;
        }
    }

    /// Removes, in the kernel's name, the notes `filter` finds - all of them, no limit asked -,
    /// also those a purge that failed before marked deleted already, so that it is tried again.
    async fn purge_all(&self, filter: Filter<NotesField>) -> StoreResult<()> {
        let system = SecurityContext::system();
        let query = Query::filtered(filter);
        let store = self.notes.store();
        for notes in store.list(&system, &query).await?.items {
            self.kept.lock().await.remove(&notes.id);
            self.notes.purge_for_good(notes.id).await?;
        }
        for deleted in store.list_deleted(&system, &query).await?.items {
            store.purge(&system, deleted.entity.id).await?;
        }
        Ok(())
    }

    /// What is kept of the notes of `agent` in `channel`, its numbers begun from the notes as
    /// stored, read in the kernel's name once.
    async fn kept(&self, agent: AgentId, channel: ChannelId) -> StoreResult<Arc<Kept>> {
        let id = AgentNotesId::of(agent, channel);
        let kept = Arc::clone(self.kept.lock().await.entry(id).or_default());
        kept.begun
            .get_or_try_init(async || {
                kept.raise_to(&self.stored(agent, channel).await?);
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(kept)
    }
}

/// An agent's notes go with it: removed, in the kernel's name, in every channel, when the agent is
/// purged.
#[async_trait]
impl KeptBehind<Agent> for NotesManager {
    async fn remove_behind(&self, agent: AgentId) -> StoreResult<()> {
        self.purge_all(Filter::eq(NotesField::Agent, agent)).await
    }
}

/// The notes noted in a channel go with it: removed, in the kernel's name, of every agent, when
/// the channel is purged with its session.
#[async_trait]
impl KeptBehind<Channel> for NotesManager {
    async fn remove_behind(&self, channel: ChannelId) -> StoreResult<()> {
        self.purge_all(Filter::eq(NotesField::Channel, channel)).await
    }
}
