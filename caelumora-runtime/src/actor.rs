use crate::message::{MessageEvent, MessageProcessor};
use crate::security::SecurityContext;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::session::{Channel, ChannelId, Session, SessionId};
use caelumora_model::user::User;
use chrono::{DateTime, Utc};
use log::{debug, info, warn};
use std::any::Any;
use std::fmt::Debug;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex, MutexGuard};
use tokio_stream::StreamExt;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub channel_name: String,
    pub sender_id: ActorId,
    pub sender_name: String,
    pub event_id: Uuid,
    pub correlation_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
}

impl EventContext {
    pub fn new(session_id: SessionId, channel_id: ChannelId, channel_name:String, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id,
            channel_id,
            channel_name,
            sender_id,
            sender_name,
            event_id: Uuid::new_v4(),
            correlation_id: Uuid::default(),
            span_id: Uuid::new_v4(),
            span_depth: 0,
        }
    }

    pub fn next(&self, sender_id:ActorId, sender_name:String) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            channel_name: self.channel_name.clone(),
            sender_id,
            sender_name,
            event_id: Uuid::new_v4(),
            correlation_id: self.event_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
        }
    }
}


#[derive(Debug, Clone)]
pub enum ActorEvent {
    ToolExecutionRequested,
    ToolExecutionGranted,
    ToolExecutionDenied,
    ResponseRequested,
    PartialMessageGenerated(Message),
    CompleteMessageGenerated(Message),
    FullResponseGenerated(Vec<Message>),
}

impl ActorEvent {
    pub fn from_message_event(event: MessageEvent) -> Self {
        match event {
            MessageEvent::Partial(msg) => {
                ActorEvent::PartialMessageGenerated(msg)
            }
            MessageEvent::Complete(msg) => {
                ActorEvent::CompleteMessageGenerated(msg)
            }
            MessageEvent::EndOfTurn(vec) => {
                ActorEvent::FullResponseGenerated(vec)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub timestamp: DateTime<Utc>,
    pub security: SecurityContext,
    pub context: EventContext,
    pub event: ActorEvent
}


impl Envelope {
    pub fn new(security: SecurityContext, context: EventContext, event: ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security,
            context,
            event
        }
    }

    pub fn forward(&self) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: self.event.clone(),
        }
    }
    pub fn forward_with_response(&self, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }
    pub fn forward_with_event(&self, event:ActorEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.clone(),
            context: self.context.clone(),
            event
        }
    }

    pub fn respond(&self, sender_id:ActorId, sender_name:String, messages:Vec<Message>) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.next(sender_id),
            context: self.context.next(sender_id, sender_name),
            event: ActorEvent::FullResponseGenerated(messages)
        }
    }

    pub fn with_event(mut self, event: ActorEvent) -> Self {
        self.event = event;
        self
    }
    pub fn with_security(mut self, security_context: SecurityContext) -> Self {
        self.security = security_context;
        self
    }
}


#[async_trait]
pub trait Actor : Any + Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    async fn run(&self, rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>;
}


#[derive(Debug)]
pub struct MessageProcessorActor {
    id: ActorId,
    kind: String,
    name: String,
    processor: Box<dyn MessageProcessor>,
    run_lock: Mutex<()>
}

impl MessageProcessorActor {
    pub fn new(processor: Box<dyn MessageProcessor>) -> Self {
        MessageProcessorActor {
            id: processor.id(),
            kind: processor.kind(),
            name: processor.name(),
            processor,
            run_lock: Mutex::new(())
        }
    }
}

impl MessageProcessorActor {
    async fn request_response(&self, messages:&Vec<Message>, event_context: &EventContext, event_security: &SecurityContext, tx: &Sender<Envelope>) {
        let mut response = self.processor.process_messages(messages.clone(), event_security).await;
        while let Some(event_result) = response.next().await {
            match event_result {
                Ok(ev) => {
                    let ev2 = Envelope::new(
                        event_security.next(self.id),
                        event_context.next(self.id, self.name.clone()),
                        ActorEvent::from_message_event(ev)
                    );
                    if let Err(e) = tx.send(ev2).await {
                        warn!("Actor {} cannot send response because of an error: {}", self.id, e);
                        return
                    }
                },
                Err(e) => {
                    warn!("Actor {} received an error: {}", self.id, e);
                    break;
                }
            }
        }
    }
}

#[async_trait]
impl Actor for MessageProcessorActor {
    fn id(&self) -> ActorId {
        self.id
    }
    fn kind(&self) -> String {
        self.kind.clone()
    }
    fn name(&self) -> String {
        self.name.clone()
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()>{
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());
        let mut buffer = Vec::new();
        let mut messsages = Vec::new();
        while rx.recv_many(&mut buffer, 100).await > 0 {
            debug!("AgentActor {} received actor events: {:?}", self.id(), buffer);
            let last_security = buffer.last().unwrap().security.clone();
            let last_context = buffer.last().unwrap().context.clone();
            for event in &mut buffer {
                match &mut event.event {
                    ActorEvent::FullResponseGenerated(m) => {
                        messsages.append(m);
                    }
                    ActorEvent::ResponseRequested => {
                        if !messsages.is_empty() {
                            self.request_response(&messsages, &last_context, &last_security, &tx).await;
                            messsages.clear();
                        }
                    }
                    _ => {}
                }
            }
            buffer.clear();
        }
        info!("AgentActor {} has finished finished", self.id());
        Ok(())
    }

}


#[derive(Debug)]
pub struct UserActor {
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
            session_id: session.id,
            channel_id: channel.id,
            channel_name: channel.name.clone(),
            security_context: SecurityContext {
                initiator_id: ActorId::User(user.id),
                sender_id: ActorId::User(user.id),
            },
            user,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            actor_rx: Mutex::new(arx),
            actor_tx: atx,
        }
    }

    pub async fn send_message(&self, message: ActorEvent) -> Result<()> {
        self.user_tx.send(message).await?;
        Ok(())
    }

    pub async fn lock_receiver(&self) -> MutexGuard<'_, Receiver<ActorEvent>> {
        self.actor_rx.lock().await
    }
}

#[async_trait]
impl Actor for UserActor {
    fn id(&self) -> ActorId {
        ActorId::User(self.user.id)
    }

    fn kind(&self) -> String{
        "user".to_string()
    }

    fn name(&self) -> String {
        self.user.username.clone()
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()> {
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
