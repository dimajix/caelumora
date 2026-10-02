//! The actor of an external agent (issue 179): an agent that runs elsewhere
//! and is reached through its personality's `ExternalAgent` component. It
//! has no turn, no tools, no transcript - the remote agent does all that.
//! What it does here is what every participant does: it is asked, it
//! answers, and it can be cancelled and stopped.
//!
//! It hears what the others say, as every participant does, and keeps it in
//! its history, with its own answers. When it is asked, it gives the remote
//! agent what it heard since it was last asked - kept as its transcript -
//! and publishes what comes back. That is all that is known of it, and all
//! that is kept (`ExternalAgentChannelState`); it reads nothing of the
//! channel itself.

use crate::actor::Actor;
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::controller::AgentController;
use crate::external::{ExternalAgent, ExternalEvent, OutboundMessage, Said};
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::agent::{AgentId, Personality};
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::Classification;
use caelumora_model::external_agent::{ExternalAgentChannelState, OpenTask, OpenTaskState, TaskId};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::security::{AccessQuestions, SecurityContext};
use chrono::Utc;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn, Instrument};
use uuid::Uuid;

#[derive(Debug)]
pub struct ExternalAgentActor {
    agent_id: AgentId,
    agent_controller: Arc<AgentController>,
    actor_runtime: ActorRuntime,
    /// The remote agent: exactly one (issue 179).
    remote: ComponentImpl<dyn ExternalAgent>,
    /// Its state in this channel, held by the run loop for as long as it
    /// runs.
    state: Mutex<ExternalAgentChannelState>,
    /// `Planning` while the remote agent works on an answer, else `Idle`.
    /// A plain lock, never held across an await.
    processing: std::sync::Mutex<ActorState>,
    /// Cancelled to end the answer under way (`cancel`, `stop`); replaced
    /// for every answer.
    current: std::sync::Mutex<CancellationToken>,
    /// Cancelled when the actor ends: it left the channel.
    ended: CancellationToken,
}

/// How an exchange with the remote agent ended.
enum Ending {
    /// It answered - with nothing, if it had nothing to say or was
    /// cancelled before it said anything.
    Answered(Option<Box<Message>>),
    /// It failed or refused the task, for the reason given.
    Failed(String),
    /// The actor was stopped meanwhile: nothing is published.
    Stopped,
}

/// The contexts an answer is published in: those of the request, continued
/// in the agent's name.
struct Answering {
    event_context: EventContext,
    security_context: SecurityContext,
}

impl Answering {
    async fn send(&self, event: ActorEvent, output: &dyn EventSender) -> Result<()> {
        let Some(event) = event.published() else { return Ok(()) };
        output.send(Envelope::new(self.security_context.clone(), self.event_context.clone(), event)).await
            .context(format!("external agent {} failed to send an event", self.event_context.sender_id))
    }
}

impl ExternalAgentActor {
    /// The actor of the external agent `agent_controller` holds - the
    /// session's controller of it, shared with its actors in the session's
    /// other channels (issue 120) - built from its `personality`, as the
    /// channel read it for the agent.
    ///
    /// # Errors
    /// If its personality provides no `ExternalAgent` - or more than one -
    /// or its answers, as the component classifies them, may not be
    /// published in `channel`; whatever its state store reports.
    pub async fn new(channel: &Channel, agent_controller: Arc<AgentController>, personality: Personality, actor_runtime: ActorRuntime) -> Result<Self> {
        let agent = agent_controller.load();
        // It acts on behalf of its owner, in this channel, as an agent run
        // here does (issue 87).
        let security_context = agent.security_context(channel.session_id, channel.id);
        let agent_id = agent.id;
        drop(agent);

        let components = ComponentContainer::from_config(
            &personality.components,
            &actor_runtime.component_registry(),
            &ComponentContext::new(security_context.clone(), actor_runtime.security_manager(), actor_runtime.secret_manager()),
        ).await?;
        let remote = components.one::<dyn ExternalAgent>().await?;

        // Answers that may never enter the channel would each be dropped
        // there in silence: refused here instead, once.
        let classification = remote.classification().unwrap_or_else(|| channel.classification.clone());
        if !actor_runtime.security_manager().may_publish(&security_context, ActorId::Agent(agent_id), &classification, &channel.classification).await {
            return Err(anyhow!(
                "the answers of external agent {} are classified {}, which may not be published in channel {} ({})",
                agent_id, classification, channel.id, channel.classification
            ));
        }

        let state = actor_runtime.load_external_channel_state().await?;
        Ok(Self {
            agent_id,
            agent_controller,
            actor_runtime,
            remote,
            state: Mutex::new(state),
            processing: std::sync::Mutex::new(ActorState::Idle),
            current: std::sync::Mutex::new(CancellationToken::new()),
            ended: CancellationToken::new(),
        })
    }

