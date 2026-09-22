use crate::actor::Actor;
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::user::UserActor;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use crate::transport::{EventSender, TransportError};
use crate::transport_impl::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender};
use anyhow::{anyhow, Result};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::channel::{Channel, ChannelController, ChannelId, ChannelState, Participant};
use caelumora_model::event::GlobalEvent;
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::message::Message;
use caelumora_model::session::SessionId;
use caelumora_model::store::{StateStore, StoreError};
use tracing::{debug, info, trace, warn, Instrument};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
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

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    runtime_manager: Arc<RuntimeManager>,
    actors: HashMap<ActorId,Arc<dyn Actor>>,
    moderator: Mutex<Box<dyn Moderator>>,
    mentions: MentionDirectory,
    recorder: Mutex<Box<dyn Recorder>>,
    state: Arc<Mutex<ChannelState>>,
    state_store: Arc<dyn StateStore>,
    processing_state: Arc<AtomicU8>,
    event_tx: broadcast::Sender<Envelope>,
    max_route_decisions: i32,
    run_lock: Mutex<()>
}

impl ChannelLoop {
    pub async fn new(channel: Channel, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let id = channel.id;
        let session_id = channel.session_id;
        let state_store = runtime_manager.state_store();
        let mut moderator = runtime_manager.new_moderator(&channel.moderator)?;
        
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

        let recorder = runtime_manager.new_recorder()?;
        let (event_tx, _) = broadcast::channel(100);

        let channel_controller = runtime_manager.controller_manager().new_channel_controller(channel);

        let mut actors = HashMap::new();
        for participant in &channel_controller.load().participants {
            let actor = Self::create_actor(channel_controller.clone(), participant, runtime_manager.clone()).await?;
            let actor: Arc<dyn Actor> = Arc::from(actor);
            moderator.add_actor(actor.clone());
            actors.insert(actor.id(), actor);
        }
        let mentions = Self::mention_directory(&actors);
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", id, handle);
        }

        Ok(Self {
            id,
            session_id,
            channel: channel_controller,
            runtime_manager,
            actors,
            moderator: Mutex::new(moderator),
            mentions,
            recorder: Mutex::new(recorder),
            state,
            state_store,
            processing_state: Arc::new(AtomicU8::new(ActorState::Idle.into())),
            event_tx,
            max_route_decisions: 100,
            run_lock: Mutex::new(())
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    pub async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
    }


    pub fn id(&self) -> ChannelId { self.id }
    
    pub fn name(&self) -> String {
        self.channel.name()
    }

    pub async fn update_name(&self, new_name: String) {
        self.channel.update_name(new_name).await;
    }

    pub async fn cancel(&self) -> Result<()> {
        self.processing_state.store(ActorState::Cancelling.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.cancel().await?;
        }
        Ok(())
    }
    pub async fn wakeup(&self) -> Result<()> {
        self.processing_state.store(ActorState::Active.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.wakeup().await?;
        }
        Ok(())
    }

    pub fn actor_names(&self) -> Vec<String> {
        self.actors.values().map(|a| a.name()).collect()
    }
    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.values().map(|a| a.id()).collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.values()
            .map(|a| {
                let name = a.name();
                let handle = Some(handle_of(&name))
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
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
        self.actors.values().find(|c| c.id() == actor_id).map(|c| c.clone())
    }
    pub fn actor_by_name(&self, name: &str) -> Option<Arc<dyn Actor>> {
        self.actors.values().find(|c| c.name() == name).map(|c| c.clone())
    }

    pub fn user_actor(&self) -> Option<Arc<UserActor>> {
        self.actors.values().find_map(|actor| {
            let any_actor: Arc<dyn Any + Send + Sync> = actor.clone();
            Arc::downcast::<UserActor>(any_actor).ok()
        })
    }

    async fn create_actor(channel: Arc<ChannelController>, participant: &Participant, runtime_manager: Arc<RuntimeManager>) -> Result<Box<dyn Actor>> {
        let actor_runtime = runtime_manager.new_actor_runtime();
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
        let mut global_rx = self.runtime_manager.event_bus().subscribe_channel(self.id);
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let actors = Arc::new(self.spawn_actors(event_tx, &mut join_set)?);

        loop {
            tokio::select! {
                maybe_event = event_rx.recv() => {
                    if self.handle_actor_event(maybe_event, &actors).await {
                        break;
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global, &actors).await;
                }
            }
        }

        info!("Channel {}/{} stopped", self.channel.name(), self.id);
        Ok(())
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, actors: &Arc<HashMap<ActorId,ActorHandle>>) -> bool {
        if let Some(event) = maybe_event {
            let span = crate::logging::envelope_span(&event.context, event.event.kind());
            self.process_envelope(event, actors).instrument(span).await;
            false
        } else {
            true
        }
    }

    async fn process_envelope(&self, event: Envelope, actors: &Arc<HashMap<ActorId,ActorHandle>>) {
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

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>, _actors: &Arc<HashMap<ActorId, ActorHandle>>) {
        if let Some(global_event) = maybe_global {
            self.channel.process_global_event(&global_event).await;
        }
    }

    fn spawn_actors(&self, actor_output:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> Result<HashMap<ActorId,ActorHandle>> {
        let mut actors = HashMap::new();
        for (id, actor) in &self.actors {
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
                m.mentions = self.mentions.resolve(content);
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
                if let Some(actor) = self.actors.get(id) {
                    if matches!(id, ActorId::Agent(_)) && actor.state() == ActorState::Sleeping {
                        if let Err(e) = actor.wakeup().await {
                            warn!("Cannot wake mentioned agent {}: {}", id, e);
                        }
                    }
                }
            }
        }
    }

    async fn route_event(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) {
        if matches!(event.event, ActorEvent::WakeupRequested) {
            let _ = self.wakeup().await;
        }
        self.wake_mentioned(&event).await;

        //if let ActorEvent::FullResponseGenerated(messages) = &event.event {
        //    if messages.iter().any(|m| m.role == MessageRole::User) {
        //        let _ = self.wakeup().await;
        //    }
        //}

        let is_cancelling = ActorState::from(self.processing_state.load(Ordering::SeqCst)) == ActorState::Cancelling;
        let broadcast = if !is_cancelling {
            self.route_event_impl(actors, event.clone()).await
        } else {
            true // cancellation only suppresses internal routing, not external visibility
        };

        if broadcast {
            let _ = self.event_tx.send(event);
        }
    }

    async fn route_event_impl(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) -> bool {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return true
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let mut moderator = self.moderator.lock().await;
        let decision = moderator.route(&event).await;
        drop(moderator);

        // First, route all messages to all agents...
        for route in decision.forwards {
            let target_id = route.0;

            // Do not propagate message to sender again
            if target_id == event.context.sender_id {
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
