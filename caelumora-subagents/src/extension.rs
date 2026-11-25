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
    /// `UnknownPersonality`, `External`; `ClearedWider` if the personality's agents would be
    /// cleared wider than the work's agent, `TooDeep` if the work is a sub-agent as deep as
    /// allowed, `TooMany` if as many sub-agents are open in its tree as allowed (issue 597); or
    /// `Forbidden` or `Failed` with what the kernel refused or failed with.
    async fn start(
        &self,
        personality: &str,
        origin: ThreadOrigin,
        brief: String,
    ) -> SubAgentResult<ThreadId>;

    /// Gives the sub-agent of `thread` `text` to read and asks it to respond, once it answered
    /// what it was given before - asked in the turn that read that answer, or it is let go then
    /// (issue 598).
    ///
    /// # Errors
    /// `NoSubAgent` if `thread` holds none the work started, or one let go; `Forbidden` or
    /// `Failed` as for `start`.
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
    /// A personality whose agents would be cleared wider than the work's agent (issue 597).
    #[error(
        "'{0}' is cleared wider than the agent that would start it, so it cannot be its sub-agent"
    )]
    ClearedWider(String),
    /// A thread that holds no sub-agent the work started.
    #[error("thread {0} holds no sub-agent of yours")]
    NoSubAgent(ThreadId),
    /// As many sub-agents open in the work's tree as allowed, the number allowed (issue 597).
    #[error("as many sub-agents are open as allowed, {0}: end one, or wait for one to end")]
    TooMany(usize),
    /// The work is a sub-agent as deep as allowed, the depth allowed (issue 597).
    #[error("sub-agents are started no deeper than {0}")]
    TooDeep(usize),
    /// What the kernel refused the work, worded as users are told it - not by which rule.
    #[error("{0}")]
    Forbidden(String),
    /// What the kernel failed with, worded as users are told it; what may not be shown is logged.
    #[error("{0}")]
    Failed(String),
}

#[cfg(feature = "library")]
impl SubAgentError {
    /// Whether whoever asked can correct it (issue 596): a personality, one cleared no wider, a
    /// thread, a sub-agent to end first - not what the kernel refused, a depth nothing they do
    /// changes, or what failed. What a tool's model and a command's issuer are told apart by.
    pub(crate) fn is_correctable(&self) -> bool {
        match self {
            SubAgentError::UnknownPersonality(_)
            | SubAgentError::External(_)
            | SubAgentError::ClearedWider(_)
            | SubAgentError::NoSubAgent(_)
            | SubAgentError::TooMany(_) => true,
            SubAgentError::TooDeep(_) | SubAgentError::Forbidden(_) | SubAgentError::Failed(_) => {
                false
            }
        }
    }
}

pub type SubAgentResult<T> = Result<T, SubAgentError>;