    fn set_processing(&self, state: ActorState) {
        *self.processing.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    /// A fresh token for the answer that begins, which `cancel` reaches.
    fn begin(&self) -> CancellationToken {
        let token = self.ended.child_token();
        *self.current.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = token.clone();
        self.set_processing(ActorState::Planning);
        token
    }

    /// Keeps what others said in its history, all of it, for the record
    /// and for the remote agent to be given when it is next asked - stored
    /// at once, so that a restart loses none of it.
    async fn hear(&self, state: &mut ExternalAgentChannelState, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }
        trace!("External agent {} hears {:?}", self.id(), messages.iter().map(|m| &m.content).collect::<Vec<_>>());
        state.history.extend(messages);
        if let Err(e) = self.actor_runtime.store_external_channel_state(state).await {
            warn!("External agent {} cannot store what it heard: {:#}", self.id(), e);
        }
    }

    /// Answers the `ResponseRequested` in `request`. Whatever happens, the
    /// answer ends here: published, failed with `TurnFailed`, or - stopped -
    /// not at all; and the state is stored. An error ends the answer, never
    /// the actor.
    async fn respond(&self, state: &mut ExternalAgentChannelState, request: &Envelope, output: &dyn EventSender) {
        let answering = Answering {
            event_context: request.context.next(self.id(), self.name()),
            security_context: request.security.next(self.id()),
        };
        let token = self.begin();
        let ending = self.exchange(state, &answering, &token, output).await;
        if let Err(e) = self.actor_runtime.store_external_channel_state(state).await {
            warn!("External agent {} cannot store its state in its channel: {:#}", self.id(), e);
        }
        let published = match ending {
            Ok(Ending::Answered(message)) => {
                let message = message.map(|message| *message);
                if let Some(message) = &message {
                    if let Err(e) = answering.send(ActorEvent::CompleteMessageGenerated(message.clone()), output).await {
                        warn!("{:#}", e);
                    }
                }
                answering.send(ActorEvent::FullResponseGenerated(message.into_iter().collect()), output).await
            }
            Ok(Ending::Failed(reason)) => {
                info!("External agent {} {} failed its task: {}", self.id(), self.name(), reason);
                answering.send(ActorEvent::TurnFailed { message: format!("{} could not answer: {reason}", self.name()) }, output).await
            }
            Ok(Ending::Stopped) => Ok(()),
            Err(e) => {
                tracing::error!("External agent {} {} could not answer: {:#}", self.id(), self.name(), e);
                let message = crate::error::user_message(&e)
                    .unwrap_or_else(|| "internal error - the details are in the server log".to_string());
                answering.send(ActorEvent::TurnFailed { message }, output).await
            }
        };
        if let Err(e) = published {
            warn!("{:#}", e);
        }
        self.set_processing(ActorState::Idle);
    }

    /// Gives the remote agent what it has not heard yet and collects what
    /// comes back, until the task ends, asks back, or `token` is cancelled.
    async fn exchange(
        &self,
        state: &mut ExternalAgentChannelState,
        answering: &Answering,
        token: &CancellationToken,
        output: &dyn EventSender,
    ) -> Result<Ending> {
        answering.send(ActorEvent::ResponseStarted, output).await?;
        let channel = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("external agent {} has no channel to answer in", self.id()))?;
        let classification = self.remote.classification().unwrap_or_else(|| channel.classification());
        let unheard = unheard(state, self.id());
        let answers = state.open_tasks.iter().find(|t| t.state == OpenTaskState::InputRequired).map(|t| t.task.clone());
        if unheard.is_empty() && answers.is_none() {
            debug!("External agent {} has heard everything; nothing to ask", self.id());
            return Ok(Ending::Answered(None));
        }
        let said = unheard.iter()
            .map(|m| Said { sender: m.sender_name.clone(), text: m.content.clone().unwrap_or_default() })
            .collect();
        trace!("External agent {} is given {:?}", self.id(), said);
        let mut stream = self.remote.send(&answering.security_context, state.conversation.as_ref(), OutboundMessage { said, answers: answers.clone() }).await?;
        // Sent: from now on the remote agent has heard it, whatever comes
        // back.
        state.transcript.extend(unheard);

