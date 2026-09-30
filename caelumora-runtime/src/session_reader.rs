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
//! classifications, not by who takes part in them, since an agent passes on
//! what it reads to its channel (principle 11 in issue 19, as changed by
//! issues 61 and 133).

use crate::channel::ChannelLoop;
use crate::runtime::ActorRuntime;
use crate::tool::ToolError;
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::ChannelId;
use caelumora_model::confidentiality::Classification;
use caelumora_model::digest::Digest;
use caelumora_model::note::Note;
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreError;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::security::{AccessQuestions, Place, SecurityContext};
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
    /// The session of the turn's own channel as privacy sees it (issue
    /// 134): whom it lets read what was said in it, and who takes part.
    fn own_privacy(&self) -> SessionPrivacy;
    /// The session `id` as privacy sees it now, or `None` if there is no
    /// such session any more (issue 134). Looked up in the runtime's name:
    /// it is asked to enforce what that session's people agreed to, and
    /// nothing of it reaches the turn.
    ///
    /// # Errors
    /// [`ReadError::Failed`] if it cannot be looked up.
    async fn privacy_of(&self, id: SessionId) -> ReadResult<Option<SessionPrivacy>>;
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
    /// that the turn's channel may read, note by note (issues 51, 133) -
    /// its agent session memory, bounded by confidentiality. The own
    /// channel's are not among them: the turn has them itself.
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

    fn own_privacy(&self) -> SessionPrivacy {
        SessionPrivacy::default()
    }

    async fn privacy_of(&self, _id: SessionId) -> ReadResult<Option<SessionPrivacy>> {
        Ok(None)
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

    /// Whether the channel `own` may read what `source` holds classified
    /// `classification`, as the confidentiality rules decide
    /// (`AccessQuestions::may_channel_read`, issue 133).
    async fn may_read(&self, own: &ChannelLoop, source: &ChannelLoop, classification: &Classification) -> bool {
        let (own_channel, own_classification, own_session) = (own.channel_ref(), own.classification(), privacy_of(own));
        let (source_channel, source_session) = (source.channel_ref(), privacy_of(source));
        let reader = Place { channel: &own_channel, classification: &own_classification, session: &own_session };
        let source = Place { channel: &source_channel, classification, session: &source_session };
        self.runtime.security_manager().may_channel_read(&self.context, reader, source).await
    }

    /// Whether the channel `own` may read `source` as a whole: everything
    /// in it, at the channel's classification.
    async fn may_read_all(&self, own: &ChannelLoop, source: &ChannelLoop) -> bool {
        self.may_read(own, source, &source.classification()).await
    }

    /// The channel named `name` of the session, and whether the turn's
    /// channel may read it as a whole. What it may not read as a whole is
    /// read entry by entry: only what was released to it.
    async fn find(&self, name: &str) -> ReadResult<(Arc<ChannelLoop>, Arc<ChannelLoop>, bool)> {
        let not_readable = || ReadError::NotReadable(name.to_string());
        let own = self.runtime.channel_loop().ok_or_else(not_readable)?;
        let source = if own.name() == name {
            own.clone()
        } else {
            let session = own.session_loop().ok_or_else(not_readable)?;
            session.channel_by_name(name).ok_or_else(not_readable)?
        };
        let whole = self.may_read_all(&own, &source).await;
        Ok((own, source, whole))
    }

    /// Of `notes` in `source`, those the channel `own` may read.
    async fn readable_notes(&self, own: &ChannelLoop, source: &ChannelLoop, notes: Vec<Note>) -> Vec<Note> {
        let mut readable = Vec::with_capacity(notes.len());
        for note in notes {
            if self.may_read(own, source, &note.classification).await {
                readable.push(note);
            }
        }
        readable
    }
}

/// The session of `channel` as privacy sees it (issue 134): that of a
/// channel of no session - or none at all - is private, which no other
/// session reads.
fn privacy_of(channel: &ChannelLoop) -> SessionPrivacy {
    channel.session_loop().map(|session| session.privacy()).unwrap_or_default()
}

