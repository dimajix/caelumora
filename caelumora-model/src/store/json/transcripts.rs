//! The transcripts of the agents, as the `JsonFileStore` keeps them (issue
//! 380): per agent and channel a directory beside the state's snapshot,
//! `<agent>.transcript`, with the chain as every chain is kept (`chains`)
//! and nothing laid over it. Slow where it has to be - a path is followed
//! back segment by segment, a message is looked for in every segment -,
//! which a database's indexes are for.

use super::chains::{self, read_chain, read_head, segment_of, SEGMENT};
use super::JsonFileStore;
use crate::agent::AgentId;
use crate::chain::Node;
use crate::channel::ChannelId;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::store::transcript::{transcript_of, TranscriptStore};
use crate::store::StoreResult;
use crate::transcript::{known, Transcript, TranscriptEntry};
use async_trait::async_trait;
use std::collections::HashSet;
use std::path::PathBuf;

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

    /// The transcript of `agent` in `channel` from the position `from` on,
    /// as its state holds it; empty if none is kept.
    pub(super) async fn load_transcript(&self, agent: AgentId, channel: ChannelId, from: u64) -> StoreResult<Transcript> {
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
        let nodes: Vec<Node<TranscriptEntry>> = read_chain(&dir, 0, end).await?;
        Ok(known(nodes.iter(), id))
    }

    async fn given(&self, agent: AgentId, channel: ChannelId, ids: &[MessageId]) -> StoreResult<HashSet<MessageId>> {
        let dir = self.transcript_dir(agent, channel);
        let end = read_head(&dir).await?.end;
        let mut wanted: HashSet<MessageId> = ids.iter().copied().collect();
        let mut found = HashSet::new();
        let mut segment = segment_of(end.saturating_sub(1));
        while end > 0 && !wanted.is_empty() {
            let nodes: Vec<Node<TranscriptEntry>> = read_chain(&dir, segment, end.min(segment + SEGMENT)).await?;
            for id in nodes.iter().rev().filter_map(|node| node.entry.message().map(|m| m.id)) {
                if wanted.remove(&id) {
                    found.insert(id);
                }
            }
            if segment == 0 {
                break;
            }
            segment -= SEGMENT;
        }
        Ok(found)
    }
}