        let mut task = answers;
        let mut answer = Answer::default();
        loop {
            let next = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    if let Some(task) = &task {
                        self.cancel_remote(state, &answering.security_context, task).await;
                    }
                    if self.ended.is_cancelled() {
                        return Ok(Ending::Stopped);
                    }
                    break;
                }
                next = stream.next() => next,
            };
            let Some(event) = next else {
                // The remote side owes an end; without one the task is left
                // open, as far as anyone knows.
                warn!("External agent {}: the answer ended before its task did", self.id());
                break;
            };
            match event? {
                ExternalEvent::Conversation(conversation) => state.conversation = Some(conversation),
                ExternalEvent::Started(started) => {
                    set_open(state, &started, OpenTaskState::Working);
                    task = Some(started);
                }
                ExternalEvent::Partial(piece) => {
                    answer.partial.push_str(&piece);
                    let partial = self.message(Some(piece), classification.clone());
                    answering.send(ActorEvent::PartialMessageGenerated(partial), output).await?;
                }
                ExternalEvent::Message(text) => answer.add(text),
                ExternalEvent::InputRequired(question) => {
                    answer.add(question);
                    if let Some(task) = &task {
                        set_open(state, task, OpenTaskState::InputRequired);
                    }
                    break;
                }
                ExternalEvent::Completed | ExternalEvent::Canceled => {
                    close(state, task.as_ref());
                    break;
                }
                ExternalEvent::Failed(reason) => {
                    close(state, task.as_ref());
                    return Ok(Ending::Failed(reason));
                }
            }
        }

        let message = answer.finish().map(|text| Box::new(self.message(Some(text), classification)));
        if let Some(message) = &message {
            trace!("External agent {} answers {:?}", self.id(), message.content);
            state.history.extend(vec![Message::clone(message)]);
        }
        Ok(Ending::Answered(message))
    }

    /// Asks the remote agent to cancel `task`, and forgets it: a cancel that
    /// does not get through leaves a task nobody waits for any more.
    async fn cancel_remote(&self, state: &mut ExternalAgentChannelState, security: &SecurityContext, task: &TaskId) {
        if let Err(e) = self.remote.cancel(security, task).await {
            warn!("External agent {} cannot cancel its task: {:#}", self.id(), e);
        }
        close(state, Some(task));
    }

    /// A message of this agent with `content`.
    fn message(&self, content: Option<String>, classification: Classification) -> Message {
        Message {
            role: MessageRole::Assistant,
            content,
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name(),
            timestamp: Utc::now(),
            ..Message::new(classification)
        }
    }
}

/// What came back so far.
#[derive(Default)]
struct Answer {
    /// The whole messages, in order.
    parts: Vec<String>,
    /// The pieces of the message being written.
    partial: String,
}

impl Answer {
    /// A whole message: it replaces the pieces of it that came before.
    fn add(&mut self, text: String) {
        self.partial.clear();
        if !text.is_empty() {
            self.parts.push(text);
        }
    }

    /// The answer as one text - with what was being written when it ended -
    /// or `None` if nothing came.
    fn finish(mut self) -> Option<String> {
        if !self.partial.is_empty() {
            self.parts.push(self.partial);
        }
        (!self.parts.is_empty()).then(|| self.parts.join("\n\n"))
    }
}

/// Whether the remote agent is given `message`: said by another, with text
/// to give, and none of a plan's inner steps.
fn worth_giving(message: &Message, own: ActorId) -> bool {
    message.sender_id != own
        && !message.internal
        && matches!(message.role, MessageRole::User | MessageRole::Assistant)
        && message.content.as_deref().is_some_and(|c| !c.is_empty())
}

