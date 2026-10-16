//! An agent's transcript as a store keeps it (issue 380): every branch it
//! ever had, written by the put of the agent's state in the channel
//! (`StateStore::put_agent_channel_state`), while the agent's actor holds
//! only its active branch. The `TranscriptStore` is the read side, for
//! whoever reads what lies on an older branch.

use crate::agent::AgentId;
use crate::chain::Node;
use crate::channel::ChannelId;
use crate::digest::Digest;
use crate::message::{Message, MessageId};
use crate::store::StoreResult;
use crate::transcript::TranscriptEntry;
use async_trait::async_trait;
use std::collections::HashSet;
use std::fmt::Debug;

/// The read side of the transcripts a store keeps (issue 380), each of an
/// agent in a channel. A transcript is written with the agent's state in
/// the channel; what is read here is as fresh as what was put. A transcript
/// no store holds reads as empty. A digest may be there twice - a holder
/// dedupes only within its window -, and names the same content either
/// time.
#[async_trait]
pub trait TranscriptStore: Debug + Send + Sync {
    /// The path from the root up to the entry with the digest `digest`, on
    /// whichever branch: what the context of a message names (`ContextRef`).
    /// `None` if the transcript holds no such entry.
    async fn path(&self, agent: AgentId, channel: ChannelId, digest: Digest) -> StoreResult<Option<Vec<Node<TranscriptEntry>>>>;

    /// The message `id` as the transcript holds it, on whichever branch: as
    /// it was first recorded, with every change of it recorded since. What
    /// the remote side of an external agent was given of it (issue 236).
    /// `None` if the transcript holds it nowhere.
    async fn message(&self, agent: AgentId, channel: ChannelId, id: MessageId) -> StoreResult<Option<Message>>;

    /// Those of the messages `ids` the transcript holds, on whichever
    /// branch: which of what the agent heard was passed on to it (issue
    /// 196).
    async fn given(&self, agent: AgentId, channel: ChannelId, ids: &[MessageId]) -> StoreResult<HashSet<MessageId>>;
}

/// How a transcript is named where something about it fails.
pub(crate) fn transcript_of(agent: AgentId, channel: ChannelId) -> String {
    format!("the transcript of agent {agent} in channel {channel}")
}
