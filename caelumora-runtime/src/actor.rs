use async_trait::async_trait;
use chrono::{DateTime, Utc};
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use log::{info, warn};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_stream::StreamExt;
use uuid::Uuid;
use caelumora_model::user::User;

pub struct ActorEvent {
    pub timestamp: DateTime<Utc>,
    pub session_id: Uuid,
    pub channel_id: Uuid,
    pub sender_id: Uuid,
    pub span_id: Uuid,
    pub span_depth: i32,
    pub event: MessageEvent
}

impl ActorEvent {
    pub fn new(session_id: Uuid, channel_id: Uuid, sender_id:Uuid, event: MessageEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            session_id,
            channel_id,
            sender_id,
            span_id: Uuid::new_v4(),
            span_depth: 0,
            event
        }
    }

    pub fn next(&self, sender_id:Uuid, event:MessageEvent) -> Self {
        Self {
            timestamp: Utc::now(),
            session_id: self.session_id,
            channel_id: self.channel_id,
            sender_id,
            span_id: self.span_id,
            span_depth: self.span_depth + 1,
            event
        }
    }
}


#[async_trait]
pub trait Actor : Send + Sync {
    fn id(&self) -> Uuid;
    fn kind(&self) -> String;
    fn name(&self) -> String;

    async fn run(&mut self, rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>);
}


#[derive(Debug)]
pub struct MessageProcessorActor {
    id: Uuid,
    kind: String,
    name: String,
    processor: Box<dyn MessageProcessor>
}

impl MessageProcessorActor {
    pub fn new(processor: Box<dyn MessageProcessor>) -> Self {
        MessageProcessorActor {
            id: processor.id(),
            kind: processor.kind(),
            name: processor.name(),
            processor,
        }
    }
}

impl MessageProcessorActor {
    async fn handle_end_of_turn(&mut self, event:ActorEvent, tx: &Sender<ActorEvent>) {
        if let MessageEvent::EndOfTurn(messages) = &event.event {
            let mut response = self.processor.process_messages(messages.clone()).await;
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
    fn id(&self) -> Uuid {
        self.id
    }
    fn kind(&self) -> String {
        self.kind.clone()
    }
    fn name(&self) -> String {
        self.name.clone()
    }

    async fn run(&mut self, mut rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>) {
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
    }

}


pub struct UserActor {
    user: User
}

impl UserActor {
    pub fn new(user:User) -> Self {
        Self {
            user
        }
    }
}

#[async_trait]
impl Actor for UserActor {
    fn id(&self) -> Uuid {
        todo!()
    }

    fn kind(&self) -> String{
        "user".to_string()
    }

    fn name(&self) -> String {
        self.user.username.clone()
    }

    async fn run(&mut self, rx: Receiver<ActorEvent>, tx: Sender<ActorEvent>) {
        todo!()
    }
}

