//! The facet through which an external agent is reached (issue 179): an
//! agent that runs elsewhere and takes part in a channel through an
//! `ExternalAgentActor`. The facet says nothing of the protocol, and the
//! actor knows only the facet: a protocol is a component kind in a crate of
//! its own - A2A is `a2a_agent` in `caelumora-a2a` (issue 326).
//!
//! The remote agent is given what was said in the channel and answers as it
//! sees fit. What it is given becomes a *task* of the remote agent, within a
//! *conversation* it keeps with the channel; both are opaque handles of the
//! remote side (`ConversationId`, `TaskId`), which the actor stores.

use caelumora_runtime::component::ComponentRegistry;
use async_trait::async_trait;
use caelumora_model::confidentiality::Classification;
use caelumora_model::external_agent::{ConversationId, TaskId};
use caelumora_model::secret::SecretError;
use caelumora_model::security::SecurityContext;
use futures::stream::BoxStream;
use std::fmt::Debug;

/// What one participant said, as the remote agent is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// The name the channel knows the speaker by - for a protocol that can
    /// carry a sender itself; A2A cannot (issue 135), so `text` names them.
    pub sender: String,
    /// What they said, as the remote agent is given it: marked with the
    /// sender as the component's profile says.
    pub text: String,
}

/// One request to the remote agent: what was said since it was last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    /// The prompt parts of the agent's personality the remote agent was not
    /// given yet, such as who is in the channel (issue 204): the frame of
    /// what was said, given before it.
    pub prompts: Vec<String>,
    pub said: Vec<Said>,
    /// The task that asked for input and is answered by this message, if
    /// any; otherwise the message begins a task of its own.
    pub answers: Option<TaskId>,
}

/// What comes back from the remote agent, in the order it comes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEvent {
    /// The conversation the request belongs to - the one it was sent in, or
    /// the one the remote agent opened for it.
    Conversation(ConversationId),
    /// The remote agent took the request on as this task.
    Started(TaskId),
    /// A piece of an answer still being written, to be appended to the
    /// pieces before it.
    Partial(String),
    /// A whole message or artifact of the answer.
    Message(String),
    /// The remote agent asks something back; the task stays open until it is
    /// answered (`OutboundMessage::answers`). Ends the stream.
    InputRequired(String),
    /// The task is done. Ends the stream.
    Completed,
    /// The task was cancelled, by us or by the remote agent. Ends the stream.
    Canceled,
    /// The remote agent failed the task or refused it, for the reason given.
    /// Ends the stream.
    Failed(String),
}

impl ExternalEvent {
    /// Whether nothing comes after this event on its stream.
    pub fn ends_stream(&self) -> bool {
        matches!(self, ExternalEvent::InputRequired(_) | ExternalEvent::Completed | ExternalEvent::Canceled | ExternalEvent::Failed(_))
    }
}

/// A failure talking to the remote agent - not the remote agent failing a
/// task, which is `ExternalEvent::Failed`. Every variant names the endpoint,
/// with any credentials stripped from it (`caelumora_model::remote::redact_endpoint`), so
/// that the message is actionable on its own; it is shown to users when a
/// response fails (issue 02).
#[derive(Debug, thiserror::Error)]
pub enum ExternalAgentError {
    /// The remote agent could not be reached or the connection broke.
    #[error("cannot reach the external agent at {endpoint}: {reason}")]
    Connection { endpoint: String, reason: String },
    /// The remote agent answered the request with an error of its protocol.
    #[error("the external agent at {endpoint} rejected the request: {message}")]
    Rejected { endpoint: String, message: String },
    /// The response could not be understood.
    #[error("invalid response from the external agent at {endpoint}: {reason}")]
    InvalidResponse { endpoint: String, reason: String },
    /// The credential for the remote agent could not be had. Why is for the
    /// log: it names the secret's key.
    #[error("no usable credential for the external agent at {endpoint}")]
    Credential { endpoint: String, #[source] source: SecretError },
}

pub type ExternalAgentResult<T> = Result<T, ExternalAgentError>;

/// What comes back from the remote agent for one request, until an event
/// that ends it (`ExternalEvent::ends_stream`). Dropping it abandons the
/// request, not the task: a task that goes on is cancelled with
/// `ExternalAgent::cancel`.
pub type ExternalStream = BoxStream<'static, ExternalAgentResult<ExternalEvent>>;

/// An agent that runs elsewhere, as an external personality reaches it
/// (issue 179). Every call carries the security context it is made in, so
/// that credentials are resolved for whoever acts (`SecretManager::resolve`).
#[async_trait]
pub trait ExternalAgent: Send + Sync + Debug {
    /// Hands `message` to the remote agent within `conversation` - `None`
    /// lets it open one, which the stream then names
    /// (`ExternalEvent::Conversation`) - and streams what comes back.
    async fn send(&self, security: &SecurityContext, conversation: Option<&ConversationId>, message: OutboundMessage) -> ExternalAgentResult<ExternalStream>;

