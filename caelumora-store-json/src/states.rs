//! The states of channels, agents and users as JSON files (issue 193), where a `StateLayout` puts
//! them (issue 554).
//!
//! A state is kept as a snapshot and a journal: the state as it was last written whole in
//! `<key>.json`, and what changed since as one line per put in `<key>.jsonl`. Loading reads the
//! snapshot and replays the journal; a put appends a line, or writes a new snapshot and empties the
//! journal once the journal is at least as large as the snapshot and `journal_floor`. What a crash
//! leaves is read as it is: a torn last line is dropped, and a journal that survived its snapshot
//! replays onto it without effect, since replaying is idempotent. A state's history and an agent's
//! transcript are kept apart, in segments beside the snapshot (`chains`; `histories`,
//! `transcripts`, issues 379, 380).

use super::layout::StateLayout;
use super::{delete_if_exists, make_parent, read_json_opt, to_backend_error, write_json_atomic};
use async_trait::async_trait;
use caelumora_domain::agent::{AgentChannelState, AgentId};
use caelumora_domain::channel::{ChannelId, ChannelState};
use caelumora_domain::store::{Journaled, StoreError, StoreResult};
use caelumora_domain::user::{UserChannelState, UserId};
use caelumora_store::transcript::load_named_by_turn;
use caelumora_store::StateStore;
use serde::Serialize;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

/// A journal is folded into its snapshot once it is at least as large as
/// the snapshot, and at least this large (issue 193): small states are not
/// written whole at every other put. `journal_floor` of the configuration.
pub const DEFAULT_JOURNAL_FLOOR: u64 = 64 * 1024;

/// The states, their histories and their transcripts in JSON files, where a layout puts them: the
/// three state stores of a backend over files.
#[derive(Debug)]
pub struct JsonStates {
    layout: Arc<dyn StateLayout>,
    journal_floor: u64,
}

impl JsonStates {
    /// The states where `layout` puts them, a journal folded into its snapshot once it is
    /// `journal_floor` large, if that is more than the snapshot.
    pub fn new(layout: Arc<dyn StateLayout>, journal_floor: u64) -> Self {
        Self { layout, journal_floor }
    }

    /// The snapshot of the state of `channel`.
    pub(super) async fn channel_state_path(&self, channel: ChannelId) -> StoreResult<PathBuf> {
        self.layout.channel_state(channel).await
    }

    /// The snapshot of the state of `agent` in `channel`.
    pub(super) async fn agent_channel_state_path(
        &self,
        agent: AgentId,
        channel: ChannelId,
    ) -> StoreResult<PathBuf> {
        Ok(self.layout.agents_in(channel).await?.join(format!("{agent}.json")))
    }

    /// The snapshot of the state of `user` in `channel`.
    async fn user_channel_state_path(
        &self,
        user: UserId,
        channel: ChannelId,
    ) -> StoreResult<PathBuf> {
        Ok(self.layout.users_in(channel).await?.join(format!("{user}.json")))
    }

    /// The entry of `agent` in `channel` in the index of the work under way (issue 388): an empty
    /// file `<channel_id>.<agent_id>`.
    fn under_way_path(&self, agent: AgentId, channel: ChannelId) -> PathBuf {
        self.layout.under_way().join(format!("{channel}.{agent}"))
    }
}

/// Creates the empty file `path` unless it exists, and the directory it lies in.
async fn create_if_missing(path: &Path) -> StoreResult<()> {
    make_parent(path).await?;
    match OpenOptions::new().write(true).create_new(true).open(path).await {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(to_backend_error(err)),
    }
}

