//! The actor of an external agent (issue 179): an agent that runs elsewhere
//! and is reached through its personality's `ExternalAgent` component. It
//! has no turn, no tools, no transcript - the remote agent does all that.
//! What it does here is what every participant does: it is asked, it
//! answers, and it can be cancelled and stopped.
//!
//! It hears what the others say, as every participant does, and takes it in
//! as an agent run here does (issue 206): all of it into its history, with
//! its own answers; what can be given on - text others said - as it passes
//! its perceive processors, into its transcript (issue 196). When it is
//! asked, it gives the remote agent its transcript - which then starts anew,
//! so that it holds what the remote agent has not heard yet, and what was
//! given in each call stays as a branch of it, the context of that call's
//! answer (issue 205) - and publishes what comes back. That is all that is
//! known of it, and all that is kept: in its `AgentChannelState`, as every agent's, with what
//! ties it to the remote side (`RemoteState`, issue 206). It reads nothing
//! of the channel itself.
//!
//! Every request is a piece of work (`Work`), as a turn is (issues 211,
//! 212): done in the request's contexts, bound by the channel as it was
//! when it began, its answer classified as what it has seen - and given up
//! when the channel was lowered since.

use super::{catch_up, failure_message, lowered_message, perceive};
use crate::actor::{Actor, Answering};
use crate::component::{ComponentContainer, ComponentContext, ComponentImpl};
use crate::controller::AgentController;
use crate::external::{ExternalAgent, ExternalAgentResult, ExternalEvent, OutboundMessage, Said};
use crate::message::PerceiveInProcessor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::agent::{AgentId, Personality};
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::Classification;
use caelumora_model::context::ContextRef;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::external_agent::{OpenTaskState, TaskId, Work};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::security::{AccessQuestions, SecurityContext, SecurityManager};
use chrono::Utc;
use futures::stream::{BoxStream, SelectAll};
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
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
    /// Where it is, and in whose name it acts (issue 87): what it says on
    /// its own is sent in it.
    security_context: SecurityContext,
    /// The remote agent: exactly one (issue 179).
    remote: ComponentImpl<dyn ExternalAgent>,
    /// What filters what reaches it, as an agent's do (issue 196).
    perceive_in_processors: Vec<ComponentImpl<dyn PerceiveInProcessor>>,
    /// Its state in this channel, held by the run loop for as long as it
    /// runs.
    state: Mutex<AgentChannelState>,
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
    /// The channel was lowered to this classification since it began: it
    /// is given up, nothing of it is published (issue 212).
    Lowered(Classification),
    /// The actor was stopped meanwhile: nothing is published.
    Stopped,
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
        let perceive_in_processors = components.get_all::<dyn PerceiveInProcessor>(None);

        // Answers that may never enter the channel would each be dropped
        // there in silence: refused here instead, once - and once more with
        // every request, should the channel have been lowered since.
        let classification = remote.classification().unwrap_or_else(|| channel.classification.clone());
        if !may_answer_in(actor_runtime.security_manager().as_ref(), &security_context, ActorId::Agent(agent_id), &classification, &channel.classification).await {
            return Err(anyhow!(
                "the answers of external agent {} are classified {}, which may not be published in channel {} ({})",
                agent_id, classification, channel.id, channel.classification
            ));
        }

        let state = actor_runtime.load_channel_state().await?;
        Ok(Self {
            agent_id,
            agent_controller,
            actor_runtime,
            security_context,
            remote,
            perceive_in_processors,
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

    /// Takes in what others said (`perceive`), for the remote agent to be
    /// given when it is next asked; stored at once, so that a restart loses
    /// none of it.
    async fn hear(&self, state: &mut AgentChannelState, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }
        trace!("External agent {} hears {:?}", self.id(), messages.iter().map(|m| &m.content).collect::<Vec<_>>());
        self.perceive(state, messages).await;
        if let Err(e) = self.actor_runtime.store_channel_state(state).await {
            warn!("External agent {} cannot store what it heard: {:#}", self.id(), e);
        }
    }

    /// Takes `heard` into its state as an agent run here does (issues 196,
    /// 206): what can be given on to the remote agent - text others said
    /// (`worth_giving`) - as it passes its perceive processors.
    async fn perceive(&self, state: &mut AgentChannelState, heard: Vec<Message>) {
        let own = self.id();
        perceive(state, heard, |m| worth_giving(m, own), &self.perceive_in_processors, &self.security_context).await;
    }

    /// Answers the `ResponseRequested` in `request`. Whatever happens, the
    /// answer ends here: published, failed with `TurnFailed`, or - stopped -
    /// not at all; and the state is stored. An error ends the answer, never
    /// the actor.
    async fn respond(&self, state: &mut AgentChannelState, request: &Envelope, output: &dyn EventSender) {
        let answering = Answering::to(request, self.id(), self.name());
        let token = self.begin();
        let ending = self.exchange(state, &answering, &token, output).await;
        if let Err(e) = self.actor_runtime.store_channel_state(state).await {
            warn!("External agent {} cannot store its state in its channel: {:#}", self.id(), e);
        }
        let events = match ending {
            // A request is always answered, with nothing if need be.
            Ok(Ending::Answered(message)) => answered(message.map(|message| *message)),
            Ok(Ending::Failed(reason)) => {
                info!("External agent {} {} failed its task: {}", self.id(), self.name(), reason);
                vec![ActorEvent::TurnFailed { message: format!("{} could not answer: {reason}", self.name()) }]
            }
            Ok(Ending::Lowered(channel)) => {
                info!("External agent {} {} gives up its answer: the channel was lowered to {}", self.id(), self.name(), channel);
                vec![ActorEvent::TurnFailed { message: lowered_message(&self.name(), &channel) }]
            }
            Ok(Ending::Stopped) => Vec::new(),
            Err(e) => {
                tracing::error!("External agent {} {} could not answer: {:#}", self.id(), self.name(), e);
                vec![ActorEvent::TurnFailed { message: failure_message(&e) }]
            }
        };
        answering.send_all(events, output).await;
        self.set_processing(ActorState::Idle);
    }

    /// Gives the remote agent what it has not heard yet and collects what
    /// comes back, until the task ends, asks back, or `token` is cancelled.
    async fn exchange(
        &self,
        state: &mut AgentChannelState,
        answering: &Answering,
        token: &CancellationToken,
        output: &dyn EventSender,
    ) -> Result<Ending> {
        answering.send(ActorEvent::ResponseStarted, output).await?;
        let channel = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("external agent {} has no channel to answer in", self.id()))?;
        // The work of this exchange, as a turn: bound by the channel as it
        // is now, and written as what it has seen - what the remote agent
        // brings in itself, and all it was given in this channel (issue 212).
        let mut work = Work::new(answering.event_context.clone(), answering.security_context.clone(), channel.classification());
        let brought = self.remote.classification().unwrap_or_else(|| work.classification.clone());
        if !may_answer_in(self.actor_runtime.security_manager().as_ref(), &work.security_context, self.id(), &brought, &work.classification).await {
            return Ok(Ending::Failed(format!("its answers are classified {brought}, which may not be published in this channel ({})", work.classification)));
        }
        work.see(&brought);
        // All it was ever given: the calls before, each a branch of the
        // transcript, and what it is given now.
        state.transcript.all_entries().filter_map(|e| e.entry.message()).for_each(|m| work.see(&m.classification));
        let given = state.transcript.messages();
        // A task that asked back before the channel was lowered is not
        // taken up: it is cancelled, and the request begins a new one.
        if let Some(asking) = state.remote.asking().cloned() {
            if let Some(begun) = state.remote.work_of(&asking) {
                if self.lowered(&begun).await.is_some() {
                    info!("External agent {} does not take up the task that asked back: the channel was lowered since", self.id());
                    self.cancel_remote(state, &begun.security_context, &asking).await;
                }
            }
        }
        let answers = state.remote.asking().cloned();
        if given.is_empty() && answers.is_none() {
            debug!("External agent {} has nothing new to give; nothing to ask", self.id());
            return Ok(Ending::Answered(None));
        }
        let said = given.iter()
            .map(|m| Said { sender: m.sender_name.clone(), text: m.content.clone().unwrap_or_default() })
            .collect();
        trace!("External agent {} is given {:?}", self.id(), said);
        let mut stream = self.remote.send(&answering.security_context, state.remote.conversation.as_ref(), OutboundMessage { said, answers: answers.clone() }).await?;
        // Sent: from now on the remote agent has heard it, whatever comes
        // back - the transcript starts anew, and what was given is the
        // context of what comes of this call (issue 205). A send that failed
        // sends it again with the next call.
        work.context = Some(ContextRef { transcript: state.transcript.start_anew(), ..ContextRef::default() });

        let mut task = answers;
        let mut answer = Answer::default();
        loop {
            let next = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    if let Some(task) = &task {
                        self.cancel_remote(state, &work.security_context, task).await;
                    }
                    if self.ended.is_cancelled() {
                        return Ok(Ending::Stopped);
                    }
                    // A lowering cancels the channel's actors: what came so
                    // far is then not published, but given up (issue 212).
                    if let Some(channel) = self.lowered(&work).await {
                        return Ok(Ending::Lowered(channel));
                    }
                    break;
                }
                next = stream.next() => next,
            };
            let event = match next {
                Some(Ok(event)) => event,
                broken => {
                    match &broken {
                        Some(Err(e)) => warn!("External agent {}: the answer broke off before its task ended: {:#}", self.id(), e),
                        _ => warn!("External agent {}: the answer ended before its task did", self.id()),
                    }
                    // The remote side owes an end; without one the task is
                    // left open, as far as anyone knows, and followed again:
                    // what came so far is kept with it and said with the
                    // rest when it ends - not now, cut off, and then again
                    // (issue 195). The request is answered with nothing.
                    if let Some(open) = &task {
                        state.remote.set_unsaid(open, answer.finish());
                        return Ok(Ending::Answered(None));
                    }
                    // No task to follow: what came is all there is.
                    match broken {
                        Some(Err(e)) => return Err(e.into()),
                        _ => break,
                    }
                }
            };
            match answer.take(state, task.as_ref(), event) {
                Step::Continue => {}
                // The task it began - or the one it answers, taken up again -
                // done in the request's contexts from now on. Stored at once,
                // with what was given for it: a crash while the task works
                // leaves it to be followed after the restart, not forgotten
                // while the remote agent goes on with it.
                Step::Started(started) => {
                    state.remote.begin(&started, &work);
                    task = Some(started);
                    if let Err(e) = self.actor_runtime.store_channel_state(state).await {
                        warn!("External agent {} cannot store the task it began: {:#}", self.id(), e);
                    }
                }
                // Shown as it is written, to whoever waits for the answer.
                Step::Partial(piece) => {
                    let partial = self.message(Some(piece), work.label());
                    answering.send(ActorEvent::PartialMessageGenerated(partial), output).await?;
                }
                Step::Ended => break,
                Step::Failed(reason) => return Ok(Ending::Failed(reason)),
            }
        }

        Ok(Ending::Answered(self.record(state, answer, work.label(), work.context).map(Box::new)))
    }

    /// Re-attaches to every task left working that is not followed yet
    /// (`ExternalAgent::resume`), in the authority it is done with
    /// (`acting_for`): after a restart, or when the answer to a request
    /// ended before its task did. A task is re-attached once per
    /// run of the actor, so that a remote side that keeps breaking off does
    /// not keep it busy; the next run tries again. A task that asked back
    /// waits for the next request instead. A task whose channel was lowered
    /// since it began is given up instead of followed (`give_up_task`).
    async fn follow_open_tasks(&self, state: &mut AgentChannelState, followed: &mut Followed, attempted: &mut HashSet<TaskId>, output: &dyn EventSender) {
        let working = state.remote.working().cloned();
        for task in working.collect::<Vec<_>>() {
            if !attempted.insert(task.clone()) {
                continue;
            }
            if let Some(work) = state.remote.work_of(&task) {
                if let Some(channel) = self.lowered(&work).await {
                    self.give_up_task(state, &task, &work, channel, output).await;
                    continue;
                }
            }
            match self.remote.resume(&self.acting_for(state, &task), &task).await {
                Ok(stream) => {
                    debug!("External agent {} follows its open task again", self.id());
                    let tag = task.clone();
                    let tagged = stream.map(move |event| (tag.clone(), Some(event)))
                        .chain(futures::stream::once(async move { (task, None) }));
                    followed.push(Box::pin(tagged));
                }
                Err(e) => warn!("External agent {} cannot follow its open task: {:#}", self.id(), e),
            }
        }
    }

    /// Takes in `event` of the followed `task` - `None` once its stream
    /// ends. When the task has ended or asks back, what came of it is said
    /// in the channel (`speak`); `pending` holds what came so far of each.
    async fn on_followed(&self, state: &mut AgentChannelState, task: TaskId, event: Option<ExternalAgentResult<ExternalEvent>>, pending: &mut HashMap<TaskId, Answer>, output: &dyn EventSender) {
        // Read before the task may be closed by what comes of it.
        let work = state.remote.work_of(&task);
        let answer = pending.entry(task.clone()).or_insert_with(|| Answer::after(state.remote.unsaid_of(&task)));
        let outcome = match event {
            // It is known to be open and working, and nobody waits for its
            // pieces: what it says is said once it ends.
            Some(Ok(event)) => match answer.take(state, Some(&task), event) {
                Step::Continue | Step::Started(_) | Step::Partial(_) => return,
                Step::Ended => Ok(()),
                Step::Failed(reason) => Err(reason),
            },
            // The task has not ended: nothing is said of it. What came so
            // far waits for whatever comes after, if anything.
            Some(Err(e)) => {
                warn!("External agent {}: following its open task failed: {:#}", self.id(), e);
                return;
            }
            // Ended with the task still open, as far as anyone knows: what
            // came of it is dropped, not said cut off - the next run follows
            // it again and says it when it ends.
            None => {
                if pending.remove(&task).is_some_and(|a| !a.is_empty()) {
                    warn!("External agent {}: the open task's stream ended before the task did", self.id());
                }
                return;
            }
        };
        let answer = pending.remove(&task).unwrap_or_default();
        // Said now, if it asks back and stays open.
        state.remote.set_unsaid(&task, None);
        // Not said at all if its channel was lowered since it began.
        if let Some(work) = &work {
            if let Some(channel) = self.lowered(work).await {
                self.give_up_task(state, &task, work, channel, output).await;
                return;
            }
        }
        self.speak(state, work, outcome.map(|()| answer), output).await;
    }

    /// Says in the channel, on the agent's own account, what came of an
    /// open task - a long task finishing, a question back - or that it
    /// failed. It wakes nobody: sleeping agents hear it all the same, and
    /// only one it mentions is woken, to answer it (issue 199). What it
    /// says is kept in its history and stored first; a failure, as when it
    /// answers a request, keeps nothing of what came before it. It speaks as
    /// the task's `work` is done (`RemoteState::work_of`): with the
    /// authority of the request that began it, in the event context of the
    /// answer to it, continued - in its span, and as many hops between
    /// agents away from the last user's message as that answer (issue 45) -
    /// and classified as the work has seen (issue 212), with the context of
    /// the call that began the task or last took it up (issue 205). A task
    /// without it
    /// speaks afresh, as the agent itself, classified as its remote agent
    /// declares, else as the channel is now.
    async fn speak(&self, state: &mut AgentChannelState, work: Option<Work>, outcome: Result<Answer, String>, output: &dyn EventSender) {
        let Some(channel) = self.actor_runtime.channel_loop() else {
            warn!("External agent {} has no channel to speak in", self.id());
            return;
        };
        let (speaking, label, context) = match work {
            Some(work) => (self.speaking_for(&work), work.label(), work.context),
            None => (
                Answering {
                    event_context: EventContext::new(self.security_context.session_id, self.security_context.channel_id, self.id(), self.name()),
                    security_context: self.security_context.clone(),
                },
                self.remote.classification().unwrap_or_else(|| channel.classification()),
                None,
            ),
        };
        let events = match outcome {
            Err(reason) => vec![ActorEvent::TurnFailed { message: format!("{} could not finish: {reason}", self.name()) }],
            // Nobody asked: it says nothing if it has nothing to say.
            Ok(answer) => match self.record(state, answer, label, context) {
                Some(message) => answered(Some(message)),
                None => Vec::new(),
            },
        };
        if let Err(e) = self.actor_runtime.store_channel_state(state).await {
            warn!("External agent {} cannot store its state in its channel: {:#}", self.id(), e);
        }
        speaking.send_all(events, output).await;
    }

    /// The contexts what comes of `work` later is said in: those of the
    /// answer to the request that began it, continued.
    fn speaking_for(&self, work: &Work) -> Answering {
        Answering { event_context: work.event_context.next(self.id(), self.name()), security_context: work.security_context.clone() }
    }

    /// The channel's classification as it is now, if `work` - begun under
    /// another - may no longer flow into it (`AccessQuestions::may_publish`,
    /// in the work's name, as `AgentActor::turn_flow`): then it is given up
    /// (issue 212). `None` while it flows, or without a channel.
    async fn lowered(&self, work: &Work) -> Option<Classification> {
        let channel = self.actor_runtime.channel_loop()?.classification();
        let flows = self.actor_runtime.security_manager().may_publish(&work.security_context, self.id(), &work.classification, &channel).await;
        (!flows).then_some(channel)
    }

    /// Gives up the open `task`, done as `work`, whose channel was lowered
    /// to `channel` since it began (issue 212): nothing of what came of it
    /// is said, the remote agent is asked to cancel it if it has not ended,
    /// it is forgotten and stored so, and the users are told in its
    /// contexts, as for a turn.
    async fn give_up_task(&self, state: &mut AgentChannelState, task: &TaskId, work: &Work, channel: Classification, output: &dyn EventSender) {
        info!("External agent {} {} gives up its open task: the channel was lowered to {}", self.id(), self.name(), channel);
        if state.remote.work_of(task).is_some() {
            self.cancel_remote(state, &work.security_context, task).await;
        }
        if let Err(e) = self.actor_runtime.store_channel_state(state).await {
            warn!("External agent {} cannot store its state in its channel: {:#}", self.id(), e);
        }
        self.speaking_for(work).send_all(vec![ActorEvent::TurnFailed { message: lowered_message(&self.name(), &channel) }], output).await;
    }

    /// The authority the open `task` is done with: that of the request that
    /// began it, as a turn acts with its request's (issue 211) - else, for a
    /// task stored before it was kept, the agent's own.
    fn acting_for(&self, state: &AgentChannelState, task: &TaskId) -> SecurityContext {
        state.remote.work_of(task).map(|work| work.security_context).unwrap_or_else(|| self.security_context.clone())
    }

    /// Asks the remote agent to cancel `task`, and forgets it: a cancel that
    /// does not get through leaves a task nobody waits for any more.
    async fn cancel_remote(&self, state: &mut AgentChannelState, security: &SecurityContext, task: &TaskId) {
        if let Err(e) = self.remote.cancel(security, task).await {
            warn!("External agent {} cannot cancel its task: {:#}", self.id(), e);
        }
        state.remote.close(task);
    }

    /// The message `answer` comes to - `None` if nothing came - kept in its
    /// history with its `context`: what the remote agent was given in the
    /// call it came of (`Work::context`), if that is known.
    fn record(&self, state: &mut AgentChannelState, answer: Answer, classification: Classification, context: Option<ContextRef>) -> Option<Message> {
        let message = self.message(Some(answer.finish()?), classification);
        trace!("External agent {} answers {:?}", self.id(), message.content);
        state.history.push(message.clone(), context);
        Some(message)
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

/// The streams of the open tasks an actor follows, each event with its
/// task, and `None` when the stream ends.
type Followed = SelectAll<BoxStream<'static, (TaskId, Option<ExternalAgentResult<ExternalEvent>>)>>;

/// What came back so far.
#[derive(Default)]
struct Answer {
    /// What came of the task before - while it was asked - and was not said
    /// yet (issue 195).
    unsaid: Option<String>,
    /// The whole messages, in order.
    parts: Vec<String>,
    /// The pieces of the message being written.
    partial: String,
}

/// What an event of the remote agent comes to, once taken in.
enum Step {
    /// Nothing more than taken in.
    Continue,
    /// A task began, or the task answered was taken up again.
    Started(TaskId),
    /// A piece of the message being written.
    Partial(String),
    /// The task ended or asks back: the answer is complete.
    Ended,
    /// The task failed or was refused, for the reason given.
    Failed(String),
}

impl Answer {
    /// The answer of a task of which `unsaid` came before.
    fn after(unsaid: Option<String>) -> Self {
        Self { unsaid, ..Self::default() }
    }

    /// Takes in `event` of `task` - the one it belongs to, if known - what it
    /// says into the answer, what it tells of the conversation and the task
    /// into `state`.
    fn take(&mut self, state: &mut AgentChannelState, task: Option<&TaskId>, event: ExternalEvent) -> Step {
        match event {
            ExternalEvent::Conversation(conversation) => {
                state.remote.conversation = Some(conversation);
                Step::Continue
            }
            ExternalEvent::Started(started) => Step::Started(started),
            ExternalEvent::Partial(piece) => {
                self.partial.push_str(&piece);
                Step::Partial(piece)
            }
            ExternalEvent::Message(text) => {
                self.add(text);
                Step::Continue
            }
            ExternalEvent::InputRequired(question) => {
                self.add(question);
                if let Some(task) = task {
                    state.remote.open(task, OpenTaskState::InputRequired);
                }
                Step::Ended
            }
            ExternalEvent::Completed | ExternalEvent::Canceled => {
                if let Some(task) = task {
                    state.remote.close(task);
                }
                Step::Ended
            }
            ExternalEvent::Failed(reason) => {
                if let Some(task) = task {
                    state.remote.close(task);
                }
                Step::Failed(reason)
            }
        }
    }

    /// Whether nothing came - since the task was followed.
    fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.partial.is_empty()
    }

    /// A whole message: it replaces the pieces of it that came before.
    fn add(&mut self, text: String) {
        self.partial.clear();
        if !text.is_empty() {
            self.parts.push(text);
        }
    }

    /// The answer as one text - with what was being written when it ended,
    /// and what came before and was not said yet - or `None` if nothing
    /// came. A remote agent that repeats what came before (A2A's `GetTask`
    /// gives a task's whole result) is not said to repeat it.
    fn finish(mut self) -> Option<String> {
        if !self.partial.is_empty() {
            self.parts.push(self.partial);
        }
        let text = (!self.parts.is_empty()).then(|| self.parts.join("\n\n"));
        match (self.unsaid, text) {
            (Some(unsaid), Some(text)) if !text.starts_with(&unsaid) => Some(format!("{unsaid}\n\n{text}")),
            (unsaid, text) => text.or(unsaid),
        }
    }
}

