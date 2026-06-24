use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::event::GlobalEvent;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::user::{User, UserController};
use chrono::Utc;
use log::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Debug)]
pub struct UserActor {
    pub id: ActorId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub security_context: SecurityContext,
    pub user: Arc<UserController>,
    pub runtime_manager: Arc<crate::runtime::RuntimeManager>,
    pub user_rx: Mutex<Receiver<ActorEvent>>,
    user_tx: Sender<ActorEvent>,
    actor_rx: Mutex<Receiver<ActorEvent>>,
    actor_tx: Sender<ActorEvent>,
}

impl UserActor {
    pub async fn new(channel: &Channel, user:User, runtime_manager: Arc<crate::runtime::RuntimeManager>) -> Result<Self> {
        let (utx,urx) = mpsc::channel(100);
        let (atx,arx) = mpsc::channel(100);
        let user_controller = runtime_manager.controller_manager().new_user_controller(user, channel.name.clone());
        Ok(Self {
            id: ActorId::User(user_controller.id()),
            session_id: channel.session_id,
            channel_id: channel.id,
            security_context: SecurityContext {
                initiator_id: ActorId::User(user_controller.id()),
                sender_id: ActorId::User(user_controller.id()),
                session_id: channel.session_id,
                channel_id: channel.id
            },
            user: user_controller,
            runtime_manager,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            actor_rx: Mutex::new(arx),
            actor_tx: atx,
        })
    }

    pub async fn send_message(&self, message: String) -> anyhow::Result<()> {
        let msg = Message {
            role: MessageRole::User,
            content: Some(message),
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name(),
            timestamp: Utc::now(),
            ..Message::default()
        };
        self.user_tx.send(ActorEvent::CompleteMessageGenerated(msg.clone())).await?;
        self.user_tx.send(ActorEvent::FullResponseGenerated(vec![msg])).await?;
        Ok(())
    }

    pub async fn lock_receiver(&self) -> MutexGuard<'_, Receiver<ActorEvent>> {
        self.actor_rx.lock().await
    }

    async fn handle_actor_envelope(&self, maybe_event: Option<Envelope>) -> bool {
        let mut rx_closed = false;
        match maybe_event {
            Some(event) => {
                debug!("UserActor {} received actor event: {:?}", self.id(), event);
                if let Err(e) = self.actor_tx.send(event.event).await {
                    warn!("UserActor {} cannot forward event because of an error: {}", self.id(), e);
                    rx_closed = true;
                }
            }
            None => {
                rx_closed = true;
            }
        }
        rx_closed
    }

    async fn handle_user_event(&self, maybe_message: Option<ActorEvent>, tx: &Sender<Envelope>) -> bool {
        let mut user_rx_closed = false;
        match maybe_message {
            Some(message) => {
                debug!("Received user message: {:?}", message);
                let channel_name = self.user.channel_name();
                let context = EventContext::new(self.session_id, self.channel_id, channel_name, self.id(), self.name());
                let event = Envelope::new(self.security_context.clone(), context, message);
                if let Err(e) = tx.send(event).await {
                    warn!("UserActor {} cannot forward user message because of an error: {}", self.id(), e);
                    user_rx_closed = true;
                }
            }
            None => {
                user_rx_closed = true;
            }
        }
        user_rx_closed
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) -> bool {
        let mut global_rx_closed = false;
        match maybe_global {
            Some(event) => {
                match event {
                    GlobalEvent::ChannelUpdated(_, cid, event) => {
                        if cid == self.channel_id {
                            self.user.process_channel_update(&event);
                        }
                    }
                    GlobalEvent::UserUpdated(id, event) => {
                        if ActorId::User(id) == self.id {
                            self.user.process_update(&event);
                        }
                    }
                    _ => {}
                }
            }
            None => {
                global_rx_closed = true;
            }
        }
        global_rx_closed
    }
}

#[async_trait]
impl Actor for UserActor {
    fn id(&self) -> ActorId {
        self.id
    }

    fn kind(&self) -> String{
        "user".to_string()
    }

    fn name(&self) -> String {
        self.user.username()
    }

    fn new_sender(&self, tx: Sender<Envelope>) -> Box<dyn ActorSender> {
        Box::new(StandardActorSender {
            tx,
            filter: Box::new(|_| true),
        })
    }

    async fn run(&self, mut input: Receiver<Envelope>, output: Sender<Envelope>) -> anyhow::Result<()> {
        let mut user_rx = self.user_rx.try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;
        
        let user_id = self.id.as_user_id().unwrap();
        let mut global_rx = self.runtime_manager.event_bus().subscribe(vec![
            crate::event::SubscriptionTopic::User(user_id),
            crate::event::SubscriptionTopic::Channel(self.channel_id),
        ]);
        let mut rx_closed = false;
        let mut user_rx_closed = false;
        let mut global_rx_closed = false;

        info!("UserActor {} is starting", self.id());
        while !rx_closed || !user_rx_closed || !global_rx_closed {
            tokio::select! {
                maybe_event = input.recv(), if !rx_closed => {
                    rx_closed = self.handle_actor_envelope(maybe_event).await;
                }
                maybe_message = user_rx.recv(), if !user_rx_closed => {
                    user_rx_closed = self.handle_user_event(maybe_message, &output).await;
                }
                maybe_global = global_rx.recv(), if !global_rx_closed => {
                    global_rx_closed = self.handle_global_event(maybe_global).await;
                }
                else => break,
            }
        }
        info!("UserActor {} has finished", self.id());
        Ok(())
    }
}
