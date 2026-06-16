use crate::agent::AgentLoop;
use crate::moderator::Moderator;
use crate::recorder::{Recorder};
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU8, Ordering};
use caelumora_model::agent::ProcessingState;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::session::{Channel, ChannelId, ChannelState, Participant, Session, SessionId, ChannelController};
use caelumora_model::user::{User, UserId};
use log::{debug, info, warn};
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use uuid::Uuid;
use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, SendError};
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::user::UserActor;
use crate::runtime::RuntimeManager;

struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
    request_tx: Box<dyn ActorSender>,
}



#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel_controller: Arc<ChannelController>,
    runtime_manager: Arc<RuntimeManager>,
    actors: HashMap<ActorId,Arc<dyn Actor>>,
    moderator: Mutex<Box<dyn Moderator>>,
    recorder: Mutex<Box<dyn Recorder>>,
    state: Arc<Mutex<ChannelState>>,
    processing_state: Arc<AtomicU8>,
    event_tx: broadcast::Sender<Envelope>,
    max_route_decisions: i32,
    run_lock: Mutex<()>
}

impl ChannelLoop {
    pub fn new(session: &Session, channel: Channel, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let channel_id = channel.id;
        let event_sender = runtime_manager.channel_event_sender(session.id, channel_id);
        let channel_controller = Arc::new(ChannelController::new(channel.clone(), event_sender));
        let mut moderator = runtime_manager.new_moderator(&channel.moderator)?;
        let state = Arc::new(Mutex::new(ChannelState::new(channel.id)));
        let recorder = runtime_manager.new_recorder(state.clone())?;
        let (event_tx, _) = broadcast::channel(100);

        let mut actors = HashMap::new();
        for participant in &channel.participants {
            let actor = Self::create_actor(session, &channel, participant, runtime_manager.clone())?;
            moderator.add_actor(actor.as_ref());
            actors.insert(actor.id(), Arc::from(actor));
        }

        Ok(Self {
            id: channel_id,
            session_id: session.id,
            channel_controller,
            runtime_manager,
            actors,
            moderator: Mutex::new(moderator),
            recorder: Mutex::new(recorder),
            state,
            processing_state: Arc::new(AtomicU8::new(ProcessingState::Idle.into())),
            event_tx,
            max_route_decisions: 100,
            run_lock: Mutex::new(())
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    pub fn send_event(&self, envelope: Envelope) {
        let _ = self.event_tx.send(envelope);
    }

    pub async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
    }


    pub fn id(&self) -> ChannelId { self.id }
    
    pub fn name(&self) -> String {
        self.channel_controller.name()
    }

    pub async fn update_name(&self, new_name: String) -> Result<()> {
        self.channel_controller.update_name(new_name);
        Ok(())
    }

    pub async fn cancel_all_actors(&self) -> Result<()> {
        self.processing_state.store(ProcessingState::Cancelling.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.cancel().await?;
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

    fn create_actor(session: &Session, channel: &Channel, participant: &Participant, runtime_manager: Arc<RuntimeManager>) -> Result<Box<dyn Actor>> {
        match participant {
            Participant::User => {
                let user = User {
                    id: UserId(Uuid::new_v4()),
                    username: "user".to_string(),
                    email: String::new(),
                    roles: vec!["user".to_string()],
                };
                let actor = UserActor::new(session, channel, user.clone(), runtime_manager.clone());
                Ok(Box::new(actor))
            },
            Participant::Agent(agent) => {
                let l = AgentLoop::new(agent.clone(), runtime_manager.clone())?;
                let actor = AgentActor::new(l);
                Ok(Box::new(actor))
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel_controller.name(), self.id);
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let actors = Arc::new(self.spawn_actors(event_tx, &mut join_set)?);

        while let Some(event) = event_rx.recv().await {
            debug!("Received actor event {:?}", event);
            self.record_receive_event(&event).await;
            self.route_event(&actors, event).await
        }

        info!("Channel {}/{} stopped", self.channel_controller.name(), self.id);
        Ok(())
    }

    fn spawn_actors(&self, event_tx:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> Result<HashMap<ActorId,ActorHandle>> {
        let mut actors = HashMap::new();
        for (id, actor) in &self.actors {
            let handle = self.spawn_actor(actor.clone(), event_tx.clone(), join_set);
            actors.insert(*id, handle);
        };

        Ok(actors)
    }

    fn spawn_actor(&self, actor: Arc<dyn Actor>, event_tx:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let (request_tx, msg_rx) = actor.create_channel();
        let event_tx = event_tx.clone();
        let abort_handle = join_set.spawn(async move {
            let result = actor.run(msg_rx, event_tx).await;
            if let Err(e) = result {
                warn!("Failed to spawn actor {}: {}", actor.id(), e)
            }
            id
        });

        ActorHandle {
            id,
            abort_handle,
            request_tx,
        }
    }

    async fn route_event(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) {
        if matches!(event.event, ActorEvent::Wakeup) {
            self.processing_state.store(ProcessingState::Active.into(), Ordering::SeqCst);
        }

        let is_cancelling = ProcessingState::from(self.processing_state.load(Ordering::SeqCst)) == ProcessingState::Cancelling;
        if (!is_cancelling) {
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
        let mut routes = moderator.route(&event);
        drop(moderator);

        // Always route back event to source, this helps the Actor to rebuild the full
        // history in correct order
        routes.push((event.context.sender_id, event.clone()));

        // First, route all messages to all agents...
        for route in routes {
            let target_id = route.0;

            if target_id == event.context.sender_id {
                continue;
            }

            // Do not propagate message to sender again
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
                if let Err(e) = actor.request_tx.try_send(target_event) {
                    match e {
                        SendError::Full => warn!("Queue full for actor {}", target_id),
                        SendError::Filtered => {}, // Normal behavior
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
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
