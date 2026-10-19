//! The actor of an external agent (issue 179): an agent that runs elsewhere
//! and is reached through its personality's `ExternalAgent` component. It
//! has no turn, no tools, no transcript - the remote agent does all that.
//! What it does here is what every participant does: it is asked, it
//! answers, and it can be cancelled and stopped.
//!
//! It hears what the others say, as every participant does, and takes it in
//! as an agent run here does (issue 206): all of it into its history, with
//! its own answers; what it hears of what others said - their words
//! (`Message::as_heard`, issue 224) - as it passes its perceive processors,
//! into its transcript (issue 196). When it is
//! asked, it gives the remote agent the prompt parts its context provider
//! assembles that it was not given yet - the frame of what was said, such
//! as who is in the channel (issues 204, 214) - and its transcript - which then starts anew, so
//! that it holds what the remote agent has not heard yet, and what was
//! given in each call stays as a branch of it, the context of that call's
//! answer (issue 205) - and publishes what comes back. That is all that is
//! known of it, and all that is kept: in its `AgentChannelState`, as every
//! agent's, with what ties it to the remote side (`RemoteState`, issue
//! 206). What it reads beyond that, its prompt providers read, as for a
//! turn (`Situation`).
//!
//! Every request is a piece of work (`Work`), as a turn is (issues 211,
//! 212): done in the request's contexts, bound by the channel as it was
//! when it began, its answer classified as what it has seen - and given up
//! when the channel was lowered since.

use super::body::AgentBody;
use crate::error::failure_message;
use caelumora_runtime::actor::{Actor, Answering};
use crate::command::AgentCommandProvider;
use caelumora_runtime::component::ComponentImpl;
use crate::prompt::external_context_provider::EXTERNAL_CONTEXT_PROVIDER_KIND;
use crate::prompt::ContextProvider;
use caelumora_runtime::controller::{AgentController, PersonalityController};
use crate::external::{ExternalAgent, ExternalAgentResult, ExternalEvent, OutboundMessage, Said};
use caelumora_runtime::logging::envelope_span;
use crate::message::PerceiveInProcessor;
use caelumora_runtime::runtime::ActorRuntime;
use caelumora_runtime::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::chain::Position;
use caelumora_model::actor::{ActorDescription, ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::RetryRefused;
use caelumora_model::type_map::TypeKey;
use caelumora_model::confidentiality::Classification;
use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
use caelumora_model::command::Command;
use caelumora_model::context::PromptPart;
use caelumora_model::agent::AgentChannelState;
use caelumora_model::external_agent::{OpenTaskState, TaskId};
use caelumora_model::turn::GivenUpReason;
use caelumora_model::work::{TurnId, Work};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::transcript::TranscriptEntry;
use caelumora_model::security::SecurityContext;
use caelumora_model::value::name::Name;
use chrono::{Utc, DateTime};
use futures::future::BoxFuture;
use futures::stream::{BoxStream, SelectAll};
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn, Instrument};
use uuid::Uuid;
use caelumora_model::digest;

#[derive(Debug)]
pub struct ExternalAgentActor {
    /// What it is in the channel, as every agent is (issue 355): who it is,
    /// in whose name it acts, its state there, how it hears, its commands -
    /// what they stage is in its transcript, given to the remote agent with
    /// its next call. Its context provider gives the remote agent what it
    /// is given besides what was said - the frame of it, such as who is in
    /// the channel (issues 204, 214) -, by default
    /// `external_context_provider`. Where it stands is its run control's
    /// (issue 363): `Planning` while the remote agent works on an answer -
    /// a turn of its own, which `cancel` and `stop` end -, else `Idle`.
    body: AgentBody,
    /// The remote agent: exactly one (issue 179).
    remote: ComponentImpl<dyn ExternalAgent>,
    /// The prompt parts the remote agent has not been given yet, by what
    /// tells them apart (`part_key`), in the order they came: a part that
    /// changed, until a send that reaches the remote agent gave it - also
    /// when its provider, telling only changes, has nothing new by then. A
    /// plain lock, never held across an await.
    pending_prompts: Mutex<Vec<(String, PromptPart)>>,
}

/// What tells `part`, the `index`th the context provider gave, apart from
/// the others: the component it came from, else its place among them.
fn part_key(part: &PromptPart, index: usize) -> String {
    part.source.clone().unwrap_or_else(|| format!("#{index}"))
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
    /// The actor left the channel meanwhile: the answer is given up,
    /// nothing of it published, and the channel told (issue 262).
    Left,
    /// The channel stopped meanwhile (issue 377): the remote task goes on,
    /// what came of it so far kept with it, and the answer is said when it
    /// ends - followed again when the channel runs again. Nothing is said
    /// now.
    Stopped,
}

