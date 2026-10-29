//! An agent's transcript as a store keeps it (issue 380): every branch it
//! ever had, written by the put of the agent's state in the channel
//! (`StateStore::put_agent_channel_state`), while the agent's actor holds
//! only its active branch. The `TranscriptStore` is the read side, for
//! whoever reads what lies on an older branch - and of the prompt texts and
//! tool sets placed beside it (issue 383), which are written by the same
//! put, and held by the actor only while its turn under way names them, and
//! of the frames the contexts of its messages had (issue 426), held only
//! until they are stored.

use crate::agent::{AgentChannelState, AgentId};
use crate::chain::Node;
use crate::channel::ChannelId;
use crate::context::{ContextRef, FrameRef};
use crate::digest::Digest;
use crate::kept::Kept;
use crate::message::{Message, MessageId};
use crate::store::StoreResult;
use crate::tool::ToolSet;
use crate::transcript::TranscriptEntry;
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;

/// The read side of the transcripts a store keeps (issue 380), each of an
/// agent in a channel, with the prompt texts, the tool sets and the frames
/// its model was given beside it (issues 383, 426). All are written with the
/// agent's state in the channel; what is read here is as fresh as what was
/// put. A transcript no store holds reads as empty. A digest may be there
/// twice in a transcript - a holder dedupes only within its window -, and
/// names the same content either time; a text, a set of tools or a frame is
/// kept once by its digest, written again as the same.
#[async_trait]
pub trait TranscriptStore: Debug + Send + Sync {
    /// The path from the root up to the entry with the digest `digest`, on
    /// whichever branch: what the context of a message names (`ContextRef`).
    /// `None` if the transcript holds no such entry.
    async fn path(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<Vec<Node<TranscriptEntry>>>>;

    /// The message `id` as the transcript holds it, on whichever branch: as
    /// it was first recorded, with every change of it recorded since. What
    /// the remote side of an external agent was given of it (issue 236).
    /// `None` if the transcript holds it nowhere.
    async fn message(
        &self,
        agent: AgentId,
        channel: ChannelId,
        id: MessageId,
    ) -> StoreResult<Option<Message>>;

    /// Those of the messages `ids` the transcript holds, on whichever
    /// branch: which of what the agent heard was passed on to it (issue
    /// 196).
    async fn in_transcript(
        &self,
        agent: AgentId,
        channel: ChannelId,
        ids: &[MessageId],
    ) -> StoreResult<HashSet<MessageId>>;

    /// The prompt texts of those of `digests` that are kept, each by its
    /// digest (issue 383).
    async fn prompts(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digests: &[Digest],
    ) -> StoreResult<BTreeMap<Digest, String>>;

    /// The tool sets of the offer `digest` names (issue 383); `None` if it
    /// is not kept.
    async fn offered_tools(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<Vec<ToolSet>>>;

    /// The frame `digest` names (issue 426); `None` if it is not kept.
    async fn frame(
        &self,
        agent: AgentId,
        channel: ChannelId,
        digest: Digest,
    ) -> StoreResult<Option<FrameRef>>;

    /// The context a message of `agent` in `channel` was produced from, as
    /// it names it (`Message::produced_from`, issue 426): the digest
    /// `transcript` of the transcript before it, and the frame `frame`
    /// names, read. `None` if the frame is not kept.
    async fn context(
        &self,
        agent: AgentId,
        channel: ChannelId,
        transcript: Option<Digest>,
        frame: Digest,
    ) -> StoreResult<Option<ContextRef>> {
        Ok(self.frame(agent, channel, frame).await?.map(|frame| ContextRef { transcript, frame }))
    }

    /// The prompt texts and the tool sets `reference` names, of `agent` in
    /// `channel`, with nothing pending (issue 383): what a context is
    /// resolved with besides its transcript (`ContextResolver`). What it
    /// names and is not kept is not there.
    async fn named_by(
        &self,
        agent: AgentId,
        channel: ChannelId,
        reference: &ContextRef,
    ) -> StoreResult<(Kept<String>, Kept<Vec<ToolSet>>)> {
        let digests: Vec<Digest> =
            reference.frame.prompts.iter().map(|prompt| prompt.text).collect();
        let prompts = self.prompts(agent, channel, &digests).await?;
        let offered = match reference.frame.tools {
            Some(digest) => {
                self.offered_tools(agent, channel, digest).await?.map(|sets| (digest, sets))
            }
            None => None,
        };
        Ok((Kept::stored(prompts), Kept::stored(offered)))
    }
}

/// Reads into `state`, that of `agent` in `channel` as a store loaded it,
/// the prompt texts and the tool sets its turn under way names (issue
/// 383): what a turn cut off by a crash is resumed with. A state between
/// turns holds none.
pub async fn load_named_by_turn(
    transcripts: &dyn TranscriptStore,
    agent: AgentId,
    channel: ChannelId,
    state: &mut AgentChannelState,
) -> StoreResult<()> {
    let Some(turn) = &state.turn else { return Ok(()) };
    let (prompts, offered_tools) = transcripts.named_by(agent, channel, &turn.work.context).await?;
    state.prompts = prompts;
    state.offered_tools = offered_tools;
    Ok(())
}

/// How a transcript is named where something about it fails.
pub fn transcript_of(agent: AgentId, channel: ChannelId) -> String {
    format!("the transcript of agent {agent} in channel {channel}")
}