/// The events that publish an answer: the message, if there is one, and
/// the end of the response.
fn answered(message: Option<Message>) -> Vec<ActorEvent> {
    let complete = message.clone().map(ActorEvent::CompleteMessageGenerated);
    complete.into_iter().chain([ActorEvent::FullResponseGenerated(message.into_iter().collect())]).collect()
}

/// Whether what a remote agent brings in itself - classified `brought`, as
/// its component declares, else as the channel - may enter a channel
/// classified `channel`, put there by `agent` in `security`
/// (`AccessQuestions::may_publish`): asked when its actor is built, and
/// before every exchange, as the channel may have been lowered since
/// (issue 212).
async fn may_answer_in(policy: &dyn SecurityManager, security: &SecurityContext, agent: ActorId, brought: &Classification, channel: &Classification) -> bool {
    policy.may_publish(security, agent, brought, channel).await
}

/// Whether the remote agent is given `message`: said by another, with text
/// to give, and none of a plan's inner steps.
fn worth_giving(message: &Message, own: ActorId) -> bool {
    message.sender_id != own
        && !message.internal
        && matches!(message.role, MessageRole::User | MessageRole::Assistant)
        && message.content.as_deref().is_some_and(|c| !c.is_empty())
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

    /// Takes in what it missed as what it hears (issues 190, 196, 206); the
    /// remote agent is given it when it is next asked.
    async fn catch_up(&self, visible: Vec<Message>) -> Result<()> {
        let mut state = self.state.lock().await;
        let own = self.id();
        catch_up(&mut state, visible, |m| worth_giving(m, own), &self.perceive_in_processors, &self.security_context, &self.actor_runtime).await
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, output: Box<dyn EventSender>) -> Result<()> {
        let mut state = self.state.try_lock()
            .map_err(|_| anyhow!("ExternalAgentActor {} already running", self.id()))?;
        info!("ExternalAgentActor {} is starting", self.id());
        // The open tasks it follows between requests, and what came of them
        // so far. While it answers a request, what they send waits.
        let mut followed = Followed::new();
        let mut attempted = HashSet::new();
        let mut pending = HashMap::new();
        self.follow_open_tasks(&mut state, &mut followed, &mut attempted, &*output).await;
        loop {
            // What others said, a request to respond, or an open task's news.
            tokio::select! {
                _ = self.ended.cancelled() => break,
                request = rx.recv() => {
                    let Some(mut request) = request else { break };
                    let span = crate::logging::envelope_span(&request.context, request.event.kind());
                    match &mut request.event {
                        ActorEvent::FullResponseGenerated(messages) => self.hear(&mut state, std::mem::take(messages)).instrument(span).await,
                        ActorEvent::ResponseRequested => {
                            self.respond(&mut state, &request, &*output).instrument(span).await;
                            self.follow_open_tasks(&mut state, &mut followed, &mut attempted, &*output).await;
                        }
                        _ => {}
                    }
                    rx.commit().await?;
                }
                Some((task, event)) = followed.next(), if !followed.is_empty() => {
                    self.on_followed(&mut state, task, event, &mut pending, &*output).await;
                }
            }
        }
        // Left the channel: nobody waits for its open tasks any more. A
        // channel that merely stops keeps them, to be followed again.
        if self.ended.is_cancelled() {
            let working: Vec<TaskId> = state.remote.open_tasks.iter().map(|t| t.task.clone()).collect();
            for task in working {
                let acting = self.acting_for(&state, &task);
                self.cancel_remote(&mut state, &acting, &task).await;
            }
            if let Err(e) = self.actor_runtime.store_channel_state(&mut state).await {
                warn!("External agent {} cannot store its state in its channel: {:#}", self.id(), e);
            }
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

    #[tokio::test]
    async fn all_it_hears_is_kept_and_what_others_said_with_text_is_given_once() {
        let own = ActorId::Agent(AgentId::new());
        let alice = ActorId::User(caelumora_model::user::UserId::new());
        let first = said(alice, "alice", "hello");
        let mine = Message { role: MessageRole::Assistant, ..said(own, "hermes", "hi") };
        let second = said(alice, "alice", "how are you?");
        let empty = Message { content: None, ..said(alice, "alice", "") };
        let step = Message { internal: true, ..said(alice, "alice", "thinking") };
        let ids = |messages: Vec<Message>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let security = SecurityContext::default();
        let takes = |m: &Message| worth_giving(m, own);

        let mut state = AgentChannelState::default();
        perceive(&mut state, vec![first.clone(), mine.clone()], takes, &[], &security).await;
        assert_eq!(ids(state.transcript.messages()), vec![first.id], "never sent anything: what others said");
        state.transcript.start_anew();
        perceive(&mut state, vec![empty.clone(), step.clone(), second.clone()], takes, &[], &security).await;
        assert_eq!(ids(state.history.messages()), vec![first.id, mine.id, empty.id, step.id, second.id], "all it heard is kept");
        assert_eq!(ids(state.transcript.messages()), vec![second.id], "after what was sent, with text");
        state.transcript.start_anew();
        assert!(state.transcript.messages().is_empty(), "nothing new");
    }

    #[test]
    fn what_the_remote_agent_sends_is_taken_into_the_answer_and_the_state() {
        let task = TaskId("t-1".to_string());
        let mut state = AgentChannelState::default();
        state.remote.open(&task, OpenTaskState::Working);
        let mut answer = Answer::default();

        assert!(matches!(answer.take(&mut state, Some(&task), ExternalEvent::Partial("Wh".to_string())), Step::Partial(p) if p == "Wh"));
        assert!(matches!(answer.take(&mut state, Some(&task), ExternalEvent::InputRequired("Which city?".to_string())), Step::Ended));
        assert_eq!(state.remote.open_tasks[0].state, OpenTaskState::InputRequired, "asking back keeps the task open");
        assert_eq!(answer.finish().as_deref(), Some("Which city?"), "the question replaces its pieces");

        let mut answer = Answer::default();
        assert!(matches!(answer.take(&mut state, Some(&task), ExternalEvent::Failed("no".to_string())), Step::Failed(r) if r == "no"));
        assert!(state.remote.open_tasks.is_empty(), "a failed task is forgotten");
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

    #[test]
    fn what_came_before_is_said_with_the_rest_but_not_twice() {
        let unsaid = || Some("Here it".to_string());
        let mut repeated = Answer::after(unsaid());
        repeated.add("Here it is.".to_string());
        assert_eq!(repeated.finish().as_deref(), Some("Here it is."), "the remote agent repeated it");
        let mut new = Answer::after(unsaid());
        new.add("is.".to_string());
        assert_eq!(new.finish().as_deref(), Some("Here it\n\nis."), "the remote agent only went on");
        assert_eq!(Answer::after(unsaid()).finish().as_deref(), Some("Here it"), "nothing came after it");
    }
}