async fn delete_dir_if_exists(path: &Path) -> StoreResult<bool> {
    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// The journal of the state whose snapshot is at `path`: `<key>.jsonl`.
fn journal_path(path: &Path) -> PathBuf {
    path.with_extension("jsonl")
}

/// The size of the file at `path`, `None` if there is none.
pub(super) async fn size_of(path: &Path) -> StoreResult<Option<u64>> {
    match fs::metadata(path).await {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

/// The state whose snapshot is at `path`, with its journal replayed onto
/// it; `None` if there is no snapshot. A last line the journal does not end
/// with a newline and that does not parse is what a crash during an append
/// left, and is dropped; any other line that does not parse, or does not
/// fit the state, is a corrupt state.
async fn load_state<T: Journaled>(path: &Path) -> StoreResult<Option<T>> {
    let Some(mut state) = read_json_opt::<T>(path).await? else { return Ok(None) };
    let journal = journal_path(path);
    let text = match fs::read_to_string(&journal).await {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Some(state)),
        Err(err) => return Err(to_backend_error(err)),
    };
    let torn = !text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let changes: Vec<T::Change> = match serde_json::from_str(line) {
            Ok(changes) => changes,
            Err(err) if torn && i + 1 == lines.len() => {
                warn!(
                    "The journal {} ends in a torn line, left by a crash; it is dropped: {}",
                    journal.display(),
                    err
                );
                break;
            }
            Err(err) => {
                return Err(StoreError::Backend(format!(
                    "line {} of the journal of a stored state is corrupt: {err}",
                    i + 1
                )))
            }
        };
        for change in changes {
            state.apply(change).map_err(|e| {
                StoreError::Backend(format!(
                    "line {} of the journal does not fit the stored state: {e}",
                    i + 1
                ))
            })?;
        }
    }
    Ok(Some(state))
}

/// Appends `changes` as one line to the journal at `journal`, which is
/// `len_before` bytes long. Should the write fail, the journal is cut back
/// to what it was, so that a torn line is never followed by a good one.
async fn append_changes<C: Serialize>(
    journal: &Path,
    len_before: u64,
    changes: &[C],
) -> StoreResult<()> {
    let mut line = serde_json::to_vec(changes).map_err(to_backend_error)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(journal)
        .await
        .map_err(to_backend_error)?;
    // A tokio file hands the write to a blocking task: flushed, it is done
    // before the put returns, and the next put sees the journal's size.
    let written = match file.write_all(&line).await {
        Ok(()) => file.flush().await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = file.set_len(len_before).await;
        return Err(to_backend_error(err));
    }
    Ok(())
}

/// The state operations: a snapshot and a journal per state (issue 193).
impl JsonStates {
    /// Takes out of the index of the work under way every entry whose
    /// channel and agent, as written in its name, `matches`.
    async fn leave_under_way(&self, matches: impl Fn(&str, &str) -> bool) -> StoreResult<()> {
        let mut entries = match fs::read_dir(self.layout.under_way()).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(to_backend_error(err)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|name| name.split_once('.'))
                .is_some_and(|(channel, agent)| matches(channel, agent))
            {
                delete_if_exists(&entry.path()).await?;
            }
        }
        Ok(())
    }

    /// Stores `state` at `path`, and marks it stored: as a line of its
    /// journal, or - if there is no snapshot yet, or the journal is as large
    /// as the snapshot and `journal_floor` - as a new snapshot, after which
    /// the journal is emptied. The sizes are asked of the file system every
    /// time; nothing is kept between puts.
    async fn store_state<T: Journaled>(&self, path: &Path, state: &mut T) -> StoreResult<()> {
        make_parent(path).await?;
        let journal = journal_path(path);
        let journal_len = size_of(&journal).await?.unwrap_or(0);
        match size_of(path).await? {
            Some(snapshot_len) if journal_len < snapshot_len.max(self.journal_floor) => {
                debug!("Appending to the journal {}", journal.display());
                append_changes(&journal, journal_len, &state.changes()).await?;
            }
            _ => {
                debug!("Writing the snapshot {}", path.display());
                write_json_atomic(path, state).await?;
                delete_if_exists(&journal).await?;
            }
        }
        state.mark_stored();
        Ok(())
    }
}

