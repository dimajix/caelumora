use crate::actor::ActorId;
use crate::agent::{AgentChannelState, AgentId, AgentState};
use crate::chain::{Node, Position};
use crate::channel::{ChannelId, ChannelState};
use crate::context::FrameRef;
use crate::digest::Digest;
use crate::history::{History, HistoryEntry, HistoryPage, Reader, Recorded};
use crate::message::{Message, MessageId};
use crate::security::Admission;
use crate::store::documents::{Document, DocumentOf, DocumentStorage};
use crate::store::history::{
    answer_of, page_of, seen, seen_nodes, HistoryField, HistoryStore, Memberships, StandingItems,
};
use crate::store::kept_chain::KeptChain;
use crate::store::query::{ordered_after, FieldPath, Filter};
use crate::store::transcript::{load_named_by_turn, transcript_of, TranscriptStore};
use crate::store::{
    EntityKind, Journaled, PersistenceBackend, Presence, Revision, StateStore, StoreError,
    StoreResult,
};
use crate::tool::ToolSet;
use crate::transcript::{known, Transcript, TranscriptEntry};
use crate::user::{UserChannelState, UserId};
use crate::value::name::ScopedName;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem;
use std::ops::Range;
use std::sync::{Arc, RwLock};
use tracing::info;
use uuid::Uuid;

/// A backend in memory (issue 446): the documents of every kind, a storage
/// per kind (`MemoryDocuments`) opened when it is first asked for, and maps
/// for the states - their documents, and apart their histories, each whole
/// with its items as they stand (issue 379), the agents' transcripts, each
/// whole (issue 380), and the prompt texts and tool sets their models were
/// given (issue 383).
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<MemoryDocuments>>>,
    agent_states: RwLock<HashMap<AgentId, AgentState>>,
    agent_channel_states: RwLock<HashMap<(AgentId, ChannelId), AgentChannelState>>,
    channel_states: RwLock<HashMap<ChannelId, ChannelState>>,
    user_channel_states: RwLock<HashMap<(UserId, ChannelId), UserChannelState>>,
    histories: RwLock<HashMap<ChannelId, KeptHistory>>,
    transcripts: RwLock<HashMap<(AgentId, ChannelId), KeptChain<TranscriptEntry>>>,
    given: RwLock<HashMap<(AgentId, ChannelId), KeptGiven>>,
}

/// The prompt texts, the tool sets and the frames an agent's model was
/// given in a channel, as the memory store keeps them: every one, each once.
#[derive(Debug, Default)]
struct KeptGiven {
    prompts: BTreeMap<Digest, String>,
    offered_tools: BTreeMap<Digest, Vec<ToolSet>>,
    frames: BTreeMap<Digest, FrameRef>,
}

