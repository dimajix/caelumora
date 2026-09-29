use crate::actor::Actor;
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::user::UserActor;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::ChannelRuntime;
use crate::transport::{EventSender, TransportError};
use crate::transport_impl::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender};
use anyhow::{anyhow, Result};
use arc_swap::ArcSwap;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::channel::{Channel, ChannelController, ChannelError, ChannelId, ChannelState, HistoryAccess, HistoryStart, Participant};
use caelumora_model::confidentiality::ChannelClassification;
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::message::Message;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::{StateStore, StoreError};
use caelumora_model::user::UserId;
use tracing::{debug, info, trace, warn, Instrument};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

/// A participant of a channel, with the handle it is `@`-mentioned by.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMember {
    pub id: ActorId,
    pub name: String,
    /// The handle the participant is mentioned by (see `caelumora_model::mention`),
    /// or `None` if it cannot be mentioned - approvers, and names that yield
    /// no handle.
    pub handle: Option<String>,
    /// The actor kind, e.g. `"user"` or the agent's kind.
    pub kind: String,
}

struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
    sender: Box<dyn EventSender>,
}

/// How long a stopped actor gets to end its task by itself before it is
/// aborted (issue 75): a tool that hangs is the case this is for.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// A change to the actors a running channel runs (issue 46), carried out by
/// `run`, which owns their tasks.
enum ActorChange {
    Start(Arc<dyn Actor>),
    Stop(ActorId),
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    runtime: ChannelRuntime,
    /// The actors of the participants; replaced as a whole when someone
    /// joins or leaves (issue 46), so that every reader sees one set.
    actors: ArcSwap<HashMap<ActorId, Arc<dyn Actor>>>,
    moderator: Mutex<Box<dyn Moderator>>,
    mentions: ArcSwap<MentionDirectory>,
    /// Actors for `run` to start or stop; `run` takes the receiver.
    changes_tx: mpsc::UnboundedSender<ActorChange>,
    changes_rx: Mutex<mpsc::UnboundedReceiver<ActorChange>>,
    recorder: Mutex<Box<dyn Recorder>>,
    state: Arc<Mutex<ChannelState>>,
    state_store: Arc<dyn StateStore>,
    /// Set by `cancel`, cleared when the channel is woken: while set, what is
    /// said is still delivered, but nobody is asked to respond to it
    /// (`route_event_impl`). The turns themselves are cancelled on the actors.
    quiet: AtomicBool,
    event_tx: broadcast::Sender<Envelope>,
    max_route_decisions: i32,
    run_lock: Mutex<()>
}

impl ChannelLoop {
    /// Builds the channel and its actors, whose runtimes `runtime` creates and
    /// which reach the channel through it once it exists.
    pub async fn new(channel: Channel, runtime: ChannelRuntime) -> Result<Arc<Self>> {
        let id = channel.id;
        let session_id = channel.session_id;
        let state_store = runtime.state_store();
        let mut moderator = runtime.new_moderator(&channel.moderator)?;
        
        let state = match state_store.get_channel_state(id).await {
            Ok(s) => s,
            Err(StoreError::NotFound { entity: _, id: _ }) => {
                let state = ChannelState::new(id);
                state_store.put_channel_state(id, &state).await?;
                state
            }
            Err(e) => return Err(e.into()),
        };
        let state = Arc::new(Mutex::new(state));

        let recorder = runtime.new_recorder()?;
        let (event_tx, _) = broadcast::channel(100);

        let channel_controller = runtime.controller_manager().new_channel_controller(channel);

        let mut actors = HashMap::new();
        for participant in &channel_controller.load().participants {
            let actor = Self::create_actor(channel_controller.clone(), participant, &runtime).await?;
            let actor: Arc<dyn Actor> = Arc::from(actor);
            moderator.add_actor(actor.clone());
            actors.insert(actor.id(), actor);
        }
        let mentions = Self::mention_directory(&actors);
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", id, handle);
        }