/// What the agent heard that the remote agent was not given yet: what came
/// after the last message of its transcript - everything, if it was never
/// given anything - and is worth giving.
fn unheard(state: &ExternalAgentChannelState, own: ActorId) -> Vec<Message> {
    let heard = state.history.messages();
    let after = state.transcript.messages().last()
        .and_then(|last| heard.iter().position(|m| m.id == last.id))
        .map_or(0, |last| last + 1);
    heard.into_iter().skip(after).filter(|m| worth_giving(m, own)).collect()
}

/// Records `task` as open, standing at `state`.
fn set_open(channel_state: &mut ExternalAgentChannelState, task: &TaskId, state: OpenTaskState) {
    match channel_state.open_tasks.iter_mut().find(|t| &t.task == task) {
        Some(open) => open.state = state,
        None => channel_state.open_tasks.push(OpenTask { task: task.clone(), since: Utc::now(), state }),
    }
}

/// Forgets `task`: it ended.
fn close(state: &mut ExternalAgentChannelState, task: Option<&TaskId>) {
    if let Some(task) = task {
        state.open_tasks.retain(|t| &t.task != task);
    }
}

#[async_trait]
impl Actor for ExternalAgentActor {
    fn id(&self) -> ActorId {
        ActorId::Agent(self.agent_id)
    }

    fn kind(&self) -> String {
        "external_agent".to_string()
    }

    fn name(&self) -> String {
        self.agent_controller.name()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| matches!(event.event, ActorEvent::ResponseRequested | ActorEvent::FullResponseGenerated(_))
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, output: Box<dyn EventSender>) -> Result<()> {
        let mut state = self.state.try_lock()
            .map_err(|_| anyhow!("ExternalAgentActor {} already running", self.id()))?;
        info!("ExternalAgentActor {} is starting", self.id());
        loop {
            // What others said, or a request to respond.
            let request = tokio::select! {
                _ = self.ended.cancelled() => break,
                request = rx.recv() => match request {
                    Some(request) => request,
                    None => break,
                },
            };
            let span = crate::logging::envelope_span(&request.context, request.event.kind());
            match &request.event {
                ActorEvent::FullResponseGenerated(messages) => self.hear(&mut state, messages.clone()).instrument(span).await,
                ActorEvent::ResponseRequested => self.respond(&mut state, &request, &*output).instrument(span).await,
                _ => {}
            }
            rx.commit().await?;
        }
        info!("ExternalAgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        *self.processing.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn cancel(&self) {
        info!("Cancelling external agent {} {}", self.id(), self.name());
        self.current.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).cancel();
    }

    fn stop(&self) {
        info!("Stopping external agent {} {}: it left the channel", self.id(), self.name());
        // The answer under way is a child of it, and ends too.
        self.ended.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::Classification;

    fn said(sender: ActorId, name: &str, text: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: Some(text.to_string()),
            id: MessageId(Uuid::new_v4()),
            sender_id: sender,
            sender_name: name.to_string(),
            ..Message::new(Classification::CHANNEL_DEFAULT)
        }
    }

    #[test]
    fn the_remote_agent_is_given_what_others_said_since_it_was_last_given_anything() {
        let own = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(caelumora_model::user::UserId::new());
        let first = said(alice, "alice", "hello");
        let mine = Message { role: MessageRole::Assistant, ..said(own, "hermes", "hi") };
        let second = said(alice, "alice", "how are you?");
        let empty = Message { content: None, ..said(alice, "alice", "") };
        let step = Message { internal: true, ..said(alice, "alice", "thinking") };

        let mut state = ExternalAgentChannelState::default();
        state.history.extend(vec![first.clone(), mine, empty, step, second.clone()]);
        let ids = |state: &ExternalAgentChannelState| unheard(state, own).into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&state), vec![first.id, second.id], "never given anything: what others said, with text");
        state.transcript.extend(vec![first.clone()]);
        assert_eq!(ids(&state), vec![second.id], "after what it was given");
        state.transcript.extend(vec![second.clone()]);
        assert!(ids(&state).is_empty(), "nothing new");
    }

    #[test]
    fn an_answer_is_its_whole_messages_and_what_was_being_written() {
        let mut answer = Answer::default();
        answer.partial.push_str("Hel");
        answer.add("Hello.".to_string());
        answer.partial.push_str("How");
        assert_eq!(answer.finish().as_deref(), Some("Hello.\n\nHow"));
        assert_eq!(Answer::default().finish(), None, "nothing came");
    }
}
