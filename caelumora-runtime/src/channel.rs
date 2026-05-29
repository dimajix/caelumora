use crate::actor::{Actor, ActorEvent, MessageProcessorActor, UserActor};
use crate::agent::AgentLoop;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use caelumora_model::message::{ActorId, Message, MessageId};
use caelumora_model::session::{Channel, ChannelId, Participant, Session, SessionId};
use caelumora_model::user::{User, UserId};
use log::{debug, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::any::Any;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use uuid::Uuid;
use crate::message::MessageEvent;

struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
    request_tx: Sender<ActorEvent>,
}



#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    name: String,
    session_id: SessionId,
    channel: Arc<Channel>,
    runtime_manager: Arc<RuntimeManager>,
    actors: HashMap<ActorId,Arc<dyn Actor>>,
    moderator: Box<dyn Moderator>,
    recorder: Box<dyn Recorder>,
    max_route_decisions: i32,
    run_lock: Mutex<()>
}

impl ChannelLoop {
    pub fn new(session: &Session, channel: Channel, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let moderator = runtime_manager.moderator(&"".to_string())?;
        let recorder = runtime_manager.recorder(&"".to_string())?;

        let mut actors = HashMap::new();
        for participant in &channel.participants {
            let actor = Self::create_actor(session, &channel, participant, runtime_manager.clone())?;
            moderator.add_actor(actor.as_ref());
            actors.insert(actor.id(), Arc::from(actor));
        }

        Ok(Self {
            id: channel.id,
            name: channel.name.clone(),
            session_id: session.id,
            channel: Arc::new(channel),
            runtime_manager,
            actors,
            moderator,
            recorder,
            max_route_decisions: 100,
            run_lock: Mutex::new(())
        })
    }

    pub fn id(&self) -> ChannelId { self.id }
    pub fn name(&self) -> String { self.name.clone() }

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
                let actor = UserActor::new(session, channel, user.clone());
                Ok(Box::new(actor))
            },
            Participant::Agent(agent) => {
                let l = AgentLoop::new(agent.clone(), runtime_manager.clone())?;
                let actor = MessageProcessorActor::new(Box::new(l));
                //self.moderator.add_actor(&actor);
                Ok(Box::new(actor))
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name, self.id);
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let actors = Arc::new(self.spawn_actors(event_tx, &mut join_set)?);

        while let Some(event) = event_rx.recv().await {
            debug!("Received actor event {:?}", event);
            self.recorder.record(&event);
            self.route_event(&actors, &event).await
        }

        info!("Channel {}/{} stopped", self.channel.name, self.id);
        Ok(())
    }

    fn spawn_actors(&self, event_tx:Sender<ActorEvent>, join_set: &mut JoinSet<ActorId>) -> Result<HashMap<ActorId,ActorHandle>> {
        let mut actors = HashMap::new();
        for (id, actor) in &self.actors {
            let handle = self.spawn_actor(actor.clone(), event_tx.clone(), join_set);
            actors.insert(*id, handle);
        };

        Ok(actors)
    }

    fn spawn_actor(&self, actor: Arc<dyn Actor>, event_tx:Sender<ActorEvent>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let (msg_tx, msg_rx) = mpsc::channel(10);
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
            request_tx: msg_tx,
        }
    }

    async fn route_event(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: &ActorEvent) {
        let default_message = Message::default();
        let routes = self.moderator.route(&event);

        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return
        }

        // First, route all messages to all agents...
        for route in routes {
            // Do not propagate message to sender again
            let target_id = route.0;
            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let event_desc = match &target_event.event {
                MessageEvent::Partial(m) => {
                    format!("Partial(sender_id={}, sender_name={}, role={:?}, mid={})", m.sender_id, m.sender_name, m.role, m.id)
                }
                MessageEvent::Complete(m) => {
                    format!("Complete(sender_id={}, sender_name={}, role={:?}, mid={})", m.sender_id, m.sender_name, m.role, m.id)
                }
                MessageEvent::EndOfTurn(v) => {
                    let m = v.last().unwrap_or(&default_message);
                    format!("EndOfTurn(sender_id={}, sender_name={}, role={:?}, mid={})", m.sender_id, m.sender_name, m.role, m.id)
                }
            };
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = event.context.span_id;
            target_event.context.span_depth = event.context.span_depth + 1;
            debug!("Routing event from {:?} to {:?}", target_event.context.sender_id, target_id);
            if target_event.context.sender_id != target_id {
                if let Some(actor) = actors.get(&target_id) {
                    debug!("route_messages: send event {} from={} to to={target_id}", event_desc, target_event.context.sender_id);
                    if let Err(e) = actor.request_tx.try_send(target_event) {
                        warn!("Could not forward event {} from={} to={}: {}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }
    }
}