impl ExternalAgentActor {
    /// The facets it asks its components for (issue 337) - its commands
    /// through its body.
    pub const COMPONENT_FACETS: &[TypeKey] = &[
        TypeKey::of::<dyn ExternalAgent>(),
        TypeKey::of::<dyn PerceiveInProcessor>(),
        TypeKey::of::<dyn ContextProvider>(),
        TypeKey::of::<dyn AgentCommandProvider>(),
    ];

    /// The actor of the external agent `agent_controller` holds - the
    /// session's controller of it, shared with its actors in the session's
    /// other channels (issue 120) - built from the personality `personality`
    /// holds, the instance's controller of it (issue 406), in the channel of
    /// `actor_runtime`.
    ///
    /// # Errors
    /// If its personality provides no `ExternalAgent` - or more than one -
    /// or its answers, as the component classifies them, may not be
    /// published in the channel; whatever its state store reports.
    pub async fn new(agent_controller: Arc<AgentController>, personality: Arc<PersonalityController>, actor_runtime: ActorRuntime) -> Result<Self> {
        let channel = Arc::clone(&actor_runtime.channel_controller().load());
        // Its context provider is, unless it configures one, not a turn's.
        let defaults = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new(EXTERNAL_CONTEXT_PROVIDER_KIND, EXTERNAL_CONTEXT_PROVIDER_KIND)],
        };
        // It resumes no answer: the remote agent's tasks are followed again
        // (`follow_open_tasks`), and it starts idle.
        let resume = |state: &mut AgentChannelState| state.processing_state = ActorState::Idle;
        let (body, components) = AgentBody::build(agent_controller, personality, Self::COMPONENT_FACETS, &defaults, actor_runtime, resume).await?;
        let remote = components.one::<dyn ExternalAgent>().await?;

        // Answers that may never enter the channel would each be dropped
        // there in silence: refused here instead, once - and once more with
        // every request, should the channel have been lowered since.
        let classification = remote.classification().unwrap_or_else(|| channel.classification.clone());
        // What it brings in itself - classified as its component declares,
        // else as the channel - has to enter the channel: asked here, and
        // before every exchange, as the channel may have been lowered since
        // (issue 212).
        if !body.runtime.confidentiality().may_publish(&body.security_context(), &classification, &channel.classification) {
            return Err(anyhow!(
                "the answers of external agent {} are classified {}, which may not be published in channel {} ({})",
                body.agent_id(), classification, channel.id, channel.classification
            ));
        }

        Ok(Self {
            // A change of a message it was given is told by a notice (issue 236).
            body: body.handing_over(tell),
            remote,
            pending_prompts: Mutex::new(Vec::new()),
        })
    }

    /// The prompt parts for `work` the remote agent has not been given yet
    /// (issues 204, 214), from its context provider: each that is not what
    /// it was given last of it (`RemoteState::given_prompts`), and each that
    /// a send which failed did not give - in the order they came. The work
    /// has seen all the parts and the knowledge they hold, given now or
    /// before: the remote agent knows them.
    ///
    /// # Errors
    /// If its context provider cannot assemble them.
    async fn prompts_to_give(&self, state: &AgentChannelState, work: &mut Work) -> Result<Vec<(String, PromptPart)>> {
        let (parts, knowledge) = self.body.current_context(&self.body.situation(work, state)).await?;
        work.take_in_context(&parts, knowledge);
        let mut pending = self.pending_prompts();
        for (index, part) in parts.into_iter().enumerate() {
            let key = part_key(&part, index);
            let due = !state.remote.has_given(&key, &digest::prompt(&part.text));
            match pending.iter().position(|(k, _)| *k == key) {
                Some(at) if due => pending[at].1 = part,
                Some(at) => { pending.remove(at); }
                None if due => pending.push((key, part)),
                None => {}
            }
        }
        Ok(pending.clone())
    }

    fn pending_prompts(&self) -> MutexGuard<'_, Vec<(String, PromptPart)>> {
        self.pending_prompts.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Moves it to `state` and tells the users at once, if that is a move
    /// (issue 243): an answer begins and ends as it happens.
    async fn go_to(&self, state: ActorState, output: &dyn EventSender) {
        self.body.control.go_to(state);
        self.body.announce_state(output).await;
    }

    /// Takes in `received` and everything queued behind it as an agent run
    /// here does (`take_in`, issues 196, 206, 222, 224): what it hears of
    /// what others said, as it passes its perceive processors, for the
    /// remote agent to be given when it is next asked; stored at once, so
    /// that a restart loses none of it. Returns the last request to
    /// respond, if any. A request to answer again is rejected (issue 234):
    /// the remote agent keeps what it was given, and nothing here can take
    /// its answer back from it. A message it was given that was modified
    /// or taken back since is told to it by a notice (`tell`, issue 236). A
    /// command is carried out where it came (`AgentBody::hear`, issue 269).
    async fn hear(&self, state: &mut AgentChannelState, received: Envelope, rx: &mut Box<dyn EventReceiver>, output: &dyn EventSender) -> Option<Envelope> {
        let taken = self.body.hear(state, received, rx, output).await;
        let (request, retries) = (taken.request, taken.retries);
        self.body.store(state).await;
        self.body.reject_retries(retries, RetryRefused::Elsewhere, output).await;
        request
    }

    /// Answers the `ResponseRequested` in `request`. Whatever happens, the
    /// answer ends here - published, or given up with `TurnGivenUp` -, or
    /// its channel stops and it ends with the remote task it waits for
    /// (issue 377); and the state is stored. An error ends the
    /// answer, never the actor.
    async fn respond(&self, state: &mut AgentChannelState, request: &Envelope, output: &dyn EventSender) {
        let answering = Answering::to(request, self.id(), self.name(), &self.body.security_context());
        let classification = self.body.runtime.channel_classification();
        // The work of this exchange, as a turn: bound by the channel as it
        // is now (issue 212), and how it ends is told as its turn's end.
        let mut work = Work::new(answering.event_context.clone(), answering.security_context.clone(), classification);
        // A turn of its own, whose token `cancel`, `stop` and `leave` reach.
        self.body.control.begin_turn(work.turn_id);
        let token = self.body.control.token();
        self.go_to(ActorState::Planning, output).await;
        let ending = self.exchange(state, &answering, &mut work, &token, output).await;
        self.body.store(state).await;
        let events = match ending {
            // A request is always answered, with nothing if need be.
            Ok(Ending::Answered(message)) => answered(Some(work.turn_id), message.map(|message| *message)),
            Ok(Ending::Failed(reason)) => {
                info!("External agent {} {} failed its task: {}", self.id(), self.name(), reason);
                vec![ActorEvent::TurnGivenUp { turn_id: work.turn_id, reason: GivenUpReason::Failed { message: format!("{} could not answer: {reason}", self.name()) } }]
            }
            Ok(Ending::Lowered(channel)) => {
                info!("External agent {} {} gives up its answer: the channel was lowered to {}", self.id(), self.name(), channel);
                vec![ActorEvent::TurnGivenUp { turn_id: work.turn_id, reason: GivenUpReason::Lowered { channel } }]
            }
            Ok(Ending::Left) => {
                info!("External agent {} {} gives up its answer: it left the channel", self.id(), self.name());
                vec![ActorEvent::TurnGivenUp { turn_id: work.turn_id, reason: GivenUpReason::Left }]
            }
            Ok(Ending::Stopped) => {
                info!("External agent {} {} leaves its answer to its remote task: the channel stops", self.id(), self.name());
                Vec::new()
            }
            Err(e) => {
                tracing::error!("External agent {} {} could not answer: {:#}", self.id(), self.name(), e);
                vec![ActorEvent::TurnGivenUp { turn_id: work.turn_id, reason: GivenUpReason::Failed { message: failure_message(&e) } }]
            }
        };
        answering.send_all(events, output).await;
        self.go_to(ActorState::Idle, output).await;
    }

    /// Gives the remote agent what it has not heard yet and collects what
    /// comes back, until the task ends, asks back, or `token` is cancelled.
    async fn exchange(
        &self,
        state: &mut AgentChannelState,
        answering: &Answering,
        work: &mut Work,
        token: &CancellationToken,
        output: &dyn EventSender,
    ) -> Result<Ending> {
        // `work` is written as what it has seen - what the remote agent
        // brings in itself, and all it was given in this channel (issue 212).
        answering.send(ActorEvent::ResponseStarted { turn_id: work.turn_id }, output).await?;
        // How it is given what was said, and in which time zone, recorded
        // with the context of what comes of it, so that the context is
        // resolved as it was given (issues 135, 424).
        work.context.frame.profile = self.remote.profile();
        work.context.frame.time_zone = self.body.runtime.session_time_zone();
        let brought = self.remote.classification().unwrap_or_else(|| work.classification.clone());
        if !self.body.runtime.confidentiality().may_publish(&work.security_context, &brought, &work.classification) {
            return Ok(Ending::Failed(format!("its answers are classified {brought}, which may not be published in this channel ({})", work.classification)));
        }
        work.see(&brought);
        // All it was ever given: in the calls before, and what it is given
        // now - every message, and every change of one, as it may have used
        // either version (issue 236).
        state.classifications_given().iter().for_each(|c| work.see(c));
        let given = work.context.frame.profile.given_messages(&state.transcript, self.id(), work.context.frame.time_zone);
        // A task that asked back before the channel was lowered is not
        // taken up: it is cancelled, and the request begins a new one.
        if let Some(asking) = state.remote.asking().cloned() {
            if let Some(begun) = state.remote.work_of(&asking) {
                if self.lowered(&begun).is_some() {
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
        let prompts = self.prompts_to_give(state, work).await?;
        let mut parts = Vec::with_capacity(prompts.len());
        for (_, part) in &prompts {
            // Part of the context of what comes of this call; seen already.
            let reference = state.keep_prompt(part.role, part.text.clone(), part.after, part.classification.clone());
            work.context.frame.prompts.push(reference);
            parts.push(part.text.clone());
        }
        let said = given.into_iter()
            .map(|m| Said { sender: m.sender_name, text: m.content.unwrap_or_default() })
            .collect();
        trace!("External agent {} is given {:?} after {:?}", self.id(), said, parts);
        let mut stream = self.remote.send(&answering.security_context, state.remote.conversation.as_ref(), OutboundMessage { prompts: parts, said, answers: answers.clone() }).await?;
        // Sent: from now on the remote agent has heard it, whatever comes
        // back - its prompt parts are given, the transcript starts anew, and
        // what was given is the context of what comes of this call (issue
        // 205). A send that failed sends it again with the next call.
        for (key, part) in prompts {
            state.remote.give(key, digest::prompt(&part.text));
        }
        self.pending_prompts().clear();
        work.context.transcript = state.transcript.start_anew();
        // The task that asked back is taken up by this call, whether or not
        // the remote agent tells it as begun again (issue 213).
        if let Some(asked) = &answers {
            self.begin_task(state, asked, work).await;
        }

        let mut task = answers;
        let mut answer = Answer::default();
        loop {
            let next = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    // The channel stops (issue 377): the remote task is
                    // left to work on, and what came of it so far is kept
                    // with it, as when the answer breaks off (issue 195).
                    // Without a task there is nothing to follow: it ends as
                    // a cancel does.
                    if self.body.control.is_ending() && !self.body.control.has_left() {
                        if let Some(open) = &task {
                            state.remote.set_unsaid(open, answer.finish());
                            return Ok(Ending::Stopped);
                        }
                        break;
                    }
                    if let Some(task) = &task {
                        self.cancel_remote(state, &work.security_context, task).await;
                    }
                    if self.body.control.has_left() {
                        return Ok(Ending::Left);
                    }
                    // A lowering cancels the channel's actors: what came so
                    // far is then not published, but given up (issue 212).
                    if let Some(channel) = self.lowered(work) {
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
                // The task it began, done as this work from now on.
                Step::Started(started) => {
                    self.begin_task(state, &started, work).await;
                    task = Some(started);
                }
                // Shown as it is written, to whoever waits for the answer.
                Step::Partial(piece) => {
                    let partial = self.message(Some(piece), work.label(), Some(work.turn_id));
                    answering.send(ActorEvent::PartialMessageGenerated(partial), output).await?;
                }
                Step::Ended => break,
                Step::Failed(reason) => return Ok(Ending::Failed(reason)),
            }
        }

        Ok(Ending::Answered(self.record(state, answer, work.label(), Some(work)).map(Box::new)))
    }

    /// Records `task` as begun - or, asked back before, taken up again - by
    /// `work` (`RemoteState::begin`): done in the request's contexts from now
    /// on. Stored at once, with what was given for it: a crash while the
    /// task works leaves it to be followed after the restart, not forgotten
    /// while the remote agent goes on with it.
    async fn begin_task(&self, state: &mut AgentChannelState, task: &TaskId, work: &Work) {
        state.remote.begin(task, work);
        self.body.store(state).await;
    }

    /// Re-attaches to every task left working that is not followed yet
    /// (`ExternalAgent::resume`), in the authority it is done with
    /// (`acting_for`): after a restart, or when the answer to a request
    /// ended before its task did. A task is re-attached once per
    /// run of the actor, so that a remote side that keeps breaking off does
    /// not keep it busy; the next run tries again. A task that asked back
    /// waits for the next request instead. A task whose channel was lowered
    /// since it began is given up instead of followed (`give_up_task`).
    /// Nothing is followed once the run is to end (issue 377): the answer
    /// its channel's stop ended is left to the next run.
    async fn follow_open_tasks(&self, state: &mut AgentChannelState, followed: &mut Followed, attempted: &mut HashSet<TaskId>, output: &dyn EventSender) {
        if self.body.control.is_ending() {
            return;
        }
        let working = state.remote.working().cloned();
        for task in working.collect::<Vec<_>>() {
            if !attempted.insert(task.clone()) {
                continue;
            }
            if let Some(work) = state.remote.work_of(&task) {
                if let Some(channel) = self.lowered(&work) {
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
            if let Some(channel) = self.lowered(work) {
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
        let channel = self.body.runtime.channel_classification();
        let (answering, label) = match &work {
            Some(work) => (Answering::continuing(work), work.label()),
            None => (
                Answering::own(self.id(), self.name(), &self.body.security_context()),
                self.remote.classification().unwrap_or(channel),
            ),
        };
        let events = match outcome {
            // A task nobody asked for is work of its own.
            Err(reason) => vec![ActorEvent::TurnGivenUp { turn_id: work.as_ref().map_or_else(TurnId::new, |w| w.turn_id), reason: GivenUpReason::Failed { message: format!("{} could not finish: {reason}", self.name()) } }],
            // Nobody asked: it says nothing if it has nothing to say.
            Ok(answer) => match self.record(state, answer, label, work.as_ref()) {
                Some(message) => answered(work.as_ref().map(|w| w.turn_id), Some(message)),
                None => Vec::new(),
            },
        };
        self.body.store(state).await;
        answering.send_all(events, output).await;
    }

    /// The channel's classification as it is now, if `work` - begun under
    /// another - may no longer flow into it (`ConfidentialityPolicy::may_publish`,
    /// in the work's name, as `AgentActor::turn_flow`): then it is given up
    /// (issue 212). `None` while it flows.
    fn lowered(&self, work: &Work) -> Option<Classification> {
        let channel = self.body.runtime.channel_classification();
        let flows = self.body.runtime.confidentiality().may_publish(&work.security_context, &work.classification, &channel);
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
        self.body.store(state).await;
        Answering::continuing(work).send_all(vec![ActorEvent::TurnGivenUp { turn_id: work.turn_id, reason: GivenUpReason::Lowered { channel } }], output).await;
    }

    /// The authority the open `task` is done with: that of the request that
    /// began it, as a turn acts with its request's (issue 211) - else, for a
    /// task stored before it was kept, the agent's own.
    fn acting_for(&self, state: &AgentChannelState, task: &TaskId) -> SecurityContext {
        state.remote.work_of(task).map(|work| work.security_context).unwrap_or_else(|| self.body.security_context())
    }

    /// Asks the remote agent to cancel `task`, and forgets it: a cancel that
    /// does not get through leaves a task nobody waits for any more.
    async fn cancel_remote(&self, state: &mut AgentChannelState, security: &SecurityContext, task: &TaskId) {
        if let Err(e) = self.remote.cancel(security, task).await {
            warn!("External agent {} cannot cancel its task: {:#}", self.id(), e);
        }
        state.remote.close(task);
    }

    /// The message `answer` comes to - `None` if nothing came - classified
    /// `classification`, as what came of `work`, if that is known: of its
    /// turn (issue 235), with what the remote agent was given in the call it
    /// came of (`Work::context`) as what it was produced from, for its
    /// channel's history to hold (issue 417) - its frame kept once by digest
    /// in `state` (issue 426).
    fn record(&self, state: &mut AgentChannelState, answer: Answer, classification: Classification, work: Option<&Work>) -> Option<Message> {
        let message = self.message(Some(answer.finish()?), classification, work.map(|w| w.turn_id));
        let message = match work {
            Some(work) => message.produced_from_context(work.context.transcript, state.keep_frame(&work.context.frame)),
            None => message,
        };
        trace!("External agent {} answers {:?}", self.id(), message.content);
        Some(message)
    }

    /// A message of this agent with `content`, of the turn `turn_id`.
    fn message(&self, content: Option<String>, classification: Classification, turn_id: Option<TurnId>) -> Message {
        Message {
            role: MessageRole::Assistant,
            content,
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name().to_string(),
            timestamp: Utc::now(),
            turn_id,
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
                state.remote.converse_in(conversation);
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

/// Tells the remote agent of `change`, a modification or revocation the
/// transcript of `state` did not take (issue 236, handed over by `take_in`
/// where it came): of a message it was given - one the transcript holds on
/// a branch off its active path, in the conversation the remote agent
/// keeps - by a notice (`notice`) appended to the transcript, to be given
/// with the next call as any message is; or, if a notice of the message
/// was not given yet, by modifying that one, so that it is told once, of
/// the message as it stands then. Nothing for a message never given or
/// taken back where it was given, or while the remote agent keeps no
/// conversation: then it is given everything anew.
///
/// What the remote agent was given of the message is read from the store
/// (`ActorRuntime::transcript_message`, issue 380): it lies on a branch
/// before the active one, which the actor does not hold, and every branch
/// given is stored before the next change is taken in. A store that cannot
/// be read is logged, and nothing is told.
fn tell<'a>(runtime: &'a ActorRuntime, state: &'a mut AgentChannelState, change: TranscriptEntry) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let Some(id) = change.changed().filter(|_| state.remote.conversation.is_some()) else { return };
        match runtime.transcript_message(id).await {
            Ok(was) => give_notice(state, change, was),
            Err(e) => warn!("External agent cannot read message {} as it was given, to tell of its change: {:#}", id, e),
        }
    })
}

/// Tells the remote agent of `state` of `change` (`tell`), which it was
/// given the message of as `was` - `None`: never.
fn give_notice(state: &mut AgentChannelState, change: TranscriptEntry, was: Option<Message>) {
    if state.remote.conversation.is_none() {
        return;
    }
    let (id, now, at) = match change {
        TranscriptEntry::Modified { message } => (message.id, Some(message.clone()), message.timestamp),
        TranscriptEntry::Revoked { message, at } => (message, None, at),
        _ => return,
    };
    let Some(was) = was.filter(|was| was.revoked.is_none()) else { return };
    let mut told = notice(&was, now.as_ref(), at);
    match state.transcript.messages().into_iter().find(|m| m.notice_of == Some(id)) {
        Some(pending) => {
            told.id = pending.id;
            state.transcript.modify(told);
        }
        None => {
            state.transcript.push(told);
        }
    }
}

/// How much of the message it tells of a notice names it by (issue 236).
const NOTICE_QUOTE: usize = 200;

/// The notice to the remote agent that the message it was given as `was`
/// reads as `now` from `at` on - `None`: was taken back then (issue 236). A
/// message of its own (`Message::notice_of`), in the name and role of the
/// message's sender, so that it is marked as every message is and names
/// nobody itself; it names the message by what it said, shortened, and is
/// classified as what it quotes and what it says together - without the
/// quote where the two have no audience in common. Worded so that it only
/// tells: the remote agent decides what to make of it, and a filter may
/// take an instruction to disregard something for an injection.
fn notice(was: &Message, now: Option<&Message>, at: DateTime<Utc>) -> Message {
    let said = was.content.as_deref().unwrap_or_default();
    let mut quoted: String = said.chars().take(NOTICE_QUOTE).collect();
    if quoted.len() < said.len() {
        quoted.push('…');
    }
    let (content, classification) = match now {
        Some(now) => {
            let reads = now.content.as_deref().unwrap_or_default();
            match was.classification.join(&now.classification) {
                Some(both) => (format!("[changed an earlier message \"{quoted}\"; it now reads:] {reads}"), both),
                None => (format!("[changed an earlier message; it now reads:] {reads}"), now.classification.clone()),
            }
        }
        None => (format!("[took back an earlier message \"{quoted}\"; it no longer stands.]"), was.classification.clone()),
    };
    Message {
        id: MessageId::new(),
        timestamp: at,
        sender_id: was.sender_id,
        sender_name: was.sender_name.clone(),
        role: was.role,
        content: Some(content),
        notice_of: Some(was.id),
        ..Message::new(classification)
    }
}

/// The events that publish an answer: the message, if there is one, and -
/// of the work `turn_id` - the end of the turn. What it says unasked is of
/// no turn, and answered by itself (issue 258).
fn answered(turn_id: Option<TurnId>, message: Option<Message>) -> Vec<ActorEvent> {
    let complete = message.clone().map(ActorEvent::CompleteMessageGenerated);
    let finished = turn_id.map(|turn_id| ActorEvent::TurnFinished { turn_id, messages: message.into_iter().collect() });
    complete.into_iter().chain(finished).collect()
}


#[async_trait]
impl Actor for ExternalAgentActor {
    fn id(&self) -> ActorId {
        self.body.id()
    }

    fn name(&self) -> Name {
        self.body.name()
    }

    fn describe(&self) -> ActorDescription {
        self.body.describe()
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| matches!(event.event, ActorEvent::ResponseRequested | ActorEvent::CompleteMessageGenerated(_) | ActorEvent::MessageModified(_) | ActorEvent::MessageRevoked { .. } | ActorEvent::MessageReclassified { .. } | ActorEvent::RetryRequested { .. } | ActorEvent::CommandRequested { .. }
            | ActorEvent::ParticipantJoined { .. } | ActorEvent::ParticipantLeft { .. })
    }

    fn components(&self) -> ComponentConfig {
        self.body.components()
    }

    async fn commands(&self, issuer: &SecurityContext) -> Vec<Command> {
        self.body.visible_commands(issuer).await
    }

    /// Takes in what it missed as what it hears (issues 190, 196, 206); the
    /// remote agent is given it when it is next asked.
    async fn catch_up(&self, missed: Range<Position>) -> Result<()> {
        self.body.catch_up(missed).await
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, output: Box<dyn EventSender>) -> Result<()> {
        let mut state = self.body.state.try_lock()
            .map_err(|_| anyhow!("ExternalAgentActor {} already running", self.id()))?;
        info!("ExternalAgentActor {} is starting", self.id());
        // Whoever watches already learns where it starts.
        self.body.announce(self.body.control.state(), &*output).await;
        // The open tasks it follows between requests, and what came of them
        // so far. While it answers a request, what they send waits.
        let mut followed = Followed::new();
        let mut attempted = HashSet::new();
        let mut pending = HashMap::new();
        self.follow_open_tasks(&mut state, &mut followed, &mut attempted, &*output).await;
        loop {
            // What others said, a request to respond, or an open task's news.
            tokio::select! {
                () = self.body.control.ending() => break,
                received = rx.recv() => {
                    let Some(received) = received else { break };
                    // One answer to all that was queued, to the last request.
                    if let Some(request) = self.hear(&mut state, received, &mut rx, &*output).await {
                        let span = envelope_span(&request.context, request.event.kind());
                        self.respond(&mut state, &request, &*output).instrument(span).await;
                        self.follow_open_tasks(&mut state, &mut followed, &mut attempted, &*output).await;
                    }
                    rx.commit().await?;
                }
                Some((task, event)) = followed.next(), if !followed.is_empty() => {
                    self.on_followed(&mut state, task, event, &mut pending, &*output).await;
                }
            }
            // Where it stands now, if it moved without telling: after a
            // command, as an agent run here tells it.
            self.body.announce_state(&*output).await;
        }
        // Left the channel: nobody waits for its open tasks any more. A
        // channel that merely stops keeps them, to be followed again (issue
        // 377).
        if self.body.control.has_left() {
            let working: Vec<TaskId> = state.remote.open_tasks.iter().map(|t| t.task.clone()).collect();
            for task in working {
                let acting = self.acting_for(&state, &task);
                self.cancel_remote(&mut state, &acting, &task).await;
            }
            self.body.store(&mut state).await;
        }
        info!("ExternalAgentActor {} has finished", self.id());
        Ok(())
    }

    fn state(&self) -> ActorState {
        self.body.control.state()
    }

    fn cancel(&self) {
        info!("Cancelling external agent {} {}", self.id(), self.name());
        // Only an answer under way: between answers there is nothing to end.
        self.body.control.cancel();
    }

    fn stop(&self) {
        info!("Stopping external agent {} {}: its channel stops", self.id(), self.name());
        // The answer under way is left to its remote task, and `run` ends.
        self.body.control.stop();
    }

    fn leave(&self) {
        info!("Stopping external agent {} {}: it left the channel", self.id(), self.name());
        // The answer under way ends too, and so does `run`.
        self.body.control.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::channel::ChannelId;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::transcript::TranscriptStore;
    use caelumora_model::store::StateStore;
    use caelumora_model::user::UserId;

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

    /// Tells of `change` as `tell` does, reading what the remote agent was
    /// given of the message from `store`, which `state` is put into first,
    /// as the actor stores it before it takes a change in.
    async fn tell_from(store: &MemoryStore, state: &mut AgentChannelState, change: TranscriptEntry) {
        let agent = AgentId::default();
        let channel = ChannelId::default();
        store.put_agent_channel_state(agent, channel, state).await.expect("put");
        state.keep_window();
        let was = match change.changed() {
            Some(id) => store.message(agent, channel, id).await.expect("read"),
            None => None,
        };
        give_notice(state, change, was);
    }

    #[tokio::test]
    async fn a_change_of_a_message_given_is_told_by_a_notice_once_until_it_is_given() {
        use caelumora_model::external_agent::ConversationId;
        let alice = ActorId::User(UserId::new());
        let (first, second) = (said(alice, "alice", "first"), said(alice, "alice", "second"));
        let at = Utc::now();
        let modified = |text: &str| TranscriptEntry::Modified { message: Message { content: Some(text.to_string()), timestamp: at, ..first.clone() } };
        let told = |state: &AgentChannelState| state.transcript.messages().into_iter().map(|m| (m.notice_of, m.content.unwrap_or_default())).collect::<Vec<_>>();
        let store = MemoryStore::new();
        let mut state = AgentChannelState::default();
        state.transcript.extend(vec![first.clone(), second.clone()]);
        state.transcript.start_anew();

        tell_from(&store, &mut state, modified("first, corrected")).await;
        assert!(state.transcript.messages().is_empty(), "no conversation that holds it: nothing to tell");

        state.remote.conversation = Some(ConversationId("c-1".to_string()));
        for change in [modified("first, corrected"), TranscriptEntry::Revoked { message: second.id, at }, modified("first, corrected again"), TranscriptEntry::Revoked { message: MessageId::new(), at }] {
            tell_from(&store, &mut state, change).await;
        }
        assert_eq!(told(&state), [
            (Some(first.id), "[changed an earlier message \"first\"; it now reads:] first, corrected again".to_string()),
            (Some(second.id), "[took back an earlier message \"second\"; it no longer stands.]".to_string()),
        ], "each once, where it was first told, as it stands; nothing of what it never held");
        let notice = &state.transcript.messages()[0];
        assert_ne!(notice.id, first.id, "an id of its own");
        assert_eq!((notice.sender_id, notice.role, notice.sender_name.as_str()), (alice, MessageRole::User, "alice"), "in the name of the message's sender");

        state.transcript.start_anew();
        tell_from(&store, &mut state, modified("once more")).await;
        assert_eq!(told(&state), [(Some(first.id), "[changed an earlier message \"first\"; it now reads:] once more".to_string())], "after a call, a notice of its own, naming it as it was first given");

        let mut revoked = AgentChannelState::default();
        revoked.remote.conversation = Some(ConversationId("c-1".to_string()));
        revoked.transcript.push(first.clone());
        revoked.transcript.revoke(first.id, at);
        revoked.transcript.start_anew();
        tell_from(&MemoryStore::new(), &mut revoked, modified("again")).await;
        assert!(revoked.transcript.messages().is_empty(), "taken back where it was given");
    }

    #[test]
    fn a_notice_names_the_message_by_what_it_said_shortened_and_is_classified_as_both() {
        use caelumora_model::confidentiality::{Compartment, Confidentiality};
        let alice = ActorId::User(UserId::new());
        let at = Utc::now();
        let long = said(alice, "alice", &"ä".repeat(NOTICE_QUOTE + 1));
        let told = notice(&long, None, at).content.unwrap_or_default();
        assert!(told.contains(&format!("\"{}…\"", "ä".repeat(NOTICE_QUOTE))), "{told}");

        let hello = said(alice, "alice", "hello");
        let confidential = Classification::general(Confidentiality::Confidential);
        let raised = Message { classification: confidential.clone(), ..hello.clone() };
        assert_eq!(notice(&hello, Some(&raised), at).classification, confidential, "what it says and what it quotes");
        let of = |compartment: &str| Classification::new(Confidentiality::Internal, [Compartment::new(compartment)]).expect("a classification");
        let (was, now) = (Message { classification: of("hr"), ..hello.clone() }, Message { classification: of("sales"), ..hello });
        let apart = notice(&was, Some(&now), at);
        assert_eq!((apart.content.as_deref(), apart.classification), (Some("[changed an earlier message; it now reads:] hello"), of("sales")), "no audience in common: without the quote");
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