/// A history as the memory store keeps it: its chain whole, and the items
/// and the memberships derived from it.
#[derive(Debug, Default)]
struct KeptHistory {
    chain: KeptChain<Recorded>,
    items: StandingItems,
    memberships: Memberships,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        info!("Creating memory store");
        Self::default()
    }

    /// Appends what `history`, that of `channel` as its state holds it,
    /// added since it was last stored, derives the items of it, and marks it
    /// stored.
    fn append_history(&self, channel: ChannelId, history: &mut History) -> StoreResult<()> {
        let Some(changes) = history.changes() else { return Ok(()) };
        write_lock(&self.histories, |all| {
            let kept = all.entry(channel).or_default();
            let end = kept.chain.end();
            kept.chain.append(changes).map_err(|e| {
                StoreError::Backend(format!(
                    "what the history of channel {channel} added does not fit: {e}"
                ))
            })?;
            for node in kept.chain.from(end) {
                kept.items.take(node);
                kept.memberships.take(node);
            }
            Ok::<_, StoreError>(())
        })?;
        history.mark_stored();
        Ok(())
    }

    /// The history of `channel` from the position `from` on, as a holder
    /// keeps it; empty if none is kept.
    fn load_history(&self, channel: ChannelId, from: Position) -> StoreResult<History> {
        read_lock(&self.histories, |all| {
            let Some(kept) = all.get(&channel) else { return Ok(History::default()) };
            kept.chain.suffix(from).map_err(|e| {
                StoreError::Backend(format!(
                    "the history of channel {channel} cannot be read from position {from}: {e}"
                ))
            })
        })
    }

    /// Appends what `transcript`, that of `agent` in `channel` as its state
    /// holds it, added since it was last stored, and marks it stored.
    fn append_transcript(
        &self,
        agent: AgentId,
        channel: ChannelId,
        transcript: &mut Transcript,
    ) -> StoreResult<()> {
        let Some(changes) = transcript.changes() else { return Ok(()) };
        write_lock(&self.transcripts, |all| {
            all.entry((agent, channel)).or_default().append(changes).map_err(|e| {
                StoreError::Backend(format!(
                    "what {} added does not fit: {e}",
                    transcript_of(agent, channel)
                ))
            })
        })?;
        transcript.mark_stored();
        Ok(())
    }

    /// Keeps the prompt texts, the tool sets and the frames `state`, that of
    /// `agent` in `channel`, kept since they were last stored, and marks them
    /// stored.
    fn append_given(&self, agent: AgentId, channel: ChannelId, state: &mut AgentChannelState) {
        write_lock(&self.given, |all| {
            let kept = all.entry((agent, channel)).or_default();
            for (digest, text) in state.prompts.changes() {
                kept.prompts.entry(digest).or_insert(text);
            }
            for (digest, sets) in state.offered_tools.changes() {
                kept.offered_tools.entry(digest).or_insert(sets);
            }
            for (digest, frame) in state.frames.changes() {
                kept.frames.entry(digest).or_insert(frame);
            }
        });
        state.prompts.mark_stored();
        state.offered_tools.mark_stored();
        state.frames.mark_stored();
    }

    /// The transcript of `agent` in `channel` from the position `from` on,
    /// as its state holds it; empty if none is kept.
    fn load_transcript(
        &self,
        agent: AgentId,
        channel: ChannelId,
        from: Position,
    ) -> StoreResult<Transcript> {
        read_lock(&self.transcripts, |all| {
            let Some(kept) = all.get(&(agent, channel)) else { return Ok(Transcript::default()) };
            kept.suffix(from).map_err(|e| {
                StoreError::Backend(format!(
                    "{} cannot be read from position {from}: {e}",
                    transcript_of(agent, channel)
                ))
            })
        })
    }

    /// `state`'s document - what is stored of it but its history - as a
    /// copy without the history, which is kept apart.
    fn document(state: &mut ChannelState) -> ChannelState {
        let held = mem::take(&mut state.history);
        let document = state.clone();
        state.history = held;
        document
    }

    /// `state`'s document - what is stored of it but its transcript, its
    /// prompt texts, its tool sets and its frames - as a copy without them,
    /// which are kept apart.
    fn agent_document(state: &mut AgentChannelState) -> AgentChannelState {
        let transcript = mem::take(&mut state.transcript);
        let prompts = mem::take(&mut state.prompts);
        let offered_tools = mem::take(&mut state.offered_tools);
        let frames = mem::take(&mut state.frames);
        let document = state.clone();
        state.transcript = transcript;
        state.prompts = prompts;
        state.offered_tools = offered_tools;
        state.frames = frames;
        document
    }
}

/// Opens the storage of a kind in memory, which cannot fail; a document is
/// kept with what it was told, so `document_of` is not needed. The store of
/// the states is the memory store itself.
impl PersistenceBackend for MemoryStore {
    fn documents(
        &self,
        kind: EntityKind,
        _document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        let storage = write_lock(&self.documents, |all| {
            Arc::clone(all.entry(kind).or_insert_with(|| Arc::new(MemoryDocuments::new(kind))))
        });
        Ok(storage)
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Ok(self)
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Ok(self)
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Ok(self)
    }
}

