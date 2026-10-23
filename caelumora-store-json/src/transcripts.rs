//! The transcripts of the agents, as the `JsonFileStore` keeps them (issue
//! 380): per agent and channel a directory beside the state's snapshot,
//! `<agent>.transcript`, with the chain as every chain is kept (`chains`)
//! and nothing laid over it. Slow where it has to be - a path is followed
//! back segment by segment, a message is looked for in every segment -,
//! which a database's indexes are for.
//!
//! Beside it the prompt texts and the tool sets the agent's model was given
//! (issue 383), and the frames of the contexts its messages were produced
//! from (issue 426), in `<agent>.given`: a file per digest,
//! `prompt-<digest>.json`, `tools-<digest>.json` and `frame-<digest>.json`,
//! read by its digest and written once - one there already is not written
//! again.

use super::chains::{self, read_chain, read_head, segment_before, segment_of, SEGMENT};
use super::{read_json_opt, size_of, to_backend_error, write_json_atomic, JsonFileStore};
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::chain::{Node, Position};
use caelumora_model::channel::ChannelId;
use caelumora_model::context::FrameRef;
use caelumora_model::digest::Digest;
use caelumora_model::kept::Kept;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::store::transcript::{transcript_of, TranscriptStore};
use caelumora_model::store::StoreResult;
use caelumora_model::tool::ToolSet;
use caelumora_model::transcript::{known, Transcript, TranscriptEntry};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::debug;
use tokio::fs;

/// The file of the value of kind `kind` with the digest `digest` in `dir`.
fn given_path(dir: &Path, kind: &str, digest: Digest) -> PathBuf {
    dir.join(format!("{kind}-{digest}.json"))
}

/// Writes what `kept` added since it was last stored into `dir`, as values
/// of kind `kind`, each digest once.
async fn write_kept<V: Serialize + Clone>(dir: &Path, kind: &str, kept: &Kept<V>) -> StoreResult<()> {
    let changes = kept.changes();
    if changes.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).await.map_err(to_backend_error)?;
    for (digest, value) in changes {
        let path = given_path(dir, kind, digest);
        if size_of(&path).await?.is_none() {
            debug!("Writing the {} {}", kind, digest);
            write_json_atomic(&path, &value).await?;
        }
    }
    Ok(())
}

impl JsonFileStore {
    /// The directory of the transcript of `agent` in `channel`, beside the
    /// snapshot of its state.
    pub(super) fn transcript_dir(&self, agent: AgentId, channel: ChannelId) -> PathBuf {
        self.agent_channel_state_path(agent, channel).with_extension("transcript")
    }

    /// Appends what `transcript`, that of `agent` in `channel` as its state
    /// holds it, added since it was last stored, and marks it stored.
    pub(super) async fn append_transcript(&self, agent: AgentId, channel: ChannelId, transcript: &mut Transcript) -> StoreResult<()> {
        let Some(changes) = transcript.changes() else { return Ok(()) };
        chains::append(&self.transcript_dir(agent, channel), transcript_of(agent, channel), changes).await?;
        transcript.mark_stored();
        Ok(())
    }

    /// The directory of the prompt texts, tool sets and frames of `agent` in
    /// `channel`, beside the snapshot of its state.
    pub(super) fn given_dir(&self, agent: AgentId, channel: ChannelId) -> PathBuf {
        self.agent_channel_state_path(agent, channel).with_extension("given")
    }

    /// Writes the prompt texts, the tool sets and the frames `state`, that
    /// of `agent` in `channel`, kept since they were last stored, and marks
    /// them stored.
    pub(super) async fn append_given(&self, agent: AgentId, channel: ChannelId, state: &mut AgentChannelState) -> StoreResult<()> {
        let dir = self.given_dir(agent, channel);
        write_kept(&dir, "prompt", &state.prompts).await?;
        write_kept(&dir, "tools", &state.offered_tools).await?;
        write_kept(&dir, "frame", &state.frames).await?;
        state.prompts.mark_stored();
        state.offered_tools.mark_stored();
        state.frames.mark_stored();
        Ok(())
    }

    /// The transcript of `agent` in `channel` from the position `from` on,
    /// as its state holds it; empty if none is kept.
    pub(super) async fn load_transcript(&self, agent: AgentId, channel: ChannelId, from: Position) -> StoreResult<Transcript> {
        chains::load(&self.transcript_dir(agent, channel), transcript_of(agent, channel), from).await
    }
}

#[async_trait]
impl TranscriptStore for JsonFileStore {
    async fn path(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<Vec<Node<TranscriptEntry>>>> {
        chains::path(&self.transcript_dir(agent, channel), transcript_of(agent, channel), digest).await
    }

    async fn message(&self, agent: AgentId, channel: ChannelId, id: MessageId) -> StoreResult<Option<Message>> {
        let dir = self.transcript_dir(agent, channel);
        let end = read_head(&dir).await?.end;
        let nodes: Vec<Node<TranscriptEntry>> = read_chain(&dir, Position::START, end).await?;
        Ok(known(nodes.iter(), id))
    }

    async fn in_transcript(&self, agent: AgentId, channel: ChannelId, ids: &[MessageId]) -> StoreResult<HashSet<MessageId>> {
        let dir = self.transcript_dir(agent, channel);
        let end = read_head(&dir).await?.end;
        let mut wanted: HashSet<MessageId> = ids.iter().copied().collect();
        let mut found = HashSet::new();
        let mut segment = end.previous().map(segment_of);
        while let Some(from) = segment.filter(|_| !wanted.is_empty()) {
            let nodes: Vec<Node<TranscriptEntry>> = read_chain(&dir, from, end.min(from.after(SEGMENT))).await?;
            for id in nodes.iter().rev().filter_map(|node| node.entry.message().map(|m| m.id)) {
                if wanted.remove(&id) {
                    found.insert(id);
                }
            }
            segment = segment_before(from);
        }
        Ok(found)
    }
    async fn prompts(&self, agent: AgentId, channel: ChannelId, digests: &[Digest]) -> StoreResult<BTreeMap<Digest, String>> {
        let dir = self.given_dir(agent, channel);
        let mut prompts = BTreeMap::new();
        for &digest in digests {
            if let Some(text) = read_json_opt(&given_path(&dir, "prompt", digest)).await? {
                prompts.insert(digest, text);
            }
        }
        Ok(prompts)
    }

    async fn offered_tools(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<Vec<ToolSet>>> {
        read_json_opt(&given_path(&self.given_dir(agent, channel), "tools", digest)).await
    }

    async fn frame(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<FrameRef>> {
        read_json_opt(&given_path(&self.given_dir(agent, channel), "frame", digest)).await
    }
}