    /// Re-attaches to `task`, left open by an earlier request - after a
    /// restart - and streams what comes back from where it stands now. A
    /// task that ended meanwhile yields its end at once.
    async fn resume(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<ExternalStream>;

    /// Asks the remote agent to cancel `task`. A task that cannot be
    /// cancelled any more - it has ended - is no error.
    async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<()>;

    /// How what the remote agent brings in itself is classified, if the
    /// component is configured with a classification; otherwise as the
    /// channel. Its answers are classified as what their work has seen -
    /// this and all it was given (`Work::label`, issue 212).
    fn classification(&self) -> Option<Classification>;
}

/// Registers `ExternalAgent` with `registry`, as a facet of which an
/// external personality has exactly one. It has no default: a remote agent
/// is always named, and a personality that names none fails to build where
/// its agent asks for it. The kinds that provide it are registered by the
/// plugins of their protocols.
pub fn register_all(registry: &mut ComponentRegistry) {
    registry.single::<dyn ExternalAgent>(None);
}

/// Stand-ins for a remote agent, for the tests of what uses the facet.
#[cfg(test)]
pub(crate) mod testing {
    use crate::prompt::ModelProfileProvider;
    use caelumora_model::context::ModelProfile;
    use super::*;
    use futures::StreamExt;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;

    /// A remote agent that answers each request with the next of its
    /// scripted event lists, and records what it was given. A script that
    /// ends without an event that ends the stream leaves the task working:
    /// nothing more comes - what a cancel has to interrupt. So does a
    /// request with no script left.
    #[derive(Debug, Default)]
    pub(crate) struct ScriptedExternalAgent {
        answers: Mutex<VecDeque<Vec<ExternalAgentResult<ExternalEvent>>>>,
        resumed: Mutex<VecDeque<Vec<ExternalAgentResult<ExternalEvent>>>>,
        /// Every request, with the conversation it was sent in.
        pub(crate) sent: Mutex<Vec<(Option<ConversationId>, OutboundMessage)>>,
        /// Every task it was asked to cancel.
        pub(crate) cancelled: Mutex<Vec<TaskId>>,
        /// In whose name it was called, for every call: the call and its
        /// security context.
        pub(crate) called_as: Mutex<Vec<(&'static str, SecurityContext)>>,
        /// How many requests to come cannot reach it.
        unreachable: Mutex<usize>,
        /// How it classifies what it brings in itself.
        declared: Option<Classification>,
        /// How it is given what was said.
        profile: ModelProfile,
        /// What each answer to a request waits for, one permit each, if
        /// anything: a slow remote agent.
        gate: Option<Arc<Semaphore>>,
    }

    impl ScriptedExternalAgent {
        /// A remote agent answering its requests with `answers`, in order.
        pub(crate) fn answering(answers: Vec<Vec<ExternalAgentResult<ExternalEvent>>>) -> Self {
            Self { answers: Mutex::new(answers.into()), ..Self::default() }
        }

        /// Answers `resume` with `events`, once per call, in order.
        pub(crate) fn resuming(self, events: Vec<Vec<ExternalAgentResult<ExternalEvent>>>) -> Self {
            Self { resumed: Mutex::new(events.into()), ..self }
        }

        /// Declares what it brings in itself classified `classification`.
        pub(crate) fn declaring(self, classification: Classification) -> Self {
            Self { declared: Some(classification), ..self }
        }

        /// Is given what was said as `profile` says.
        pub(crate) fn marking(self, profile: ModelProfile) -> Self {
            Self { profile, ..self }
        }

        /// Begins each answer to a request only once `gate` gives it a
        /// permit.
        pub(crate) fn gated(self, gate: Arc<Semaphore>) -> Self {
            Self { gate: Some(gate), ..self }
        }

        /// Cannot be reached by the next `requests`: they fail before
        /// anything is sent, and are not recorded.
        pub(crate) fn unreachable_for(self, requests: usize) -> Self {
            Self { unreachable: Mutex::new(requests), ..self }
        }

        fn called(&self, call: &'static str, security: &SecurityContext) {
            self.called_as.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((call, security.clone()));
        }

