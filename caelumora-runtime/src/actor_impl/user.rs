use anyhow::anyhow;
use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use uuid::Uuid;
use caelumora_model::message::{ActorId, Message, MessageId, MessageRole};
use caelumora_model::session::{Channel, ChannelId, Session, SessionId};
use caelumora_model::user::User;
use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::security::SecurityContext;

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    session_id: SessionId,
    channel_id: ChannelId,
    channel_name: String,
    security_context: SecurityContext,
    user: User,
    user_rx: Mutex<Receiver<ActorEvent>>,
    user_tx: Sender<ActorEvent>,
    actor_rx: Mutex<Receiver<ActorEvent>>,
    actor_tx: Sender<ActorEvent>,
}

impl UserActor {
    pub fn new(session: &Session, channel: &Channel, user:User) -> Self {
        let (utx,urx) = mpsc::channel(100);
        let (atx,arx) = mpsc::channel(100);
        Self {
            id: ActorId::User(user.id),
            session_id: session.id,
            channel_id: channel.id,
            channel_name: channel.name.clone(),
            security_context: SecurityContext {
                initiator_id: ActorId::User(user.id),
                sender_id: ActorId::User(user.id),
                session_id: session.id,
                channel_id: channel.id
            },
            user,
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
        self.user.username.clone()
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
        let mut rx_closed = false;
        let mut user_rx_closed = false;

        info!("UserActor {} is starting", self.id());
        while !rx_closed || !user_rx_closed {
            tokio::select! {
                maybe_event = rx.recv(), if !rx_closed => {
                    match maybe_event {
                        Some(event) => {
                            debug!("UserActor {} received actor event: {:?}", self.id(), event);
                            if let Err(e) = self.actor_tx.send(event.event).await {
                                warn!("UserActor {} cannot forward event because of an error: {}", self.id(), e);
                                rx_closed = true;
                                user_rx_closed = true;
                                continue;
                            }
                        }
                        None => {
                            rx_closed = true;
                        }
                    }
                }
                maybe_message = user_rx.recv(), if !user_rx_closed => {
                    match maybe_message {
                        Some(message) => {
                            debug!("Received user message: {:?}", message);
                            let context = EventContext::new(self.session_id, self.channel_id, self.channel_name.clone(), self.id(), self.name());
                            let event = Envelope::new(self.security_context.clone(), context, message);
                            if let Err(e) = tx.send(event).await {
                                warn!("UserActor {} cannot forward user message because of an error: {}", self.id(), e);
                                rx_closed = true;
                                user_rx_closed = true;
                                continue;
                            }
                        }
                        None => {
                            user_rx_closed = true;
                        }
                    }
                }
                else => break,
            }
        }
        info!("UserActor {} has finished finished", self.id());
        Ok(())
    }
}