// A poisoned lock is used anyway: the locks are held only by the closures
// passed here, none of which can leave a map half-changed.
fn read_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&T) -> R,
{
    match lock.read() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(poisoned.get_ref()),
    }
}

fn write_lock<T, F, R>(lock: &RwLock<T>, f: F) -> R
where
    F: FnOnce(&mut T) -> R,
{
    match lock.write() {
        Ok(mut guard) => f(&mut guard),
        Err(mut poisoned) => f(poisoned.get_mut()),
    }
}

/// The documents of one kind in memory (issue 446), each kept as it was
/// told, by its id.
#[derive(Debug)]
struct MemoryDocuments {
    kind: EntityKind,
    documents: RwLock<HashMap<Uuid, Document>>,
}

impl MemoryDocuments {
    fn new(kind: EntityKind) -> Self {
        Self { kind, documents: Default::default() }
    }

    /// Refuses a write in place of the document `id` of `documents` unless
    /// it is at the revision `read` (issue 492).
    fn at_revision(
        &self,
        documents: &HashMap<Uuid, Document>,
        id: Uuid,
        read: Revision,
    ) -> StoreResult<()> {
        match documents.get(&id) {
            Some(stored) if stored.revision == read => Ok(()),
            _ => Err(StoreError::Outdated { entity: self.kind.name(), id: id.to_string() }),
        }
    }

    /// Refuses `document` if another of `documents` that is not deleted has
    /// its name (issue 250).
    fn name_free(
        &self,
        documents: &HashMap<Uuid, Document>,
        document: &Document,
    ) -> StoreResult<()> {
        let Some(name) = document.name.as_ref().filter(|_| document.removal_id.is_none()) else {
            return Ok(());
        };
        match documents.values().any(|other| {
            other.id != document.id
                && other.removal_id.is_none()
                && other.name.as_ref() == Some(name)
        }) {
            false => Ok(()),
            true => Err(StoreError::NameTaken { entity: self.kind.name() }),
        }
    }
}

#[async_trait]
impl DocumentStorage for MemoryDocuments {
    async fn get(&self, id: Uuid) -> StoreResult<Option<Value>> {
        Ok(read_lock(&self.documents, |all| all.get(&id).map(|document| document.body.clone())))
    }

    /// Narrowed by the admission as far as told without the groups
    /// (`Admission::may_admit`): an entry for a group lets a document
    /// through, for the store to decide.
    async fn scan(
        &self,
        filter: Option<&Filter<FieldPath>>,
        admission: Option<&Admission>,
        presence: Presence,
        after: Option<Uuid>,
        limit: usize,
    ) -> StoreResult<Vec<(Uuid, Value)>> {
        Ok(read_lock(&self.documents, |all| {
            let meeting = all
                .values()
                .filter(|document| {
                    presence.holds_mark(document.removal_id)
                        && filter.is_none_or(|filter| filter.matches(&document.body))
                })
                .filter(|document| {
                    admission
                        .is_none_or(|admission| admission.may_admit(&document.security_attributes))
                })
                .map(|document| (document.id, document))
                .collect();
            ordered_after(meeting, after)
                .take(limit)
                .map(|(id, document)| (id, document.body.clone()))
                .collect()
        }))
    }

    async fn named(&self, name: &ScopedName) -> StoreResult<Option<Value>> {
        Ok(read_lock(&self.documents, |all| {
            all.values()
                .find(|document| {
                    document.removal_id.is_none() && document.name.as_ref() == Some(name)
                })
                .map(|document| document.body.clone())
        }))
    }

    async fn insert(&self, document: Document) -> StoreResult<()> {
        write_lock(&self.documents, |all| {
            if all.contains_key(&document.id) {
                return Err(StoreError::Conflict(format!(
                    "{} {} already exists",
                    self.kind, document.id
                )));
            }
            self.name_free(all, &document)?;
            all.insert(document.id, document);
            Ok(())
        })
    }

