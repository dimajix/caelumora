//! Reading what other agents and channels of a session know (issue 17).
//!
//! Every agent writes only its own notes, into its own state
//! (`crate::agent_notes::AgentNotes`, taken over when its turn completes).
//! What others published is read from their states in the `StateStore`,
//! where an agent's state is stored before it announces the end of its turn -
//! so a reader sees notes of finished turns only, as principle 3 in issue 19
//! requires. Another channel's notes
//! and history are read only if the reading channel may read that channel
//! (`ChannelSessionReader::may_read`, `AccessQuestions::may_channel_read`),
//! asked as the agent that reads: decided by the two channels'
//! confidentiality levels, not by who takes part in them, since an agent
//! passes on what it reads to its channel (principle 11 in issue 19, as
//! changed by issue 61).

use crate::channel::ChannelLoop;
use crate::runtime::ActorRuntime;
use crate::tool::ToolError;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::ChannelId;
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::security::{AccessQuestions, SecurityContext};
use std::fmt::Debug;
use std::sync::Arc;

/// The notes one agent published in a channel.
#[derive(Debug, Clone)]
pub struct PublishedNotes {
    pub agent: ActorId,
    pub name: String,
    pub notes: Vec<Note>,
    /// The digest of the agent's note chain: the state of knowledge read.
    pub digest: Option<Digest>,
}

/// A channel of the session, as a reader names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub name: String,
}

/// The history of a channel as the reader may see it, with what to record
/// as read.
#[derive(Debug, Clone)]
pub struct ChannelHistory {
    pub channel: ChannelInfo,
    pub history: crate::channel::VisibleHistory,
}

/// Notes of one agent from one channel.
#[derive(Debug, Clone)]
pub struct ChannelNotes {
    pub channel: ChannelInfo,
    pub notes: Vec<Note>,
    pub digest: Option<Digest>,
}

/// Why a read failed - worded for the model that asked, which is how a tool
/// reports it (`From<ReadError> for ToolError`).
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// A channel that cannot be read: it does not exist, or the users of the
    /// reading channel may not read it. Deliberately one case, so that a
    /// reader cannot probe which channels exist.
    #[error("there is no channel named \"{0}\" that can be read from here")]
    NotReadable(String),
    /// Reading a channel that may be read failed.
    #[error("reading failed: {0}")]
    Failed(#[from] anyhow::Error),
}

pub type ReadResult<T> = std::result::Result<T, ReadError>;

/// A channel that cannot be read is the model's to correct; a failed read
/// is a failure of the tool.
impl From<ReadError> for ToolError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::NotReadable(_) => ToolError::InvalidParameter(error.to_string()),
            ReadError::Failed(_) => ToolError::ExecutionFailed(error.to_string()),
        }
    }
}

/// Read access to what the agents and channels of a session published,
/// given to a turn.
#[async_trait]
pub trait SessionReader: Send + Sync + Debug {
    /// The turn's own channel.
    fn own_channel(&self) -> Option<ChannelInfo>;
    /// The notes every agent of the turn's own channel published, in no
    /// particular order.
    async fn channel_notes(&self) -> ReadResult<Vec<PublishedNotes>>;
    /// The channels of the session whose notes and history may be read from
    /// the turn's channel, its own included, by name.
    async fn readable_channels(&self) -> ReadResult<Vec<ChannelInfo>>;
    /// The notes published in the channel named `name`.
    ///
    /// # Errors
    /// [`ReadError::NotReadable`] if there is no such channel or it may not be read.
    async fn notes_of(&self, name: &str) -> ReadResult<(ChannelInfo, Vec<PublishedNotes>)>;
    /// The history of the channel named `name`.
    ///
    /// # Errors
    /// [`ReadError::NotReadable`] if there is no such channel or it may not be read.
    async fn history_of(&self, name: &str) -> ReadResult<ChannelHistory>;
    /// The private notes `agent` wrote in the other channels of the session
    /// that the turn's channel may read (issue 51) - its agent session
    /// memory, bounded by confidentiality. The own channel's are not among
    /// them: the turn has them itself.
    async fn private_notes(&self, agent: AgentId) -> ReadResult<Vec<ChannelNotes>>;
}

/// A reader that reads nothing: no channel, no notes, every channel named
/// not readable - for components built outside of a channel, such as the
/// commands of a session, and for tests.
pub fn detached() -> Arc<dyn SessionReader> {
    Arc::new(DetachedSessionReader)
}

#[derive(Debug)]
struct DetachedSessionReader;