        let (changes_tx, changes_rx) = mpsc::unbounded_channel();
        let channel_loop = Arc::new(Self {
            id,
            session_id,
            channel: channel_controller,
            runtime,
            actors: ArcSwap::from_pointee(actors),
            moderator: Mutex::new(moderator),
            mentions: ArcSwap::from_pointee(mentions),
            changes_tx,
            changes_rx: Mutex::new(changes_rx),
            recorder: Mutex::new(recorder),
            state,
            state_store,
            quiet: AtomicBool::new(false),
            event_tx,
            max_route_decisions: 100,
            run_lock: Mutex::new(())
        });
        channel_loop.runtime.attach(&channel_loop);
        Ok(channel_loop)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    /// The whole history, whoever may see it; outside the channel, what a
    /// participant sees is `visible_history`.
    pub(crate) async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
    }


    pub fn id(&self) -> ChannelId { self.id }

    pub fn session_id(&self) -> SessionId { self.session_id }
    
    pub fn name(&self) -> String {
        self.channel.name()
    }

    /// The kind of moderator the channel was built with, e.g. `round_robin`.
    pub fn moderator(&self) -> String {
        self.channel.load().moderator.clone()
    }

    /// Who the channel belongs to and who contributes to it (issue 11).
    pub fn security_attributes(&self) -> SecurityAttributes {
        self.channel.load().security_attributes.clone()
    }

    /// The channel as it is now.
    pub fn channel(&self) -> Channel {
        (**self.channel.load()).clone()
    }

    /// Renames the channel on behalf of `context`: stored first, which is
    /// where the permission is checked (`UpdateChannel`, issue 70).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// channel, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: String) -> Result<()> {
        Ok(self.channel.update_name(context, new_name).await?)
    }

    /// The part of the history `participant` may see (issue 46): all of it,
    /// unless they joined later with a limit - with what a reader records as
    /// read (issue 72). The one place the limits are applied: for users over
    /// REST, for agents reading the channel and for the transcript of an
    /// agent that joins.
    pub async fn visible_history(&self, participant: ActorId) -> VisibleHistory {
        let (history, digest) = {
            let state = self.state.lock().await;
            (state.history.messages(), state.history.digest())
        };
        let start = self.history_start_of(participant);
        VisibleHistory { messages: visible_part(start, history), digest, start }
    }

    /// Where `participant`'s view of the history begins; `None`: all of it.
    fn history_start_of(&self, participant: ActorId) -> Option<HistoryStart> {
        self.channel.load().history_limits.iter().find(|l| l.participant == participant).map(|l| l.start)
    }

    /// Adds `participant` - a participant of the session - to the channel on
    /// behalf of `context` (issue 46), seeing as much of the history as
    /// `history` grants. Stored first (`UpdateChannel`, also asked as
    /// `JoinChannel`), then its actor started, the moderator and the mention
    /// directory told, and the join announced. An agent gets the visible
    /// history into its transcript; an agent that joins again gets only what
    /// was said after the last message it has (see `seed_transcript`).
    ///
    /// # Errors
    /// `ChannelError::NotInSession`, `AlreadyParticipant` or `Forbidden`; a
    /// `StoreError` if `context` may not change the channel.
    pub async fn add_participant(&self, context: &SecurityContext, participant: ActorId, history: HistoryAccess) -> Result<()> {
        if self.actors.load().contains_key(&participant) {
            return Err(ChannelError::AlreadyParticipant { participant, channel: self.id }.into());
        }
        let found = self.runtime.session()
            .and_then(|s| s.get_session().participants.iter().find(|p| p.actor_id() == participant).cloned())
            .ok_or(ChannelError::NotInSession { participant, channel: self.id })?;
        let classification = self.classification();
        self.check(context, ActionType::JoinChannel { participant, channel: &classification }, "adding a participant to").await?;

        let messages = self.history().await;
        let start = history_start(history, &messages, chrono::Utc::now());
        self.channel.add_participant(context, found.clone(), start).await?;

        let started = async {
            if let ActorId::Agent(agent) = participant {
                self.seed_transcript(agent, visible_part(self.history_start_of(participant), messages)).await?;
            }
            Self::create_actor(self.channel.clone(), &found, &self.runtime).await
        }.await;
        let actor: Arc<dyn Actor> = match started {
            Ok(actor) => Arc::from(actor),
            Err(error) => {
                // Stored, but it cannot run: taken back out, in the runtime's
                // name - adding it was permitted.
                if let Err(undo) = self.channel.remove_participant(&SecurityContext::system(), participant).await {
                    warn!("{} was added to channel {} but could not be started nor removed again: {}", participant, self.id, undo);
                }
                return Err(error);
            }
        };
        self.moderator.lock().await.add_actor(actor.clone());
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.insert(participant, actor.clone());
            actors
        });
        self.rebuild_mentions();
        let _ = self.changes_tx.send(ActorChange::Start(actor));
        info!("{} joined channel {}", participant, self.id);
        Ok(())
    }

    /// Removes `participant` from the channel on behalf of `context` (issue
    /// 46): stored first (`UpdateChannel`), then its actor stopped - an agent
    /// at once, without finishing a turn; the approver discards its pending
    /// approvals. The leave is announced by the `ChannelController` on the
    /// event bus, which is how anyone following the channel learns of it
    /// (issue 73). Its state is kept, for the record and for when it joins
    /// again.
    ///
    /// # Errors
    /// `ChannelError::ApproverStays` or `NotAParticipant`; a `StoreError` if
    /// `context` may not change the channel.
    pub async fn remove_participant(&self, context: &SecurityContext, participant: ActorId) -> Result<()> {
        if matches!(participant, ActorId::Approver(_)) {
            return Err(ChannelError::ApproverStays { channel: self.id }.into());
        }
        let Some(actor) = self.actor_by_id(participant) else {
            return Err(ChannelError::NotAParticipant { participant, channel: self.id }.into());
        };
        self.channel.remove_participant(context, participant).await?;
        self.actors.rcu(|actors| {
            let mut actors = HashMap::clone(actors);
            actors.remove(&participant);
            actors
        });
        self.moderator.lock().await.remove_actor(actor.clone());
        self.rebuild_mentions();
        actor.stop();
        let _ = self.changes_tx.send(ActorChange::Stop(participant));
        info!("{} left channel {}", participant, self.id);
        Ok(())
    }

    /// Appends to `agent`'s transcript in this channel the messages of
    /// `visible` after the last one it has - what it missed while it was
    /// away. The transcript is only ever appended to, so older messages that
    /// a wider history grants on rejoining would land after the agent's own
    /// later answers; they are left to `read_history` instead, and the
    /// transcript stays in the order things were said.
    async fn seed_transcript(&self, agent: caelumora_model::agent::AgentId, visible: Vec<Message>) -> Result<()> {
        let mut state = self.state_store.get_agent_channel_state(agent, self.id).await?;
        let known: std::collections::HashSet<_> = state.transcript.messages().iter().map(|m| m.id).collect();
        let after = visible.iter().rposition(|m| known.contains(&m.id)).map_or(0, |last| last + 1);
        let missing: Vec<Message> = visible.into_iter().skip(after).filter(|m| !known.contains(&m.id)).collect();
        if !missing.is_empty() {
            state.transcript.extend(missing);
            self.state_store.put_agent_channel_state(agent, self.id, &state).await?;
        }
        Ok(())
    }

    fn rebuild_mentions(&self) {
        let mentions = Self::mention_directory(&self.actors.load());
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", self.id, handle);
        }
        self.mentions.store(Arc::new(mentions));
    }

    /// Ends the current turn of every actor in the channel, on behalf of
    /// `context`'s initiator, and keeps the channel quiet - nobody is asked
    /// to respond - until it is woken again, usually by the next message.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `CancelChannel`; the channel is then untouched. Deliberately one
    /// decision for the whole channel: a per-actor one could leave half of it
    /// cancelled (issue 30).
    pub async fn cancel(&self, context: &SecurityContext) -> Result<()> {
        let channel = self.channel.load();
        self.check(context, ActionType::CancelChannel(&channel.security_attributes), "cancelling").await?;
        self.quiet.store(true, Ordering::SeqCst);
        for actor in self.actors.load().values() {
            actor.cancel();
        }
        Ok(())
    }

    /// Wakes the channel's sleeping actors because someone asked for it -
    /// a `/wakeup` and nothing else. Returns the names of the actors that
    /// were asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`.
    pub async fn wakeup(&self, context: &SecurityContext) -> Result<Vec<String>> {
        let channel = self.channel.load();
        self.check(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let asleep = self.actors.load().values()
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name())
            .collect();
        self.wake_for_delivery();
        Ok(asleep)
    }

    /// Wakes one actor deliberately, like `wakeup` and under the same
    /// permission. Returns whether it was asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`; an error if `actor` is not in this channel.
    pub async fn wakeup_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<bool> {
        let channel = self.channel.load();
        self.check(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let actor = self.actors.load().get(&actor).cloned()
            .ok_or_else(|| anyhow!("actor {} is not in channel {}", actor, self.id))?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.quiet.store(false, Ordering::SeqCst);
        actor.wakeup();
        Ok(was_asleep)
    }

    /// Wakes the channel's actors because something is on its way to them:
    /// a message, a mention, a `WakeupRequested`. That is the consequence of
    /// an action already permitted, so it carries no permission of its own -
    /// otherwise a user allowed to write but not to wake could not write.
    pub fn wake_for_delivery(&self) {
        self.quiet.store(false, Ordering::SeqCst);
        for actor in self.actors.load().values() {
            actor.wakeup();
        }
    }

    /// Asks the `SecurityManager` about `action`, turning a refusal into a
    /// `ChannelError::Forbidden` that names what was refused, not why.
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>, what: &'static str) -> Result<()> {
        if self.runtime.security_manager().check(context, action).await {
            return Ok(());
        }
        info!("Refused {} channel {} for {}", what, self.id, context.initiator_id);
        Err(ChannelError::Forbidden { action: what, channel: self.id }.into())
    }

    pub fn actor_names(&self) -> Vec<String> {
        self.actors.load().values().map(|a| a.name()).collect()
    }
    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.load().values().map(|a| a.id()).collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.load().values()
            .map(|a| {
                let name = a.name();
                let handle = Some(handle_of(&name))
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
    }

    /// The channel as the confidentiality rules see it (issue 61).
    pub fn classification(&self) -> ChannelClassification {
        self.channel.load().classification()
    }

    /// The notes every agent of the channel published (issue 17), read from
    /// their states in the `StateStore`: notes of finished turns only.
    pub async fn published_notes(&self) -> Result<Vec<crate::session_reader::PublishedNotes>> {
        let mut published = Vec::new();
        for actor in self.actors.load().values() {
            let ActorId::Agent(agent_id) = actor.id() else { continue };
            let state = self.state_store.get_agent_channel_state(agent_id, self.id).await?;
            published.push(crate::session_reader::PublishedNotes {
                agent: actor.id(),
                name: actor.name(),
                notes: state.notes.current(),
                digest: state.notes.digest(),
            });
        }
        Ok(published)
    }

    /// The private notes `agent` wrote in this channel (issue 51), read from
    /// its state; `None` if it takes no part in the channel. Only for the
    /// agent itself - see `notes::SessionReader::private_notes`.
    pub async fn private_notes(&self, agent: caelumora_model::agent::AgentId) -> Result<Option<(Vec<caelumora_model::note::Note>, Option<caelumora_model::digest::Digest>)>> {
        if !self.actors.load().contains_key(&ActorId::Agent(agent)) {
            return Ok(None);
        }
        let state = self.state_store.get_agent_channel_state(agent, self.id).await?;
        Ok(Some((state.private_notes.current(), state.private_notes.digest())))
    }

    fn is_mentionable(id: ActorId) -> bool {
        matches!(id, ActorId::User(_) | ActorId::Agent(_))
    }

    fn mention_directory(actors: &HashMap<ActorId, Arc<dyn Actor>>) -> MentionDirectory {
        let names: Vec<(ActorId, String)> = actors.values()
            .filter(|a| Self::is_mentionable(a.id()))
            .map(|a| (a.id(), a.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    pub fn actor_by_id(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        self.actors.load().get(&actor_id).cloned()
    }
    pub fn actor_by_name(&self, name: &str) -> Option<Arc<dyn Actor>> {
        self.actors.load().values().find(|c| c.name() == name).map(|c| c.clone())
    }

    /// The actor `user` acts through in this channel, or `None` if they are
    /// not a participant of it.
    ///
    /// Every envelope is built by the actor itself, so this is what decides
    /// whose name a message or an approval carries: there is deliberately no
    /// way to reach "some" user actor (issue 30).
    pub fn user_actor(&self, user: UserId) -> Option<Arc<UserActor>> {
        let actor = self.actor_by_id(ActorId::User(user))?;
        let any_actor: Arc<dyn Any + Send + Sync> = actor;
        Arc::downcast::<UserActor>(any_actor).ok()
    }

    async fn create_actor(channel: Arc<ChannelController>, participant: &Participant, runtime: &ChannelRuntime) -> Result<Box<dyn Actor>> {
        let (session_id, channel_id) = { let c = channel.load(); (c.session_id, c.id) };
        let actor_runtime = runtime.new_actor_runtime(participant.actor_id(), session_id, channel_id);
        match participant {
            Participant::User(user) => {
                let actor = UserActor::new(&channel.load(), user.clone(), actor_runtime).await?;
                Ok(Box::new(actor))
            },
            Participant::Agent(agent) => {
                let actor = AgentActor::new(&channel.load(), agent.clone(), actor_runtime).await?;
                Ok(Box::new(actor))
            },
            Participant::Approver(approver) => {
                Ok(Box::new(ApproverActor::new(channel, approver.clone(), actor_runtime)))
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let mut changes_rx = self.changes_rx.lock().await;
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let mut actors = self.spawn_actors(event_tx.clone(), &mut join_set)?;

        loop {
            tokio::select! {
                maybe_event = event_rx.recv() => {
                    if self.handle_actor_event(maybe_event, &actors).await {
                        break;
                    }
                }
                Some(change) = changes_rx.recv() => {
                    self.apply_change(change, &mut actors, &event_tx, &mut join_set);
                }
                // Finished tasks are dropped as they end; once the last one
                // is gone, the channel stops - the sender kept for actors
                // joining later keeps the event queue from closing by itself.
                Some(_) = join_set.join_next(), if !join_set.is_empty() => {
                    if join_set.is_empty() {
                        break;
                    }
                }
            }
        }

        info!("Channel {}/{} stopped", self.channel.name(), self.id);
        Ok(())
    }

    /// Starts or stops an actor as a participant joined or left (issue 46).
    fn apply_change(&self, change: ActorChange, actors: &mut HashMap<ActorId, ActorHandle>, event_tx: &Sender<Envelope>, join_set: &mut JoinSet<ActorId>) {
        match change {
            ActorChange::Start(actor) => {
                // Joined before `run` started: already running.
                if !actors.contains_key(&actor.id()) {
                    let handle = self.spawn_actor(actor, event_tx.clone(), join_set);
                    actors.insert(handle.id, handle);
                }
            }
            ActorChange::Stop(id) => {
                // Dropping the handle closes the actor's inbox; told to stop
                // (`Actor::stop`), it ends its task itself once its state is
                // stored consistent. Only if it does not - a tool that hangs -
                // is it aborted.
                if let Some(handle) = actors.remove(&id) {
                    let abort_handle = handle.abort_handle.clone();
                    let (channel, actor) = (self.id, id);
                    tokio::spawn(async move {
                        tokio::time::sleep(STOP_GRACE).await;
                        if !abort_handle.is_finished() {
                            warn!("Actor {} did not stop by itself after leaving channel {}: aborted", actor, channel);
                            abort_handle.abort();
                        }
                    });
                    info!("Actor {} is stopping: it left channel {}", id, self.id);
                }
            }
        }
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, actors: &HashMap<ActorId,ActorHandle>) -> bool {
        if let Some(event) = maybe_event {
            let span = crate::logging::envelope_span(&event.context, event.event.kind());
            self.process_envelope(event, actors).instrument(span).await;
            false
        } else {
            true
        }
    }

    async fn process_envelope(&self, event: Envelope, actors: &HashMap<ActorId,ActorHandle>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        let event = self.resolve_mentions(event);
        self.record_receive_event(&event).await;
        self.append_history(&event).await;
        self.route_event(actors, event).await;
    }

    fn spawn_actors(&self, actor_output:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> Result<HashMap<ActorId,ActorHandle>> {
        let mut actors = HashMap::new();
        for (id, actor) in self.actors.load().iter() {
            let handle = self.spawn_actor(actor.clone(), actor_output.clone(), join_set);
            actors.insert(*id, handle);
        };

        Ok(actors)
    }

    fn spawn_actor(&self, actor: Arc<dyn Actor>, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let filter = actor.event_filter();
        let (tx, rx) = mpsc::channel(100);

        let actor_rx = Box::new(InMemoryEventReceiver::new(rx));
        let actor_tx = Box::new(InMemoryEventSender::new(actor_output));

        let abort_handle = join_set.spawn(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                tracing::error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        });

        // Wrap the actor's input sender with its declared filter predicate.
        let filtered_sender = Box::new(FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            filter,
        ));

        ActorHandle { id, abort_handle, sender: filtered_sender }
    }

    /// Resolves the `@`-mentions in every complete message carried by `event`
    /// and stores them in `Message::mentions`. Done here, once, as the message
    /// enters the channel, so that every frontend, the history and the
    /// moderator all see the same resolution (issue 29).
    fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let resolve = |m: &mut Message| {
            if let Some(content) = &m.content {
                m.mentions = self.mentions.load().resolve(content);
            }
        };
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) => resolve(m),
            ActorEvent::FullResponseGenerated(msgs) => msgs.iter_mut().for_each(resolve),
            _ => {}
        }
        event
    }

    /// Wakes every sleeping agent a user explicitly mentioned, so that the
    /// moderator's `ResponseRequested` reaches an agent that acts on it.
    /// Mentions in agent responses wake nobody, matching the moderator, which
    /// does not route them either.
    async fn wake_mentioned(&self, event: &Envelope) {
        if !matches!(event.context.sender_id, ActorId::User(_)) {
            return;
        }
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            for id in messages.iter().flat_map(|m| m.mentions.iter()) {
                if let Some(actor) = self.actors.load().get(id).cloned() {
                    if matches!(id, ActorId::Agent(_)) && actor.state() == ActorState::Sleeping {
                        actor.wakeup();
                    }
                }
            }
        }
    }

    async fn route_event(&self, actors: &HashMap<ActorId,ActorHandle>, event: Envelope) {
        if matches!(event.event, ActorEvent::WakeupRequested) {
            self.wake_for_delivery();
        }
        self.wake_mentioned(&event).await;

        //if let ActorEvent::FullResponseGenerated(messages) = &event.event {
        //    if messages.iter().any(|m| m.role == MessageRole::User) {
        //        let _ = self.wakeup().await;
        //    }
        //}

        let broadcast = self.route_event_impl(actors, event.clone()).await;

        if broadcast {
            let _ = self.event_tx.send(event);
        }
    }

    async fn route_event_impl(&self, actors: &HashMap<ActorId,ActorHandle>, event: Envelope) -> bool {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return true
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let mut moderator = self.moderator.lock().await;
        let decision = moderator.route(&event).await;
        drop(moderator);
        let quiet = self.quiet.load(Ordering::SeqCst);

        // First, route all messages to all agents...
        for route in decision.forwards {
            let target_id = route.0;

            // Do not propagate message to sender again
            if target_id == event.context.sender_id {
                continue;
            }
            // A cancelled channel stays quiet until it is woken: what was said
            // still reaches everyone, so that every transcript keeps up with
            // the history, but nobody is asked to answer it.
            if quiet && matches!(route.1.event, ActorEvent::ResponseRequested) {
                debug!(to = %target_id, "Not asking for a response: the channel is quiet after a cancel");
                continue;
            }

            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let event_desc = target_event.describe();
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = span_id;
            target_event.context.span_depth = span_depth;
            if let Some(actor) = actors.get(&target_id) {
                debug!(to = %target_id, "Forwarding {}", event_desc);
                self.record_send_event(&target_event).await;
                if let Err(e) = actor.sender.try_send(target_event) {
                    match e {
                        TransportError::Full => warn!("Queue full for actor {}", target_id),
                        TransportError::Filtered => {}, // Normal: FilteringEventSender drops unmatched events
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }

        decision.broadcast
    }

    async fn append_history(&self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        let mut changed = false;
        match &envelope.event {
            ActorEvent::FullResponseGenerated(msgs) => {
                state.history.extend(msgs.clone());
                changed = true;
            }
            // Handle other event types as needed
            _ => {}
        }

        if changed {
            if let Err(e) = self.state_store.put_channel_state(self.id, &state).await {
                warn!("Cannot store state of channel {}: {}", self.id, e);
            }
        }
    }
    async fn record_send_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_send(&event).await;
        drop(recorder);
    }
    async fn record_receive_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_receive(&event).await;
        drop(recorder);
    }
}

/// The part of a channel's history a participant sees (issue 46), as
/// `ChannelLoop::visible_history` gives it.
#[derive(Debug, Clone)]
pub struct VisibleHistory {
    /// The messages they see, from `start` on.
    pub messages: Vec<Message>,
    /// The digest of the channel's history at the time: up to where it was
    /// read. It names the whole chain, so it is recorded with `start`.
    pub digest: Option<caelumora_model::digest::Digest>,
    /// Where their view begins - from where it was read; `None`: all of it.
    pub start: Option<HistoryStart>,
}

/// `history` as seen from `start`: all of it if there is none.
fn visible_part(start: Option<HistoryStart>, history: Vec<Message>) -> Vec<Message> {
    match start {
        Some(start) => visible_from(start, history),
        None => history,
    }
}

/// Where the view of `history` begins for a participant granted `access`,
/// fixed now - so that "the last three days" does not move day by day.
/// `None` if all of it is visible.
fn history_start(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Option<HistoryStart> {
    let last = || history.last().map(|m| HistoryStart::After(m.id));
    let from = |index: usize| match index {
        0 => None,
        i if i >= history.len() => last(),
        i => Some(HistoryStart::From(history[i].id)),
    };
    match access {
        HistoryAccess::All => None,
        HistoryAccess::None => last(),
        HistoryAccess::LastMessages { count } => from(history.len().saturating_sub(count as usize)),
        HistoryAccess::LastDays { count } => {
            let since = now - chrono::Duration::days(i64::from(count));
            from(history.iter().position(|m| m.timestamp >= since).unwrap_or(history.len()))
        }
    }
}

/// The part of `history` visible from `start` - nothing if the message it
/// begins at is not there, rather than more than was granted.
fn visible_from(start: HistoryStart, history: Vec<Message>) -> Vec<Message> {
    let (id, skip) = match start {
        HistoryStart::From(id) => (id, 0),
        HistoryStart::After(id) => (id, 1),
    };
    match history.iter().position(|m| m.id == id) {
        Some(index) => history.into_iter().skip(index + skip).collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_impl::ollama::OllamaClient;
    use crate::command::CommandExecutor;
    use crate::component::ComponentImpl;
    use crate::component_impl::help_command::HelpCommandProvider;
    use crate::component_impl::wakeup_command::WakeupCommandProvider;
    use caelumora_model::component::ComponentFailPolicy;
    use crate::runtime::RuntimeManager;
    use caelumora_model::command::default_commands;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes, SecurityManager};
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::User;

    /// Refuses everything, to see that a refusal reaches the caller.
    #[derive(Debug)]
    struct DenyAll;

    #[async_trait::async_trait]
    impl SecurityManager for DenyAll {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    /// Decides by a rule over the action alone, to refuse one thing and
    /// allow the rest.
    #[derive(Debug)]
    struct Policy(fn(ActionType<'_>) -> bool);

    #[async_trait::async_trait]
    impl SecurityManager for Policy {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            (self.0)(action)
        }
    }

    fn user(username: &str) -> User {
        User {
            id: UserId::new(),
            username: username.to_string(),
            email: String::new(),
            roles: Vec::new(),
            security: SecurityContext::default(),
        }
    }

    async fn channel_with(participants: Vec<Participant>, security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, security, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            participants,
            SecurityAttributes::default(),
        );
        let builtin = |id: &str, facet: Arc<dyn crate::command::CommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
        let executor = CommandExecutor::new(
            runtime.security_manager(),
            vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
        ).unwrap();
        ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap()
    }

    #[tokio::test]
    async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
        let alice = user("alice");
        let bob = user("bob");
        let channel = channel_with(
            vec![Participant::User(alice.clone()), Participant::User(bob.clone())],
            Arc::new(DummySecurityManager::new()),
        ).await;

        for participant in [&alice, &bob] {
            let actor = channel.user_actor(participant.id).expect("a participant has an actor");
            assert_eq!(actor.id(), ActorId::User(participant.id));
            assert_eq!(actor.name(), participant.username, "the actor writes in this user's name");
        }
        assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
    }

    #[tokio::test]
    async fn cancelling_and_waking_deliberately_ask_the_security_manager() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let participants = vec![Participant::User(alice)];

        let refused = channel_with(participants.clone(), Arc::new(DenyAll)).await;
        for error in [
            refused.cancel(&context).await.expect_err("cancelling is refused"),
            refused.wakeup(&context).await.expect_err("waking is refused"),
        ] {
            assert!(crate::error::is_forbidden(&error), "the caller can tell a refusal from a failure: {error}");
            assert!(!error.to_string().contains("DenyAll"), "a refusal does not disclose the rule: {error}");
        }
        // Waking because something is on its way is not a wakeup of its own:
        // it follows from an action that was permitted.
        refused.wake_for_delivery();

        let allowed = channel_with(participants, Arc::new(DummySecurityManager::new())).await;
        allowed.cancel(&context).await.expect("permitted");
        allowed.wakeup(&context).await.expect("permitted");
    }

    /// Five messages, one a day, the last one `now`.
    fn daily(now: chrono::DateTime<chrono::Utc>) -> Vec<Message> {
        (0..5).rev()
            .map(|days| Message { id: caelumora_model::message::MessageId::new(), timestamp: now - chrono::Duration::days(days), ..Message::default() })
            .collect()
    }

    fn ids(history: &[Message]) -> Vec<caelumora_model::message::MessageId> {
        history.iter().map(|m| m.id).collect()
    }

    fn granted(access: HistoryAccess, history: &[Message], now: chrono::DateTime<chrono::Utc>) -> Vec<caelumora_model::message::MessageId> {
        let messages = match history_start(access, history, now) {
            Some(start) => visible_from(start, history.to_vec()),
            None => history.to_vec(),
        };
        ids(&messages)
    }

    #[test]
    fn history_access_all_and_none_grant_everything_and_nothing() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::All, &history, now), ids(&history), "All sees every message");
        assert!(granted(HistoryAccess::None, &history, now).is_empty(), "None sees no earlier message");
        assert_eq!(history_start(HistoryAccess::None, &[], now), None, "an empty channel has nothing to hide");
    }

    #[test]
    fn history_access_last_messages_and_days_cut_the_history() {
        let now = chrono::Utc::now();
        let history = daily(now);
        assert_eq!(granted(HistoryAccess::LastMessages { count: 2 }, &history, now), ids(&history[3..]));
        assert_eq!(granted(HistoryAccess::LastMessages { count: 9 }, &history, now), ids(&history),
                   "more than there is grants all of it");
        assert!(granted(HistoryAccess::LastMessages { count: 0 }, &history, now).is_empty());
        // Messages 1, 2 and 3 days ago plus today's - within the last three days.
        assert_eq!(granted(HistoryAccess::LastDays { count: 3 }, &history, now), ids(&history[1..]));
        assert!(granted(HistoryAccess::LastDays { count: 3 }, &history, now + chrono::Duration::days(10)).is_empty(),
                "nothing within the last days sees nothing");
    }

    #[test]
    fn a_history_start_shows_later_messages_and_nothing_if_its_message_is_gone() {
        let now = chrono::Utc::now();
        let mut later = daily(now);
        let start = HistoryStart::After(later[1].id);
        later.push(Message { id: caelumora_model::message::MessageId::new(), ..Message::default() });
        assert_eq!(ids(&visible_from(start, later.clone())), ids(&later[2..]), "messages sent after joining stay visible");
        assert!(visible_from(HistoryStart::From(caelumora_model::message::MessageId::new()), later).is_empty(),
                "an unknown start shows nothing rather than everything");
    }

    #[tokio::test]
    async fn a_rejoining_agent_gets_what_it_missed_but_nothing_older() {
        use caelumora_model::message::MessageId;
        let alice = user("alice");
        let channel = channel_with(vec![Participant::User(alice)], Arc::new(DummySecurityManager::new())).await;
        let agent = caelumora_model::agent::AgentId::new();
        let message = |text: &str| Message { id: MessageId::new(), content: Some(text.to_string()), ..Message::default() };
        let (one, two, three, four) = (message("one"), message("two"), message("three"), message("four"));
        let transcript = |channel: Arc<ChannelLoop>| async move {
            channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap()
                .transcript.messages().into_iter().map(|m| m.content.unwrap()).collect::<Vec<_>>()
        };

        // Joined seeing the last message only.
        channel.seed_transcript(agent, vec![three.clone()]).await.unwrap();
        assert_eq!(transcript(channel.clone()).await, ["three"]);

        // Rejoined with all of it: "four" was missed, "one" and "two" are
        // older than what it has and would come after it.
        channel.seed_transcript(agent, vec![one, two, three, four]).await.unwrap();
        assert_eq!(transcript(channel.clone()).await, ["three", "four"], "in the order things were said");
    }

    fn command_error(error: &anyhow::Error) -> &crate::command::CommandError {
        error.downcast_ref().unwrap_or_else(|| panic!("expected a CommandError, got {error:#}"))
    }

    #[tokio::test]
    async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
        let alice = user("alice");
        // `/wakeup` is hidden from everybody.
        let policy = Policy(|action| !matches!(action, ActionType::ShowCommand("wakeup")));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(policy)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let reply = actor.execute_command("help", "").await.expect("/help runs");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Private, "/help answers the issuer alone");
        let text = reply.output;
        assert!(text.contains("/help"), "{text}");
        assert!(!text.contains("/wakeup"), "a hidden command is not listed: {text}");
        let names: Vec<String> = actor.visible_commands().await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["help"]);

        let error = actor.execute_command("wakeup", "").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "a hidden command looks unknown: {error}");
        let error = actor.execute_command("help", "wakeup").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "/help does not describe it either: {error}");

        let quiet = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await;
        assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_is_announced_to_the_channel_in_the_issuers_name() {
        let alice = user("alice");
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Channel, "the answer goes to the channel");
        assert_eq!(reply.output, "No agent was asleep.", "and back to the issuer, whom the channel does not route it to");

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("the announcement arrives")
            .unwrap();
        assert_eq!(envelope.context.sender_id, ActorId::User(alice.id));
        match envelope.event {
            ActorEvent::CommandExecuted { command, output } => {
                assert_eq!(command, "wakeup");
                assert_eq!(output, "No agent was asleep.");
            }
            other => panic!("expected CommandExecuted, got {:?}", other.kind()),
        }
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_reports_refusals_and_unknown_agents() {
        let alice = user("alice");
        let refuse_waking = Policy(|action| !matches!(action, ActionType::WakeChannel(_)));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(refuse_waking)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("waking is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "{error}");
        assert!(crate::error::is_forbidden(&error), "{error}");

        let refuse_running = Policy(|action| !matches!(action, ActionType::ExecuteCommand("wakeup")));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(refuse_running)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("running it is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "visible but not runnable: {error}");

        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "@nobody").await.expect_err("no such agent");
        assert!(matches!(command_error(&error), crate::command::CommandError::InvalidArguments { .. }), "{error}");
        let error = actor.execute_command("", "").await.expect_err("empty");
        assert!(matches!(command_error(&error), crate::command::CommandError::Empty), "{error}");
        let error = actor.execute_command("nonsense", "").await.expect_err("unknown");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "{error}");
    }

    #[tokio::test]
    async fn only_the_owner_of_an_agent_may_read_what_it_saw() {
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::meta::{Metadata, Scope};
        use caelumora_model::security::OwnershipSecurityManager;
        use caelumora_model::store::EntityStore;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let personality = Personality {
            reasoning_confidentiality: caelumora_model::confidentiality::Confidentiality::Public,
            id: PersonalityId::new(),
            metadata: Metadata { name: "plain".to_string(), ..Metadata::default() },
            components: Default::default(),
            security_attributes: Default::default(),
        };
        store.put_personality(&SecurityContext::system(), &personality).await.unwrap();

        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |u: &User| SecurityContext::new(ActorId::User(u.id), Default::default(), Default::default());
        let agent = Agent::new("helper".to_string(), Scope::GlobalScope, &personality, as_user(&alice));
        let agent_id = ActorId::Agent(agent.id);

        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands(), Arc::new(OllamaClient::default())).unwrap());
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            vec![Participant::User(alice.clone()), Participant::User(bob.clone()), Participant::Agent(agent)],
            SecurityAttributes::default(),
        );
        let executor = CommandExecutor::empty(runtime.security_manager());
        let channel = ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap();

        let helper = channel.actor_by_id(agent_id).expect("the agent is a participant");
        assert!(helper.may_read_context(&as_user(&alice)).await, "alice created the agent");
        assert!(!helper.may_read_context(&as_user(&bob)).await, "bob only takes part in the channel");
        let bobs_actor = channel.actor_by_id(ActorId::User(bob.id)).unwrap();
        assert!(!bobs_actor.may_read_context(&as_user(&alice)).await, "a user has nothing to read");
    }

    /// Answers every event with the same decision: tell `to` what was said,
    /// and ask it to respond.
    #[derive(Debug)]
    struct AskBack(ActorId);

    #[async_trait::async_trait]
    impl crate::moderator::Moderator for AskBack {
        async fn route(&mut self, event: &Envelope) -> crate::moderator::RouteDecision {
            vec![(self.0, event.clone()), (self.0, event.forward_with_event(ActorEvent::ResponseRequested))].into()
        }
        fn add_actor(&mut self, _: Arc<dyn Actor>) {}
        fn remove_actor(&mut self, _: Arc<dyn Actor>) {}
    }

    #[tokio::test]
    async fn a_cancelled_channel_delivers_what_is_said_but_asks_nobody_until_it_is_woken() {
        use caelumora_model::actor::EventContext;
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(DummySecurityManager::new())).await;
        let bob = ActorId::Agent(caelumora_model::agent::AgentId::new());
        *channel.moderator.lock().await = Box::new(AskBack(bob));
        let (tx, mut rx) = mpsc::channel(8);
        let abort_handle = tokio::spawn(async {}).abort_handle();
        let actors = HashMap::from([(bob, ActorHandle { id: bob, abort_handle, sender: Box::new(InMemoryEventSender::new(tx)) })]);
        let said = Envelope::new(
            context.clone(),
            EventContext { sender_id: ActorId::User(alice.id), ..EventContext::default() },
            ActorEvent::FullResponseGenerated(vec![Message::default()]),
        );
        let delivered = |rx: &mut mpsc::Receiver<Envelope>| {
            let mut kinds = Vec::new();
            while let Ok(envelope) = rx.try_recv() { kinds.push(envelope.event.kind()); }
            kinds
        };

        channel.cancel(&context).await.expect("permitted");
        channel.route_event(&actors, said.clone()).await;
        assert_eq!(delivered(&mut rx), ["FullResponseGenerated"], "what was said still arrives, but nobody is asked");

        channel.wake_for_delivery();
        channel.route_event(&actors, said).await;
        assert_eq!(delivered(&mut rx), ["FullResponseGenerated", "ResponseRequested"], "woken, the channel asks again");
    }

    /// A model whose answer never comes: what a cancel has to interrupt.
    #[derive(Debug)]
    struct NeverAnswers;

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for NeverAnswers {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            Box::pin(futures::stream::pending())
        }
    }

    /// A running channel with alice and an agent whose model never answers,
    /// the agent already in a turn: what a cancel or a stop has to end.
    struct Stuck {
        channel: Arc<ChannelLoop>,
        context: SecurityContext,
        agent: caelumora_model::agent::AgentId,
        helper: Arc<dyn Actor>,
        events: broadcast::Receiver<Envelope>,
        running: tokio::task::JoinHandle<Result<()>>,
    }

    async fn stuck_in_a_model_call() -> Stuck {
        let stuck = with_an_agent(Arc::new(NeverAnswers), Default::default()).await;
        settled(&stuck.helper, ActorState::Planning).await;
        stuck
    }

    /// A running channel with alice and an agent that talks to `model` and
    /// has `components`, after alice said "hello".
    async fn with_an_agent(model: Arc<dyn crate::llm::LLMClient>, components: caelumora_model::component::ComponentConfig) -> Stuck {
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::meta::{Metadata, Scope};
        use caelumora_model::store::EntityStore;

        let security: Arc<dyn SecurityManager> = Arc::new(DummySecurityManager::new());
        let store = Arc::new(MemoryStore::new());
        let personality = Personality {
            reasoning_confidentiality: caelumora_model::confidentiality::Confidentiality::Public,
            id: PersonalityId::new(),
            metadata: Metadata { name: "plain".to_string(), ..Metadata::default() },
            components,
            security_attributes: Default::default(),
        };
        store.put_personality(&SecurityContext::system(), &personality).await.unwrap();
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let agent = Agent::new("helper".to_string(), Scope::GlobalScope, &personality, context.clone());
        let agent_id = agent.id;

        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, security, default_commands(), model).unwrap());
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            vec![Participant::User(alice.clone()), Participant::Agent(agent)],
            SecurityAttributes::default(),
        );
        // Stored, so that leaving it can be stored too.
        runtime.entity_store().create_channel(&SecurityContext::system(), &channel).await.unwrap();
        let executor = CommandExecutor::empty(runtime.security_manager());
        let channel = ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap();
        let events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        // The message makes the agent start a turn.
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string()).await.unwrap();
        let helper = channel.actor_by_id(ActorId::Agent(agent_id)).unwrap();
        Stuck { channel, context, agent: agent_id, helper, events, running }
    }

    /// A model that gives the answers it was given, one per call, and then
    /// none.
    #[derive(Debug)]
    struct Scripted(std::sync::Mutex<std::collections::VecDeque<Message>>);

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Scripted {
        async fn chat_completion(&self, _: &[Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            match self.0.lock().unwrap().pop_front() {
                Some(answer) => Box::pin(futures::stream::iter([Ok(caelumora_model::message::MessageEvent::Complete(answer))])),
                None => Box::pin(futures::stream::pending()),
            }
        }
    }

    #[tokio::test]
    async fn a_call_with_invalid_arguments_is_answered_without_asking_anyone() {
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::message::MessageRole;
        use caelumora_model::tool::{ToolCall, ToolConfidentiality, ToolResultStatus};

        // `create_plan` needs approval; this call's arguments are no plan.
        let not_a_plan = Message {
            role: MessageRole::Assistant,
            tool_calls: vec![ToolCall { id: Some("1".to_string()), function_name: "create_plan".to_string(), arguments: serde_json::json!({}), confidentiality: ToolConfidentiality::PUBLIC }],
            ..Message::default()
        };
        let answer = Message { role: MessageRole::Assistant, content: Some("Let me answer directly.".to_string()), ..Message::default() };
        let components = ComponentConfig { component_nodes: vec![ComponentDescriptor {
            id: "plan".to_string(),
            kind: crate::component_impl::create_plan_tool::CREATE_PLAN_TOOL_PROVIDER_KIND.to_string(),
            params: serde_json::Value::Null,
            deps: vec![],
            tags: vec![],
            fail_policy: ComponentFailPolicy::FailClosed,
            security_attributes: Default::default(),
        }] };
        let Stuck { agent, helper, mut events, running, .. } =
            with_an_agent(Arc::new(Scripted(std::sync::Mutex::new([not_a_plan, answer].into()))), components).await;

        // The turn ends without anyone approving anything: the call was
        // answered as invalid, and the model tried again.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == ActorId::Agent(agent) {
                    if let ActorEvent::FullResponseGenerated(messages) = envelope.event { break messages }
                }
            }
        }).await.expect("the turn ends without an approval");
        let results: Vec<_> = ended.iter().flat_map(|m| m.tool_results.iter()).collect();
        assert_eq!(results.len(), 1, "{ended:?}");
        assert_eq!(results[0].status, ToolResultStatus::InvalidArguments, "the model is asked to correct the call");
        assert_eq!(ended.last().and_then(|m| m.content.as_deref()), Some("Let me answer directly."));
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    /// Waits for `actor` to reach `wanted`.
    async fn settled(actor: &Arc<dyn Actor>, wanted: ActorState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while actor.state() != wanted { tokio::time::sleep(Duration::from_millis(10)).await }
        }).await.unwrap_or_else(|_| panic!("the agent does not reach {wanted:?}, it is {:?}", actor.state()));
    }

    #[tokio::test]
    async fn a_cancel_interrupts_a_model_call_that_is_in_flight() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;
        let agent_id = ActorId::Agent(agent);

        channel.cancel(&context).await.expect("permitted");

        // The turn ends at once - with an empty answer, so that observers
        // stop waiting - instead of when the model would have answered.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.expect("the channel is running");
                if envelope.context.sender_id == agent_id && matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) {
                    break envelope;
                }
            }
        }).await.expect("the cancelled turn ends without the model");
        assert!(matches!(ended.event, ActorEvent::FullResponseGenerated(ref messages) if messages.is_empty()));
        settled(&helper, ActorState::Idle).await;
        running.abort();
    }

    #[tokio::test]
    async fn a_removed_agent_abandons_its_turn_and_leaves_an_idle_state_behind() {
        let Stuck { channel, context, agent, helper, mut events, running } = stuck_in_a_model_call().await;

        channel.remove_participant(&context, ActorId::Agent(agent)).await.expect("permitted");

        // The turn ends without an answer - nothing is announced - and the
        // state in the store is the one a later actor of the agent starts
        // from: idle, with no turn to resume.
        settled(&helper, ActorState::Idle).await;
        let stored = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = channel.state_store.get_agent_channel_state(agent, channel.id).await.unwrap();
                if state.processing_state.load() == ActorState::Idle { break state }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("the idle state is stored");
        assert!(stored.turn_messages.is_empty() && stored.turn_context.is_empty() && stored.tool_calls.is_empty(), "the turn was abandoned");
        assert!(stored.transcript.messages().is_empty(), "nothing of it was recorded");
        while let Ok(envelope) = events.try_recv() {
            assert!(!matches!(envelope.event, ActorEvent::FullResponseGenerated(_)) || envelope.context.sender_id != ActorId::Agent(agent), "a stopped agent announces no answer");
        }
        // The channel is still running: alice is still in it.
        assert!(!running.is_finished());
        running.abort();
    }
}
