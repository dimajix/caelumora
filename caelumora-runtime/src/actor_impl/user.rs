use crate::actor::Actor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::ApprovalResponse;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::user::{User, UserController, UserId};
use chrono::Utc;
use log::{debug, info};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use uuid::Uuid;

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    user_id: UserId,
    session_id: SessionId,
    channel_id: ChannelId,
    security_context: SecurityContext,
    user: Arc<UserController>,
    actor_runtime: ActorRuntime,
    user_rx: Mutex<Receiver<ActorEvent>>,
    user_tx: Sender<ActorEvent>,
    actor_rx: Mutex<Receiver<ActorEvent>>,
    actor_tx: Sender<ActorEvent>,
}

impl UserActor {
    pub async fn new(channel: &Channel, mut user:User, actor_runtime: ActorRuntime) -> Result<Self> {
        user.security.session_id = channel.session_id;
        user.security.channel_id = channel.id;
        let user_id = user.id;
        let security_context = user.security.clone();
        let user_controller = actor_runtime.controller_manager().new_user_controller(user);

        let (utx,urx) = mpsc::channel(100);
        let (atx,arx) = mpsc::channel(100);

        Ok(Self {
            id: ActorId::User(user_id),
            user_id,
            session_id: channel.session_id,
            channel_id: channel.id,
            security_context,
            user: user_controller,
            actor_runtime,
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

    pub async fn send_approval(&self, response: ApprovalResponse, approved: bool) -> anyhow::Result<()> {
        let event = if approved {
            ActorEvent::ApprovalGranted(response)
        } else {
            ActorEvent::ApprovalDenied(response)
        };
        self.user_tx.send(event).await?;
        Ok(())
    }

    pub async fn lock_receiver(&self) -> MutexGuard<'_, Receiver<ActorEvent>> {
        self.actor_rx.lock().await
    }

    async fn handle_actor_envelope(&self, maybe_event: Option<Envelope>) -> Result<bool> {
        match maybe_event {
            Some(event) => {
                debug!("UserActor {} received actor event: {:?}", self.id(), event);
                self.actor_tx
                    .send(event.event)
                    .await
                    .context(format!("UserActor {} failed to forward event", self.id))?;
                Ok(false)
            }
            None => Ok(true),
        }
    }

    async fn handle_user_event(
        &self,
        maybe_message: Option<ActorEvent>,
        tx: &dyn EventSender,
    ) -> Result<bool> {
        match maybe_message {
            Some(message) => {
                debug!("Received user message: {:?}", message);
                let context =
                    EventContext::new(self.session_id, self.channel_id, self.id(), self.name());
                let event = Envelope::new(self.security_context.clone(), context, message);
                tx.send(event)
                    .await
                    .context(format!("UserActor {} failed to forward user message", self.id))?;
                Ok(false)
            }
            None => Ok(true),
        }
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) -> bool {
        let mut global_rx_closed = false;
        match maybe_global {
            Some(event) => {
                self.user.process_global_event(&event);
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

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> anyhow::Result<()> {
        let mut user_rx = self.user_rx.try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;

        let user_id = self.user_id;
        let mut global_rx = self.actor_runtime.event_bus().subscribe(vec![
            crate::event::SubscriptionTopic::User(user_id),
            crate::event::SubscriptionTopic::Channel(self.channel_id),
        ]);
        let mut rx_closed = false;
        let mut user_rx_closed = false;
        let mut global_rx_closed = false;

        info!("UserActor {} is starting", self.id());
        while !rx_closed || !user_rx_closed || !global_rx_closed {
            tokio::select! {
                maybe_event = rx.recv(), if !rx_closed => {
                    rx_closed = self.handle_actor_envelope(maybe_event).await?;
                }
                maybe_message = user_rx.recv(), if !user_rx_closed => {
                    user_rx_closed = self.handle_user_event(maybe_message, &*tx).await?;
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