        fn next(script: &Mutex<VecDeque<Vec<ExternalAgentResult<ExternalEvent>>>>) -> ExternalStream {
            let next = script.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).pop_front();
            Box::pin(futures::stream::iter(next.unwrap_or_default()).chain(futures::stream::pending()))
        }
    }

    #[async_trait]
    impl ExternalAgent for ScriptedExternalAgent {
        async fn send(&self, security: &SecurityContext, conversation: Option<&ConversationId>, message: OutboundMessage) -> ExternalAgentResult<ExternalStream> {
            self.called("send", security);
            let mut unreachable = self.unreachable.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *unreachable > 0 {
                *unreachable -= 1;
                return Err(ExternalAgentError::Connection { endpoint: "https://agent.example".to_string(), reason: "refused".to_string() });
            }
            drop(unreachable);
            self.sent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push((conversation.cloned(), message));
            let answer = Self::next(&self.answers);
            let Some(gate) = self.gate.clone() else { return Ok(answer) };
            let opened = futures::stream::once(async move {
                if let Ok(permit) = gate.acquire_owned().await {
                    permit.forget();
                }
            });
            Ok(Box::pin(opened.filter_map(|()| async { None }).chain(answer)))
        }

        async fn resume(&self, security: &SecurityContext, _: &TaskId) -> ExternalAgentResult<ExternalStream> {
            self.called("resume", security);
            Ok(Self::next(&self.resumed))
        }

        async fn cancel(&self, security: &SecurityContext, task: &TaskId) -> ExternalAgentResult<()> {
            self.called("cancel", security);
            self.cancelled.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(task.clone());
            Ok(())
        }

        fn classification(&self) -> Option<Classification> {
            self.declared.clone()
        }
    }

    impl ModelProfileProvider for ScriptedExternalAgent {
        fn profile(&self) -> ModelProfile {
            self.profile
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::ScriptedExternalAgent;
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;

    fn hello() -> OutboundMessage {
        OutboundMessage { prompts: Vec::new(), said: vec![Said { sender: "alice".to_string(), text: "hello".to_string() }], answers: None }
    }

    #[test]
    fn only_the_end_of_a_task_or_a_question_ends_a_stream() {
        assert!(!ExternalEvent::Started(TaskId("t".to_string())).ends_stream());
        assert!(!ExternalEvent::Partial("he".to_string()).ends_stream());
        assert!(!ExternalEvent::Message("hello".to_string()).ends_stream());
        assert!(ExternalEvent::InputRequired("who?".to_string()).ends_stream());
        assert!(ExternalEvent::Completed.ends_stream());
        assert!(ExternalEvent::Canceled.ends_stream());
        assert!(ExternalEvent::Failed("no".to_string()).ends_stream());
    }

    #[test]
    fn a_failed_credential_does_not_name_its_secret() {
        let error = ExternalAgentError::Credential { endpoint: "https://agent.example".to_string(), source: SecretError::NotFound("HERMES_TOKEN".to_string()) };
        assert!(!error.to_string().contains("HERMES_TOKEN"), "{error}");
        assert!(error.to_string().contains("https://agent.example"), "{error}");
    }

    #[tokio::test]
    async fn the_scripted_agent_answers_in_order_and_records_what_it_was_given() {
        let task = TaskId("t-1".to_string());
        let agent = ScriptedExternalAgent::answering(vec![vec![
            Ok(ExternalEvent::Conversation(ConversationId("c-1".to_string()))),
            Ok(ExternalEvent::Started(task.clone())),
            Ok(ExternalEvent::Message("hi".to_string())),
            Ok(ExternalEvent::Completed),
        ]]).resuming(vec![vec![Ok(ExternalEvent::Completed)]]);
        let security = SecurityContext::default();

        let events: Vec<_> = agent.send(&security, None, hello()).await.unwrap().take(4).collect().await;
        assert_eq!(events.len(), 4);
        assert!(matches!(events.last(), Some(Ok(ExternalEvent::Completed))));
        assert_eq!(agent.sent.lock().unwrap().as_slice(), &[(None, hello())]);

        let resumed: Vec<_> = agent.resume(&security, &task).await.unwrap().take(1).collect().await;
        assert!(matches!(resumed.as_slice(), [Ok(ExternalEvent::Completed)]), "resuming has a script of its own");

        agent.cancel(&security, &task).await.unwrap();
        assert_eq!(agent.cancelled.lock().unwrap().as_slice(), &[task]);

        let mut nothing_left = agent.send(&security, None, hello()).await.unwrap();
        let waited = tokio::time::timeout(Duration::from_millis(20), nothing_left.next()).await;
        assert!(waited.is_err(), "with no script left, the answer never comes");
    }
}
