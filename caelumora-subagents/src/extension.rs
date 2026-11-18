//! The part of the kernel's API that is sub-agents' (issue 558): what an agent's work does with
//! them - starts one on a brief, asks it again, ends it -, and what that fails with. A writer bound
//! by the plugin to every piece of work of an agent (`SubagentsPlugin`), handed out where the work
//! acts (`Turn::writer`), so that it acts in the work's context: the agent the giver, its owner
//! the owner of what it starts.

use async_trait::async_trait;
use caelumora_domain::channel::{ThreadId, ThreadOrigin};

/// What an agent's work does with sub-agents: starts one on a brief, asks it again, ends it.
#[async_trait]
pub trait SubAgentWriter: Send + Sync {
    /// Starts an agent of the personality `personality` on `brief`, in a thread of its own opened
    /// at `origin`; returns the thread at once. What comes of the brief is given back to read where
    /// the thread was opened.
    ///
    /// # Errors
    /// `UnknownPersonality`, `External`, or `Refused` with what the kernel refused or failed with.
    async fn start(
        &self,
        personality: &str,
        origin: ThreadOrigin,
        brief: String,
    ) -> SubAgentResult<ThreadId>;

    /// Gives the sub-agent of `thread` `text` to read and asks it to respond, once it answered
    /// what it was given before.
    ///
    /// # Errors
    /// `NoSubAgent` if `thread` holds none the work started, `Refused` as for `start`.
    async fn ask(&self, thread: ThreadId, text: String) -> SubAgentResult<()>;

    /// Ends the sub-agent of `thread`: let go from its channel and its session, its thread closed,
    /// the agent deleted - never purged: what it did is kept.
    ///
    /// # Errors
    /// As `ask`.
    async fn end(&self, thread: ThreadId) -> SubAgentResult<()>;
}

/// What the operations on sub-agents fail with (issue 558).
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    /// No personality of that name the work may start an agent of.
    #[error("no personality '{0}' to start a sub-agent of")]
    UnknownPersonality(String),
    /// A personality of an external agent, which cannot be a sub-agent (issue 591).
    #[error("'{0}' is the personality of an external agent, which cannot be a sub-agent")]
    External(String),
    /// A thread that holds no sub-agent the work started.
    #[error("thread {0} holds no sub-agent of yours")]
    NoSubAgent(ThreadId),
    /// What the kernel refused or failed with, in words.
    #[error("{0}")]
    Refused(String),
}

pub type SubAgentResult<T> = Result<T, SubAgentError>;