    async fn replace(&self, document: Document, read: Revision) -> StoreResult<()> {
        write_lock(&self.documents, |all| {
            self.at_revision(all, document.id, read)?;
            self.name_free(all, &document)?;
            all.insert(document.id, document);
            Ok(())
        })
    }

    async fn remove(&self, id: Uuid, read: Revision) -> StoreResult<bool> {
        write_lock(&self.documents, |all| {
            if !all.contains_key(&id) {
                return Ok(false);
            }
            self.at_revision(all, id, read)?;
            Ok(all.remove(&id).is_some())
        })
    }
}

#[async_trait]
impl StateStore for MemoryStore {
    async fn get_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
    ) -> StoreResult<AgentChannelState> {
        let mut state =
            read_lock(&self.agent_channel_states, |v| v.get(&(agent_id, channel_id)).cloned())
                .unwrap_or_default();
        state.transcript = self.load_transcript(agent_id, channel_id, state.transcript_from)?;
        load_named_by_turn(self, agent_id, channel_id, &mut state).await?;
        Ok(state)
    }

    /// The prompt texts and the tool sets first, as the `JsonFileStore`
    /// writes them.
    async fn put_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
        state: &mut AgentChannelState,
    ) -> StoreResult<()> {
        self.append_given(agent_id, channel_id, state);
        self.append_transcript(agent_id, channel_id, &mut state.transcript)?;
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        let document = Self::agent_document(state);
        write_lock(&self.agent_channel_states, |v| v.insert((agent_id, channel_id), document));
        Ok(())
    }

    /// The states kept are the index: each knows whether it has work under
    /// way.
    async fn channels_with_work_under_way(&self) -> StoreResult<Vec<ChannelId>> {
        let channels: HashSet<ChannelId> = read_lock(&self.agent_channel_states, |v| {
            v.iter()
                .filter(|(_, state)| state.has_work_under_way())
                .map(|((_, channel_id), _)| *channel_id)
                .collect()
        });
        Ok(channels.into_iter().collect())
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        Ok(read_lock(&self.agent_states, |v| v.get(&agent_id).cloned()).unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &mut AgentState) -> StoreResult<()> {
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        write_lock(&self.agent_states, |v| v.insert(agent_id, state.clone()));
        Ok(())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let mut state =
            read_lock(&self.channel_states, |v| v.get(&channel_id).cloned()).ok_or_else(|| {
                StoreError::NotFound { entity: "channel_state", id: channel_id.to_string() }
            })?;
        state.history = self.load_history(channel_id, state.history_from)?;
        Ok(state)
    }

    async fn put_channel_state(
        &self,
        channel_id: ChannelId,
        state: &mut ChannelState,
    ) -> StoreResult<()> {
        self.append_history(channel_id, &mut state.history)?;
        // Marked first: the copy kept is the state as stored, with nothing pending.
        state.mark_stored();
        let document = Self::document(state);
        write_lock(&self.channel_states, |v| v.insert(channel_id, document));
        Ok(())
    }

    async fn get_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
    ) -> StoreResult<UserChannelState> {
        Ok(read_lock(&self.user_channel_states, |v| v.get(&(user_id, channel_id)).cloned())
            .unwrap_or_default())
    }

    async fn put_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
        state: &mut UserChannelState,
    ) -> StoreResult<()> {
        state.mark_stored();
        write_lock(&self.user_channel_states, |v| v.insert((user_id, channel_id), state.clone()));
        Ok(())
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        write_lock(&self.channel_states, |v| v.remove(&channel_id));
        write_lock(&self.user_channel_states, |v| v.retain(|(_, cid), _| *cid != channel_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(_, cid), _| *cid != channel_id));
        write_lock(&self.histories, |v| v.remove(&channel_id));
        write_lock(&self.transcripts, |v| v.retain(|(_, cid), _| *cid != channel_id));
        write_lock(&self.given, |v| v.retain(|(_, cid), _| *cid != channel_id));
        Ok(())
    }

    async fn delete_agent_state(&self, agent_id: AgentId) -> StoreResult<()> {
        write_lock(&self.agent_states, |v| v.remove(&agent_id));
        write_lock(&self.agent_channel_states, |v| v.retain(|(aid, _), _| *aid != agent_id));
        write_lock(&self.transcripts, |v| v.retain(|(aid, _), _| *aid != agent_id));
        write_lock(&self.given, |v| v.retain(|(aid, _), _| *aid != agent_id));
        Ok(())
    }
}