#[async_trait]
impl StateStore for JsonStates {
    async fn get_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
    ) -> StoreResult<AgentChannelState> {
        let path = self.agent_channel_state_path(agent_id, channel_id).await?;
        debug!("Reading agent channel state from {}", path.display());
        let mut state: AgentChannelState = load_state(&path).await?.unwrap_or_default();
        state.transcript =
            self.load_transcript(agent_id, channel_id, state.transcript_from).await?;
        load_named_by_turn(self, agent_id, channel_id, &mut state).await?;
        Ok(state)
    }

    /// The prompt texts and the tool sets first, which what follows names,
    /// then the transcript, then the document, which says from where it is
    /// loaded. The index of the work under way is entered before the
    /// document and left after it: what a crash leaves between the two is a
    /// session started for nothing, never work that is not started.
    async fn put_agent_channel_state(
        &self,
        agent_id: AgentId,
        channel_id: ChannelId,
        state: &mut AgentChannelState,
    ) -> StoreResult<()> {
        self.append_given(agent_id, channel_id, state).await?;
        self.append_transcript(agent_id, channel_id, &mut state.transcript).await?;
        let under_way = self.under_way_path(agent_id, channel_id);
        let path = self.agent_channel_state_path(agent_id, channel_id).await?;
        if state.has_work_under_way() {
            create_if_missing(&under_way).await?;
            self.store_state(&path, state).await
        } else {
            self.store_state(&path, state).await?;
            delete_if_exists(&under_way).await.map(drop)
        }
    }

    async fn channels_with_work_under_way(&self) -> StoreResult<Vec<ChannelId>> {
        let mut entries = match fs::read_dir(self.layout.under_way()).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(to_backend_error(err)),
        };
        let mut channels = HashSet::new();
        while let Some(entry) = entries.next_entry().await.map_err(to_backend_error)? {
            let name = entry.file_name();
            match name
                .to_str()
                .and_then(|name| name.split_once('.'))
                .and_then(|(channel, _)| channel.parse::<ChannelId>().ok())
            {
                Some(channel_id) => {
                    channels.insert(channel_id);
                }
                None => warn!(
                    "The index of the work under way holds a file that names no channel: {}",
                    name.to_string_lossy()
                ),
            }
        }
        Ok(channels.into_iter().collect())
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let path = self.channel_state_path(channel_id).await?;
        debug!("Reading channel state from {}", path.display());
        let mut state: ChannelState = load_state(&path)
            .await?
            .ok_or(StoreError::NotFound { entity: "channel_state", id: channel_id.to_string() })?;
        state.history = self.load_history(channel_id, state.history_from).await?;
        Ok(state)
    }

    /// The history first, then the document, which says from where it is
    /// loaded.
    async fn put_channel_state(
        &self,
        channel_id: ChannelId,
        state: &mut ChannelState,
    ) -> StoreResult<()> {
        self.append_history(channel_id, &mut state.history).await?;
        self.store_state(&self.channel_state_path(channel_id).await?, state).await
    }

    async fn get_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
    ) -> StoreResult<UserChannelState> {
        let path = self.user_channel_state_path(user_id, channel_id).await?;
        debug!("Reading user channel state from {}", path.display());
        Ok(load_state(&path).await?.unwrap_or_default())
    }

    async fn put_user_channel_state(
        &self,
        user_id: UserId,
        channel_id: ChannelId,
        state: &mut UserChannelState,
    ) -> StoreResult<()> {
        self.store_state(&self.user_channel_state_path(user_id, channel_id).await?, state).await
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        debug!("Deleting the state of channel {}", channel_id);
        let path = self.channel_state_path(channel_id).await?;
        delete_if_exists(&path).await?;
        delete_if_exists(&journal_path(&path)).await?;
        delete_dir_if_exists(&self.history_dir(channel_id).await?).await?;
        delete_dir_if_exists(&self.layout.agents_in(channel_id).await?).await?;
        delete_dir_if_exists(&self.layout.users_in(channel_id).await?).await?;
        let channel_id = channel_id.to_string();
        self.leave_under_way(|channel, _| channel == channel_id).await
    }

    async fn delete_agent_channel_states(&self, agent_id: AgentId) -> StoreResult<()> {
        for channel in self.layout.agents_of_every_channel().await? {
            let path = channel.join(format!("{agent_id}.json"));
            delete_if_exists(&path).await?;
            delete_if_exists(&journal_path(&path)).await?;
            delete_dir_if_exists(&path.with_extension("history")).await?;
            delete_dir_if_exists(&path.with_extension("transcript")).await?;
            delete_dir_if_exists(&path.with_extension("given")).await?;
        }
        let agent_id = agent_id.to_string();
        self.leave_under_way(|_, agent| agent == agent_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::RuntimeLayout;
    use super::*;
    use caelumora_domain::actor::ActorState;
    use caelumora_domain::chain::Position;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::history::Reader;
    use caelumora_domain::message::testing::{id, make_message};
    use caelumora_domain::note::{NoteChange, NoteId};
    use caelumora_domain::turn::TurnState;
    use caelumora_store::history::HistoryStore;
    use caelumora_store::suite;
    use std::fs as std_fs;
    use tempfile::tempdir;

    /// The states under `dir`, laid out as the JSON store lays them out.
    fn states(dir: &Path) -> JsonStates {
        floored(dir, DEFAULT_JOURNAL_FLOOR)
    }

    /// `states`, a journal folded into its snapshot once it is `journal_floor` large.
    fn floored(dir: &Path, journal_floor: u64) -> JsonStates {
        JsonStates::new(Arc::new(RuntimeLayout::new(dir).unwrap()), journal_floor)
    }

    /// A layout that puts everything of a channel into a directory of its own, three levels down,
    /// and makes no directory itself, unlike that of the JSON store.
    #[derive(Debug)]
    struct Nested(PathBuf);

    impl Nested {
        fn channel(&self, channel: ChannelId) -> PathBuf {
            self.0.join("one").join("two").join(channel.to_string())
        }
    }

    #[async_trait]
    impl StateLayout for Nested {
        async fn channel_state(&self, channel: ChannelId) -> StoreResult<PathBuf> {
            Ok(self.channel(channel).join("state.json"))
        }

        async fn agents_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
            Ok(self.channel(channel).join("agents"))
        }

        async fn users_in(&self, channel: ChannelId) -> StoreResult<PathBuf> {
            Ok(self.channel(channel).join("users"))
        }

        async fn agents_of_every_channel(&self) -> StoreResult<Vec<PathBuf>> {
            let Ok(mut channels) = fs::read_dir(self.0.join("one").join("two")).await else {
                return Ok(Vec::new());
            };
            let mut found = Vec::new();
            while let Some(channel) = channels.next_entry().await.map_err(to_backend_error)? {
                found.push(channel.path().join("agents"));
            }
            Ok(found)
        }

        fn under_way(&self) -> PathBuf {
            self.0.join("elsewhere").join("under_way")
        }
    }

    /// The states are kept wherever a layout puts them: nothing of the store assumes the places
    /// of the JSON store's own.
    #[tokio::test]
    async fn a_layout_of_its_own_keeps_the_states_as_well() {
        let dir = tempdir().unwrap();
        let states =
            JsonStates::new(Arc::new(Nested(dir.path().to_path_buf())), DEFAULT_JOURNAL_FLOOR);
        suite::state_store_behaves(&states).await;
        suite::work_under_way_is_indexed(&states).await;
        suite::history_store_behaves(&states, &states).await;
        suite::transcript_store_behaves(&states, &states).await;
        suite::given_store_behaves(&states, &states).await;
        assert!(!dir.path().join("runtime").exists(), "nothing of the store's own places");
    }

    #[tokio::test]
    async fn keeps_the_index_of_the_work_under_way() {
        let dir = tempdir().unwrap();

        // Read from the files: a store opened anew over them, as after a
        // restart, finds what was under way.
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut planning = AgentChannelState {
            processing_state: ActorState::Planning,
            turn: Some(TurnState::default()),
            ..AgentChannelState::default()
        };
        states(dir.path()).put_agent_channel_state(agent, channel, &mut planning).await.unwrap();
        assert_eq!(states(dir.path()).channels_with_work_under_way().await.unwrap(), [channel]);
    }

    /// The lines of the journal of the agent channel state of `agent` in
    /// `channel`, `None` if there is no journal.
    async fn journal_lines(
        states: &JsonStates,
        agent: AgentId,
        channel: ChannelId,
    ) -> Option<usize> {
        std_fs::read_to_string(journal_file(states, agent, channel).await)
            .ok()
            .map(|text| text.lines().count())
    }

    async fn journal_file(states: &JsonStates, agent: AgentId, channel: ChannelId) -> PathBuf {
        journal_path(&states.agent_channel_state_path(agent, channel).await.unwrap())
    }

    /// A state with messages `from..to` in its transcript, taken in as an
    /// agent takes them in from its channel's history - moving where it
    /// stands in it (issue 440) -, and a note published for each in its
    /// document, which grows with them.
    fn said(state: &mut AgentChannelState, from: u128, to: u128) {
        for n in from..to {
            state.transcript.push(make_message(id(n)));
            state.last_message = Some(id(n));
            state.heard.took(Position::new(n as u64));
            let note = NoteChange::Add {
                id: NoteId(n as u32),
                text: format!("note {n}"),
                classification: Classification::CHANNEL_DEFAULT,
            };
            state.notes.push(note, Default::default(), id(n));
        }
    }

    fn same(a: &AgentChannelState, b: &AgentChannelState) -> bool {
        serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
    }

    #[tokio::test]
    async fn a_put_appends_what_changed_to_the_journal_and_a_get_replays_it() {
        let dir = tempdir().unwrap();
        let opened = states(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 3);
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(
            journal_lines(&opened, agent, channel).await,
            None,
            "the first put is the snapshot"
        );
        let snapshot =
            std_fs::metadata(opened.agent_channel_state_path(agent, channel).await.unwrap())
                .unwrap()
                .len();

        said(&mut state, 3, 5);
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        state.processing_state = ActorState::WaitingForUser;
        opened.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(journal_lines(&opened, agent, channel).await, Some(2), "one line per put");
        assert_eq!(
            std_fs::metadata(opened.agent_channel_state_path(agent, channel).await.unwrap())
                .unwrap()
                .len(),
            snapshot,
            "the snapshot is untouched"
        );

        // Read from the files, by a store opened anew, as after a restart.
        let loaded = states(dir.path()).get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        assert_eq!(loaded.transcript.messages().len(), 4);
        assert_eq!(
            (loaded.last_message, loaded.heard.next()),
            (Some(id(4)), Position::new(5)),
            "where it stands in its channel's history (issue 440)"
        );
    }

    #[tokio::test]
    async fn the_journal_is_folded_into_the_snapshot_once_it_is_as_large() {
        let dir = tempdir().unwrap();
        let states = floored(dir.path(), 0);
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let path = states.agent_channel_state_path(agent, channel).await.unwrap();
        let snapshot = std_fs::metadata(&path).unwrap().len();

        // Small puts go to the journal until it is as large as the snapshot.
        let mut puts = 0;
        while journal_lines(&states, agent, channel).await.is_some() || puts == 0 {
            state.processing_state = ActorState::Idle;
            states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
            puts += 1;
            assert!(puts < 1000, "the journal is never folded");
        }
        assert!(puts > 1, "the journal was used first");
        assert_eq!(
            std_fs::metadata(&path).unwrap().len(),
            snapshot,
            "the snapshot is the same state again"
        );

        said(&mut state, 2, 40);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(
            journal_lines(&states, agent, channel).await.is_some(),
            "a large put still goes to the journal"
        );
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert_eq!(
            journal_lines(&states, agent, channel).await,
            None,
            "and the next one folds it, the journal being larger than the snapshot"
        );
        assert!(std_fs::metadata(&path).unwrap().len() > snapshot);
        assert!(same(&states.get_agent_channel_state(agent, channel).await.unwrap(), &state));
    }

    #[tokio::test]
    async fn a_torn_last_line_is_dropped_and_a_corrupt_line_before_is_not() {
        let dir = tempdir().unwrap();
        let states = states(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        said(&mut state, 2, 3);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let before = states.get_agent_channel_state(agent, channel).await.unwrap();

        let journal = journal_file(&states, agent, channel).await;
        let good = std_fs::read_to_string(&journal).unwrap();
        std_fs::write(&journal, format!("{good}[{{\"transcript\": {{\"entr")).unwrap();
        let loaded = states.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &before), "as of the last put that completed");

        std_fs::write(&journal, format!("not json\n{good}")).unwrap();
        let corrupt = states.get_agent_channel_state(agent, channel).await.unwrap_err();
        assert!(
            matches!(corrupt, StoreError::Backend(ref m) if m.contains("line 1") && !m.contains("not json")),
            "{corrupt}"
        );
    }

    #[tokio::test]
    async fn a_journal_that_survived_its_snapshot_replays_without_effect() {
        let dir = tempdir().unwrap();
        let states = states(dir.path());
        let (agent, channel) = (AgentId::new(), ChannelId::new());
        let mut state = AgentChannelState::default();
        said(&mut state, 1, 2);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        said(&mut state, 2, 4);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        let journal = journal_file(&states, agent, channel).await;
        let lines = std_fs::read_to_string(&journal).unwrap();

        // As if the crash came after the snapshot and before the journal was emptied.
        let folding = floored(dir.path(), 0);
        folding.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(!journal.exists());
        std_fs::write(&journal, &lines).unwrap();

        let loaded = states.get_agent_channel_state(agent, channel).await.unwrap();
        assert!(same(&loaded, &state), "{loaded:?}");
        said(&mut state, 4, 5);
        states.put_agent_channel_state(agent, channel, &mut state).await.unwrap();
        assert!(
            same(&states.get_agent_channel_state(agent, channel).await.unwrap(), &state),
            "and what comes after it is applied"
        );
    }

    #[tokio::test]
    async fn items_behind_their_chain_and_entries_past_its_head_are_what_a_crash_leaves() {
        let dir = tempdir().unwrap();
        let states = states(dir.path());
        let channel = ChannelId::new();
        let key = channel;
        let mut state = ChannelState::new(channel);
        state.history.extend(vec![make_message(id(1)), make_message(id(2)), make_message(id(3))]);
        states.put_channel_state(channel, &mut state).await.unwrap();
        let history = states.history_dir(key).await.unwrap();
        let read = |file: &str| {
            serde_json::from_str::<serde_json::Value>(
                &std_fs::read_to_string(history.join(file)).unwrap(),
            )
            .unwrap()
        };
        let write = |file: &str, value: &serde_json::Value| {
            std_fs::write(history.join(file), value.to_string()).unwrap()
        };

        // As if the crash came after the chain and its head, before the items.
        let mut items = read("items-0.json");
        items.as_array_mut().unwrap().truncate(1);
        write("items-0.json", &items);
        write("items-head.json", &serde_json::json!({ "end": 1 }));
        let loaded = states.get_channel_state(channel).await.unwrap();
        assert_eq!(loaded.history.end().get(), 3);
        assert_eq!(
            states
                .page(key, &Reader::Channel, Position::START, None, 10, None)
                .await
                .unwrap()
                .items
                .len(),
            3,
            "the items are derived anew from the chain"
        );
        assert_eq!(read("items-head.json")["end"], serde_json::json!(3));

        // As if the crash came after the chain, before its head.
        let mut chain = read("chain-0.json");
        let mut past = chain[2].clone();
        past["seq"] = serde_json::json!(3);
        chain.as_array_mut().unwrap().push(past);
        write("chain-0.json", &chain);
        let mut loaded = states.get_channel_state(channel).await.unwrap();
        assert_eq!(loaded.history.end().get(), 3, "what lies past the head is not there");
        loaded.history.push(make_message(id(4)));
        states.put_channel_state(channel, &mut loaded).await.unwrap();
        assert_eq!(
            states.said(key, &Reader::Channel, id(4)).await.unwrap().map(|(at, _)| at.get()),
            Some(3),
            "and the next entry takes its place"
        );
        assert_eq!(
            states.get_channel_state(channel).await.unwrap().history.digest(),
            loaded.history.digest()
        );
    }

    #[tokio::test]
    async fn deleting_a_channels_state_removes_its_journal_too() {
        let dir = tempdir().unwrap();
        let states = states(dir.path());
        let channel = ChannelId::new();
        let mut state = ChannelState::new(channel);
        states.put_channel_state(channel, &mut state).await.unwrap();
        state.history.extend(vec![make_message(id(1))]);
        states.put_channel_state(channel, &mut state).await.unwrap();
        let journal = journal_path(&states.channel_state_path(channel).await.unwrap());
        assert!(journal.exists());
        assert!(states.history_dir(channel).await.unwrap().exists());
        states.delete_channel_state(channel).await.unwrap();
        assert!(!journal.exists() && !states.channel_state_path(channel).await.unwrap().exists());
        assert!(!states.history_dir(channel).await.unwrap().exists(), "and its history");
    }
}
