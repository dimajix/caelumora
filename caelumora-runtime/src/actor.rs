use std::fmt::Debug;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::user::User;
use chrono::{DateTime, Utc};
use log::{info, warn};
use tokio::sync::{mpsc, Mutex};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use uuid::Uuid;
use caelumora_model::message::ActorId;
use caelumora_model::session::{ChannelId, SessionId};
use crate::message::{MessageEvent, MessageProcessor};
use crate::security::SecurityContext;

#[derive(Debug, Clone)]
pub struct EventContext {
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub sender_id: ActorId,
    pub span_id: Uuid,
    pub span_depth: i32,
}

impl EventContext {
    pub fn new(session_id: SessionId, channel_id: ChannelId, sender_id:ActorId) -> Self {
        Self {
            session_id,
            channel_id,
            sender_id,
            span_id: Uuid::new_v4(),
            span_depth: 0,
        }
    }

    pub fn next(&self, sender_id:ActorId) -> Self {
        Self {
            session_id: self.session_id,
            channel_id: self.channel_id,
            sender_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
        }
    }

}

#[derive(Debug, Clone)]
pub struct ActorEvent {
    pub timestamp: DateTime<Utc>,
    pub security: SecurityContext,
    pub context: EventContext,
    pub event: MessageEvent
}

impl ActorEvent {
    pub fn new(security: SecurityContext, context: EventContext, event: MessageEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security,
            context,
            event
        }
    }

    pub fn next(&self, sender_id:ActorId, event:MessageEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            security: self.security.next(sender_id),
            context: self.context.next(sender_id),
            event
        }
    }

    pub fn with_event(mut self, event: MessageEvent) -> Self {
        self.event = event;
        self
    }
    pub fn with_security(mut self, security_context: SecurityContext) -> Self {
        self.security = security_context;
        self
    }
}


#[async_trait]
pub trait Actor : Debug + Send + Sync {
    fn id(&self) -> ActorId;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    async fn run(&self, rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>) -> Result<()>;
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
    async fn handle_end_of_turn(&self, event:ActorEvent, tx: &Sender<ActorEvent>) {
        if let MessageEvent::EndOfTurn(messages) = &event.event {
            let mut response = self.processor.process_messages(messages.clone(), &event.security).await;
            while let Some(event_result) = response.next().await {
                match event_result {
                    Ok(ev) => {
                        let ev2 = event.next(self.id, ev);
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

    async fn run(&self, mut rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>) -> Result<()>{
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Actor {} is starting", self.id);
        while let Some(cmd) = rx.recv().await {
            match cmd.event {
                MessageEvent::EndOfTurn(_) => {
                    self.handle_end_of_turn(cmd, &tx).await
                }
                _ => {}
            }
        }
        info!("Actor {} has finished finished", self.id);
        Ok(())
    }

}


#[derive(Debug)]
pub struct UserActor {
    user: User,
    user_rx: Receiver<MessageEvent>,
    user_tx: Sender<MessageEvent>,
    actor_rx: Receiver<MessageEvent>,
    actor_tx: Sender<MessageEvent>,
    run_lock: Mutex<()>
}

impl UserActor {
    pub fn new(user:User) -> Self {
        let (utx,urx) = mpsc::channel(100);
        let (atx,arx) = mpsc::channel(100);
        Self {
            user,
            user_rx: urx,
            user_tx: utx,
            actor_rx: arx,
            actor_tx: atx,
            run_lock: Mutex::new(())
        }
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

    async fn run(&self, mut rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Actor {} is starting", self.user.id);
        while let Some(event) = rx.recv().await {
            if let Err(e) = self.actor_tx.try_send(event.event) {
                warn!("Cannot send event back")
            }
        }
        info!("Actor {} has finished finished", self.user.id);
        Ok(())
    }
}