#[async_trait]
impl HistoryStore for MemoryStore {
    async fn page(
        &self,
        channel: ChannelId,
        reader: &Reader,
        from: Position,
        before: Option<Position>,
        limit: usize,
        filter: Option<&Filter<HistoryField>>,
    ) -> StoreResult<HistoryPage> {
        read_lock(&self.histories, |all| match all.get(&channel) {
            Some(kept) => {
                let ranges = kept.memberships.of_reader(reader);
                let newest = kept
                    .items
                    .newest_before(before.unwrap_or(Position::MAX))
                    .take_while(|(seq, _)| *seq >= from);
                page_of(
                    newest.map(|item| seen(reader, &ranges, filter, item)),
                    from,
                    limit,
                    kept.chain.head(),
                )
            }
            None => Ok(HistoryPage::default()),
        })
    }

    async fn said(
        &self,
        channel: ChannelId,
        reader: &Reader,
        id: MessageId,
    ) -> StoreResult<Option<(Position, HistoryEntry)>> {
        read_lock(&self.histories, |all| {
            match all.get(&channel).and_then(|kept| kept.items.said(id).map(|item| (kept, item))) {
                Some((kept, item)) => seen(reader, &kept.memberships.of_reader(reader), None, item),
                None => Ok(None),
            }
        })
    }

    async fn entries(
        &self,
        channel: ChannelId,
        reader: &Reader,
        range: Range<Position>,
    ) -> StoreResult<Vec<Node<Recorded>>> {
        Ok(read_lock(&self.histories, |all| {
            all.get(&channel)
                .map(|kept| {
                    let nodes =
                        kept.chain.from(range.start).iter().take_while(|node| node.seq < range.end);
                    seen_nodes(reader, &kept.memberships.of_reader(reader), nodes)
                })
                .unwrap_or_default()
        }))
    }

    async fn answer(
        &self,
        channel: ChannelId,
        reader: &Reader,
        end: Position,
    ) -> StoreResult<Vec<Message>> {
        Ok(read_lock(&self.histories, |all| {
            all.get(&channel)
                .and_then(|kept| {
                    let nodes = kept.chain.from(Position::START);
                    let at = usize::try_from(end.get()).ok().filter(|at| *at < nodes.len())?;
                    Some(answer_of(
                        &nodes[at].entry.entry,
                        nodes[..at].iter().rev(),
                        reader,
                        &kept.memberships.of_reader(reader),
                    ))
                })
                .unwrap_or_default()
        }))
    }

    async fn memberships(
        &self,
        channel: ChannelId,
        actor: ActorId,
    ) -> StoreResult<Vec<Range<Position>>> {
        Ok(read_lock(&self.histories, |all| {
            all.get(&channel).map(|kept| kept.memberships.of_member(actor)).unwrap_or_default()
        }))
    }

    async fn rebuild(&self, channel: ChannelId) -> StoreResult<()> {
        write_lock(&self.histories, |all| {
            if let Some(kept) = all.get_mut(&channel) {
                kept.items = StandingItems::of(kept.chain.from(Position::START));
                kept.memberships = Memberships::of(kept.chain.from(Position::START));
            }
        });
        Ok(())
    }
}