fn info(channel: &ChannelLoop) -> ChannelInfo {
    ChannelInfo { id: channel.id(), name: channel.name() }
}

#[async_trait]
impl SessionReader for ChannelSessionReader {
    fn own_channel(&self) -> Option<ChannelInfo> {
        self.runtime.channel_loop().map(|c| info(&c))
    }

    fn own_privacy(&self) -> SessionPrivacy {
        self.runtime.channel_loop().map(|c| privacy_of(&c)).unwrap_or_default()
    }

    async fn privacy_of(&self, id: SessionId) -> ReadResult<Option<SessionPrivacy>> {
        // Changes to a session are stored before they take effect, so the
        // stored one is the current one, running or not.
        match self.runtime.load_session(&SecurityContext::system(), id).await {
            Ok(session) => Ok(Some(SessionPrivacy::of(&session))),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(ReadError::Failed(anyhow::Error::new(e).context(format!("looking up session {id}")))),
        }
    }

    async fn channel_notes(&self) -> ReadResult<Vec<PublishedNotes>> {
        match self.runtime.channel_loop() {
            Some(channel) => Ok(channel.published_notes().await?),
            // Not attached to a channel (yet): nothing is published.
            None => Ok(Vec::new()),
        }
    }

    async fn readable_channels(&self) -> ReadResult<Vec<ChannelInfo>> {
        let Some(own) = self.runtime.channel_loop() else { return Ok(Vec::new()) };
        let channels = match own.session_loop() {
            Some(session) => session.channels(),
            None => vec![own.clone()],
        };
        let mut readable = Vec::new();
        for channel in channels {
            if self.may_read_all(&own, &channel).await {
                readable.push(info(&channel));
            }
        }
        Ok(readable)
    }

    async fn notes_of(&self, name: &str) -> ReadResult<(ChannelInfo, Vec<PublishedNotes>)> {
        let (own, channel, whole) = self.find(name).await?;
        let mut published = Vec::new();
        for mut notes in channel.published_notes().await? {
            notes.notes = self.readable_notes(&own, &channel, notes.notes).await;
            published.push(notes);
        }
        // A channel that may not be read, and of which nothing was
        // released, is the same as one that does not exist: no probing.
        if !whole && published.iter().all(|p| p.notes.is_empty()) {
            return Err(ReadError::NotReadable(name.to_string()));
        }
        Ok((info(&channel), published))
    }

    async fn history_of(&self, name: &str) -> ReadResult<ChannelHistory> {
        let (own, channel, whole) = self.find(name).await?;
        // A reader who joined the channel later sees no more of its history
        // than was granted - also through this tool (issue 46).
        let mut history = channel.visible_history(self.runtime.actor()).await;
        let mut readable = Vec::with_capacity(history.messages.len());
        for message in history.messages {
            if self.may_read(&own, &channel, &message.classification).await {
                readable.push(message);
            }
        }
        if !whole && readable.is_empty() {
            return Err(ReadError::NotReadable(name.to_string()));
        }
        history.messages = readable;
        Ok(ChannelHistory { channel: info(&channel), history })
    }

    async fn private_notes(&self, agent: AgentId) -> ReadResult<Vec<ChannelNotes>> {
        let Some(own) = self.runtime.channel_loop() else { return Ok(Vec::new()) };
        let Some(session) = own.session_loop() else { return Ok(Vec::new()) };
        let mut found = Vec::new();
        for channel in session.channels() {
            if channel.id() == own.id() {
                continue;
            }
            if let Some((notes, digest)) = channel.private_notes(agent).await? {
                let notes = self.readable_notes(&own, &channel, notes).await;
                if !notes.is_empty() {
                    found.push(ChannelNotes { channel: info(&channel), notes, digest });
                }
            }
        }
        Ok(found)
    }
}
