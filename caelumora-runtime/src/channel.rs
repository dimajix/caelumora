use crate::actor::{Actor, ActorSender, SendError};
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::approval::ApprovalActor;
use crate::actor_impl::user::UserActor;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use caelumora_model::actor::{ActorEvent, ActorId, Envelope};
use caelumora_model::agent::ProcessingState;
use caelumora_model::channel::{Channel, ChannelController, ChannelId, ChannelState, Participant};
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::Message;
use caelumora_model::session::SessionId;
use caelumora_model::store::{StateStore, StoreError};
use log::{debug, info, warn};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
    actor_sender: Box<dyn ActorSender>,
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    runtime_manager: Arc<RuntimeManager>,
    actors: HashMap<ActorId,Arc<dyn Actor>>,
    moderator: Mutex<Box<dyn Moderator>>,
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

        let mut actors = HashMap::new();
        for participant in &channel.participants {
            let actor = Self::create_actor(&channel, participant, runtime_manager.clone()).await?;
            let actor: Arc<dyn Actor> = Arc::from(actor);
            moderator.add_actor(actor.clone());
            actors.insert(actor.id(), actor);
        }

        let channel_controller = runtime_manager.controller_manager().new_channel_controller(channel);

        Ok(Self {
            id,
            session_id,
            channel: channel_controller,
            runtime_manager,
            actors,
            moderator: Mutex::new(moderator),
            recorder: Mutex::new(recorder),
            state,
            state_store,
            processing_state: Arc::new(AtomicU8::new(ProcessingState::Idle.into())),
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
        self.processing_state.store(ProcessingState::Cancelling.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.cancel().await?;
        }
        Ok(())
    }
    pub async fn wakeup(&self) -> Result<()> {
        self.processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
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

    async fn create_actor(channel: &Channel, participant: &Participant, runtime_manager: Arc<RuntimeManager>) -> Result<Box<dyn Actor>> {
        match participant {
            Participant::User(user) => {
                let actor = UserActor::new(channel, user.clone(), runtime_manager.clone()).await?;
                Ok(Box::new(actor))
            },
            Participant::Agent(agent) => {
                let actor = AgentActor::new(channel, agent.clone(), runtime_manager.clone()).await?;
                Ok(Box::new(actor))
            },
            Participant::Approval(id) => {
                let user_count = channel.participants.iter().filter(|p| matches!(p, Participant::User(_))).count();
                Ok(Box::new(ApprovalActor::new(*id, user_count)))
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
            debug!("Received actor event {:?}", event);
            self.record_receive_event(&event).await;
            self.append_history(&event).await;
            self.route_event(actors, event).await;
            //self.state_store.
            false
        } else {
            true
        }
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

    fn spawn_actor(&self, actor: Arc<dyn Actor>, actor_output:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let (tx, actor_input) = mpsc::channel(100);
        let actor_sender = actor.new_sender(tx);
        let abort_handle = join_set.spawn(async move {
            let result = actor.run(actor_input, actor_output).await;
            if let Err(e) = result {
                warn!("Failed to spawn actor {}: {}", actor.id(), e)
            }
            id
        });

        ActorHandle {
            id,
            abort_handle,
            actor_sender,
        }
    }

    async fn route_event(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) {
        if matches!(event.event, ActorEvent::WakeupRequested) {
            let _ = self.wakeup().await;
        }

        //if let ActorEvent::FullResponseGenerated(messages) = &event.event {
        //    if messages.iter().any(|m| m.role == MessageRole::User) {
        //        let _ = self.wakeup().await;
        //    }
        //}

        let is_cancelling = ProcessingState::from(self.processing_state.load(Ordering::SeqCst)) == ProcessingState::Cancelling;
        if !is_cancelling {
            self.route_event_impl(actors, event.clone()).await;
        }

        let _ = self.event_tx.send(event);
    }

    async fn route_event_impl(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return
        }

        let default_message = Message::default();
        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let mut moderator = self.moderator.lock().await;
        let routes = moderator.route(&event).await;
        drop(moderator);

        // First, route all messages to all agents...
        for route in routes {
            let target_id = route.0;

            // Do not propagate message to sender again
            if target_id == event.context.sender_id {
                continue;
            }

            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let ctx = &target_event.context;
            let event_desc = match &target_event.event {
                ActorEvent::PartialMessageGenerated(m) => {
                    format!("Partial(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
                }
                ActorEvent::CompleteMessageGenerated(m) => {
                    format!("Complete(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
                }
                ActorEvent::FullResponseGenerated(v) => {
                    let m = v.last().unwrap_or(&default_message);
                    format!("EndOfTurn(sender_id={}, sender_name={}, role={:?}, mid={})", ctx.sender_id, ctx.sender_name, m.role, m.id)
                }
                ActorEvent::ResponseRequested => {
                    format!("ResponseRequested(sender_id={}, sender_name={}", ctx.sender_id, ctx.sender_name)
                }
                _ => {
                    format!("{:?}", target_event.event)
                }
            };
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = span_id;
            target_event.context.span_depth = span_depth;
            debug!("Routing event from {:?} to {:?}", target_event.context.sender_id, target_id);
            if let Some(actor) = actors.get(&target_id) {
                debug!("route_messages: send event {} from={} to to={target_id}", event_desc, target_event.context.sender_id);
                self.record_send_event(&target_event).await;
                if let Err(e) = actor.actor_sender.try_send(target_event) {
                    match e {
                        SendError::Full => warn!("Queue full for actor {}", target_id),
                        SendError::Filtered => {}, // Normal behavior
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }
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