#[async_trait]
impl TranscriptStore for MemoryStore {
    async fn path(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<Vec<Node<TranscriptEntry>>>> {
        read_lock(&self.transcripts, |all| match all.get(&(agent, channel)) {
            Some(kept) => kept.path(digest).map_err(|e| {
                StoreError::Backend(format!(
                    "{} does not lead back to its root: {e}",
                    transcript_of(agent, channel)
                ))
            }),
            None => Ok(None),
        })
    }

    async fn message(
        &self,
        agent: AgentId,
        channel: ChannelId,
        id: MessageId,
    ) -> StoreResult<Option<Message>> {
        Ok(read_lock(&self.transcripts, |all| {
            all.get(&(agent, channel)).and_then(|kept| known(kept.from(Position::START).iter(), id))
        }))
    }

    async fn in_transcript(
        &self,
        agent: AgentId,
        channel: ChannelId,
        ids: &[MessageId],
    ) -> StoreResult<HashSet<MessageId>> {
        Ok(read_lock(&self.transcripts, |all| {
            all.get(&(agent, channel))
                .into_iter()
                .flat_map(|kept| kept.from(Position::START))
                .filter_map(|node| node.entry.message().map(|m| m.id))
                .filter(|id| ids.contains(id))
                .collect()
        }))
    }

    async fn prompts(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digests: &[Digest],
    ) -> StoreResult<BTreeMap<Digest, String>> {
        Ok(read_lock(&self.given, |all| {
            all.get(&(agent, channel))
                .into_iter()
                .flat_map(|kept| {
                    digests.iter().filter_map(|digest| {
                        kept.prompts.get(digest).map(|text| (*digest, text.clone()))
                    })
                })
                .collect()
        }))
    }

    async fn offered_tools(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<Vec<ToolSet>>> {
        Ok(read_lock(&self.given, |all| {
            all.get(&(agent, channel)).and_then(|kept| kept.offered_tools.get(&digest).cloned())
        }))
    }

    async fn frame(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<FrameRef>> {
        Ok(read_lock(&self.given, |all| {
            all.get(&(agent, channel)).and_then(|kept| kept.frames.get(&digest).cloned())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::GroupId;
    use crate::security::{Grant, OwnershipPolicy, SecurityAttributes, SecurityContext, Subject};
    use crate::store::query::Query;
    use crate::store::suite;
    use crate::store::{EntityStores, Persistence, Policies};
    use crate::user::UserId;

    /// The stores of a memory store, permitting everything.
    fn permitting() -> EntityStores {
        EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all()).unwrap()
    }

    /// The stores of a memory store, as the owners' policy checks them.
    fn owners() -> EntityStores {
        let policies = Policies::by_default_rules(Arc::new(OwnershipPolicy::new()));
        EntityStores::open(&Persistence::in_memory(), &policies).unwrap()
    }

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&permitting()).await;
    }

    /// A scan hands out what the admission may admit (issue 332): what
    /// the owner, a user or a role admits, and whatever names a group, for
    /// the store to decide - nothing else.
    #[tokio::test]
    async fn a_scan_is_narrowed_by_the_admission() {
        let documents = MemoryDocuments::new(EntityKind::new("folder"));
        let (alice, bob) = (ActorId::User(UserId::new()), ActorId::User(UserId::new()));
        let kept = [
            SecurityAttributes::owned_by(bob),
            SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Read),
            SecurityAttributes::owned_by(alice)
                .granting(Subject::Group(GroupId::new()), Grant::Read),
            SecurityAttributes::owned_by(alice),
            SecurityAttributes::owned_by(alice).granting(Subject::Actor(bob), Grant::Join),
        ];
        let mut ids = Vec::new();
        for security_attributes in kept {
            let id = Uuid::new_v4();
            documents
                .insert(Document {
                    id,
                    revision: Default::default(),
                    name: None,
                    removal_id: None,
                    security_attributes,
                    body: Value::Null,
                })
                .await
                .unwrap();
            ids.push(id);
        }
        let bobs =
            Admission { actors: vec![bob], grant: Some(Grant::Read), ..Admission::default() };
        let mut scanned: Vec<Uuid> = documents
            .scan(None, Some(&bobs), Presence::Present, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut admitted = ids[..3].to_vec();
        scanned.sort();
        admitted.sort();
        assert_eq!(
            scanned, admitted,
            "his own, shared with him, shared with a group - not hers, nor what lets him join"
        );
        assert_eq!(
            documents.scan(None, None, Presence::Present, None, 10).await.unwrap().len(),
            5,
            "without an admission, all of them"
        );
    }

    #[tokio::test]
    async fn lists_what_check_decides() {
        let (memberships, access_policy) = suite::admitting_policy();
        let store = Persistence::in_memory();
        let policies = Policies::by_default_rules(Arc::clone(&access_policy));
        let folders = store.entity_store::<suite::Folder>(&policies).unwrap();
        suite::lists_admit_what_check_decides(
            &EntityStores::open(&store, &policies).unwrap(),
            folders.as_ref(),
            &memberships,
            access_policy.as_ref(),
        )
        .await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let store = Persistence::in_memory();
        suite::queries_behave(&EntityStores::open(&store, &Policies::permitting_all()).unwrap())
            .await;
        suite::queries_reach_parts(
            store.entity_store::<suite::Folder>(&Policies::permitting_all()).unwrap().as_ref(),
        )
        .await;
    }

    /// A limit counts what the caller may read (issue 293): the policy is
    /// asked as the list is read, and a page is filled from further on.
    #[tokio::test]
    async fn a_limit_counts_what_the_caller_may_read() {
        use crate::actor::ActorId;
        use crate::confidentiality::Classification;
        use crate::scope::Scope;
        use crate::workspace::Workspace;
        let store = owners().workspaces;
        let (alice, bob) = (UserId::new(), UserId::new());
        let mut hers = Vec::new();
        for (index, owner) in [alice, bob, bob, alice, bob, alice].into_iter().enumerate() {
            let name =
                ScopedName::new(format!("w{index}"), Scope::UserScope { user_id: owner }).unwrap();
            let mut workspace =
                Workspace::new(name, "main", Classification::CHANNEL_DEFAULT, owner);
            store.create(&SecurityContext::system(), &mut workspace).await.unwrap();
            if owner == alice {
                hers.push(workspace.id);
            }
        }
        hers.sort();
        let ctx =
            SecurityContext::new(ActorId::User(alice), Default::default(), Default::default());
        let mut read = Vec::new();
        let mut query = Query::all().limited(1);
        loop {
            let page = store.list(&ctx, &query).await.unwrap();
            read.extend(page.items.iter().map(|w| w.id));
            match page.next {
                Some(cursor) => {
                    assert_eq!(
                        page.items.len(),
                        1,
                        "every page that goes on is full, however many of bob's lie between"
                    );
                    query = Query::all().limited(1).after(cursor);
                }
                // The last page may be empty: what followed was bob's.
                None => break,
            }
        }
        assert_eq!(read, hers, "hers alone, in the order of their ids");
    }

    #[tokio::test]
    async fn behaves_as_a_state_store() {
        suite::state_store_behaves(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_the_index_of_the_work_under_way() {
        suite::work_under_way_is_indexed(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn keeps_the_histories_of_the_states() {
        let store = MemoryStore::new();
        suite::history_store_behaves(&store, &store).await;
        suite::transcript_store_behaves(&store, &store).await;
        suite::given_store_behaves(&store, &store).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        suite::deleting_marks(&permitting()).await;
    }

    #[tokio::test]
    async fn writes_are_judged_on_their_revision() {
        suite::writes_are_judged_on_their_revision(&permitting()).await;
        suite::storage_judges_writes_on_their_revision(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn purges_are_begun() {
        suite::purges_are_begun(&permitting()).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        suite::names_are_unique(&permitting()).await;
        suite::names_are_unique_beyond_what_is_read(&owners()).await;
    }
}
