use anyhow::anyhow;
use log::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use uuid::Uuid;
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole};
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::session::{Session, SessionId};
use caelumora_model::user::{User, UserController};
use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::security::SecurityContext;
use crate::runtime::GlobalEvent;
use async_trait::async_trait;

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
    pub fn new(session: &Session, channel: &Channel, user:User, runtime_manager: Arc<crate::runtime::RuntimeManager>) -> Self {
        let (utx,urx) = mpsc::channel(100);
        let (atx,arx) = mpsc::channel(100);
        let event_sender = runtime_manager.user_event_sender(user.id);
        let user_controller = Arc::new(UserController::new(user, channel.name.clone(), event_sender));
        Self {
            id: ActorId::User(user_controller.id()),
            session_id: session.id,
            channel_id: channel.id,
            security_context: SecurityContext {
                initiator_id: ActorId::User(user_controller.id()),
                sender_id: ActorId::User(user_controller.id()),
                session_id: session.id,
                channel_id: channel.id
            },
            user: user_controller,
            runtime_manager,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            actor_rx: Mutex::new(arx),
            actor_tx: atx,
        }
    }

    pub async fn send_message(&self, mut message: ActorEvent) -> anyhow::Result<()> {
        match &mut message {
            ActorEvent::PartialMessageGenerated(msg) => {
                self.fixup_message(msg);
            }
            ActorEvent::CompleteMessageGenerated(msg) => {
                self.fixup_message(msg);
            }
            ActorEvent::FullResponseGenerated(msgs) => {
                for m in msgs {
                    self.fixup_message(m);
                }
            }
            _ => {}
        }
        self.user_tx.send(message).await?;
        Ok(())
    }

    pub async fn lock_receiver(&self) -> MutexGuard<'_, Receiver<ActorEvent>> {
        self.actor_rx.lock().await
    }

    fn fixup_message(&self, msg:& mut Message) {
        if msg.id == MessageId::default() {
            msg.id = MessageId(Uuid::new_v4());
        }
        msg.sender_id = self.id();
        msg.sender_name = self.name();
        msg.role = MessageRole::User;
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

    async fn handle_global_event(&self, maybe_global: Result<GlobalEvent, tokio::sync::broadcast::error::RecvError>) -> bool {
        let mut global_rx_closed = false;
        match maybe_global {
            Ok(event) => {
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
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                global_rx_closed = true;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Lagged, just continue
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

    fn create_channel(&self) -> (Box<dyn ActorSender>, Receiver<Envelope>) {
        let (tx, rx) = mpsc::channel(100);
        (
            Box::new(StandardActorSender {
                tx,
                filter: Box::new(|_| true),
            }),
            rx
        )
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> anyhow::Result<()> {
        let mut user_rx = self.user_rx.try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;
        let mut global_rx = self.runtime_manager.subscribe_global();
        let mut rx_closed = false;
        let mut user_rx_closed = false;
        let mut global_rx_closed = false;

        info!("UserActor {} is starting", self.id());
        while !rx_closed || !user_rx_closed || !global_rx_closed {
            tokio::select! {
                maybe_event = rx.recv(), if !rx_closed => {
                    rx_closed = self.handle_actor_envelope(maybe_event).await;
                }
                maybe_message = user_rx.recv(), if !user_rx_closed => {
                    user_rx_closed = self.handle_user_event(maybe_message, &tx).await;
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