#[async_trait]
impl SessionReader for DetachedSessionReader {
    fn own_channel(&self) -> Option<ChannelInfo> {
        None
    }

    async fn channel_notes(&self) -> ReadResult<Vec<PublishedNotes>> {
        Ok(Vec::new())
    }

    async fn readable_channels(&self) -> ReadResult<Vec<ChannelInfo>> {
        Ok(Vec::new())
    }

    async fn notes_of(&self, name: &str) -> ReadResult<(ChannelInfo, Vec<PublishedNotes>)> {
        Err(ReadError::NotReadable(name.to_string()))
    }

    async fn history_of(&self, name: &str) -> ReadResult<ChannelHistory> {
        Err(ReadError::NotReadable(name.to_string()))
    }

    async fn private_notes(&self, _agent: AgentId) -> ReadResult<Vec<ChannelNotes>> {
        Ok(Vec::new())
    }
}

/// Reads the session of the channel an actor runs in, as that actor.
#[derive(Debug)]
pub struct ChannelSessionReader {
    runtime: ActorRuntime,
    /// Whose reading it is: the actor's security context, in whose name
    /// every read is asked (issue 131).
    context: SecurityContext,
}

impl ChannelSessionReader {
    pub fn new(runtime: ActorRuntime, context: SecurityContext) -> Self {
        Self { runtime, context }
    }

    /// Whether the channel `own` may read `source`, as the confidentiality
    /// rules decide (`AccessQuestions::may_channel_read`).
    async fn may_read(&self, own: &ChannelLoop, source: &ChannelLoop) -> bool {
        self.runtime.security_manager()
            .may_channel_read(&self.context, &own.classification(), &source.classification())
            .await
    }

    /// The channel named `name` of the session, if the turn's channel may
    /// read it.
    async fn readable(&self, name: &str) -> ReadResult<Arc<ChannelLoop>> {
        let not_readable = || ReadError::NotReadable(name.to_string());
        let own = self.runtime.channel().ok_or_else(not_readable)?;
        let source = if own.name() == name {
            own.clone()
        } else {
            let session = own.session().ok_or_else(not_readable)?;
            session.channel_by_name(name).ok_or_else(not_readable)?
        };
        if self.may_read(&own, &source).await { Ok(source) } else { Err(not_readable()) }
    }
}

fn info(channel: &ChannelLoop) -> ChannelInfo {
    ChannelInfo { id: channel.id(), name: channel.name() }
}

#[async_trait]
impl SessionReader for ChannelSessionReader {
    fn own_channel(&self) -> Option<ChannelInfo> {
        self.runtime.channel().map(|c| info(&c))
    }

    async fn channel_notes(&self) -> ReadResult<Vec<PublishedNotes>> {
        match self.runtime.channel() {
            Some(channel) => Ok(channel.published_notes().await?),
            // Not attached to a channel (yet): nothing is published.
            None => Ok(Vec::new()),
        }
    }

    async fn readable_channels(&self) -> ReadResult<Vec<ChannelInfo>> {
        let Some(own) = self.runtime.channel() else { return Ok(Vec::new()) };
        let channels = match own.session() {
            Some(session) => session.channels(),
            None => vec![own.clone()],
        };
        let mut readable = Vec::new();
        for channel in channels {
            if self.may_read(&own, &channel).await {
                readable.push(info(&channel));
            }
        }
        Ok(readable)
    }

    async fn notes_of(&self, name: &str) -> ReadResult<(ChannelInfo, Vec<PublishedNotes>)> {
        let channel = self.readable(name).await?;
        Ok((info(&channel), channel.published_notes().await?))
    }

    async fn history_of(&self, name: &str) -> ReadResult<ChannelHistory> {
        let channel = self.readable(name).await?;
        // A reader who joined the channel later sees no more of its history
        // than was granted - also through this tool (issue 46).
        let history = channel.visible_history(self.runtime.actor()).await;
        Ok(ChannelHistory { channel: info(&channel), history })
    }

    async fn private_notes(&self, agent: AgentId) -> ReadResult<Vec<ChannelNotes>> {
        let Some(own) = self.runtime.channel() else { return Ok(Vec::new()) };
        let Some(session) = own.session() else { return Ok(Vec::new()) };
        let mut found = Vec::new();
        for channel in session.channels() {
            if channel.id() == own.id() || !self.may_read(&own, &channel).await {
                continue;
            }
            if let Some((notes, digest)) = channel.private_notes(agent).await? {
                found.push(ChannelNotes { channel: info(&channel), notes, digest });
            }
        }
        Ok(found)
    }
}
