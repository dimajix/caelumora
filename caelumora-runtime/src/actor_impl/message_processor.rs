use std::mem;
use anyhow::anyhow;
use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::security::SecurityContext;
use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::message::MessageProcessor;

#[derive(Debug)]
pub struct MessageProcessorActor {
    id: ActorId,
    kind: String,
    processor: Box<dyn MessageProcessor>,
    run_lock: Mutex<()>
}

impl MessageProcessorActor {
    pub fn new(processor: Box<dyn MessageProcessor>) -> Self {
        MessageProcessorActor {
            id: processor.id(),
            kind: processor.kind(),
            processor,
            run_lock: Mutex::new(())
        }
    }
}

impl MessageProcessorActor {
    async fn request_response(&self, messages:&mut Vec<Message>, event_context: &EventContext, event_security: &SecurityContext, tx: &Sender<Envelope>) {
        let mut response = self.processor.process_messages(mem::take(messages), event_security).await;
        while let Some(event_result) = response.next().await {
            match event_result {
                Ok(ev) => {
                    let ev2 = Envelope::new(
                        event_security.next(self.id),
                        event_context.next(self.id, self.name()),
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
        self.processor.name()
    }

    fn create_channel(&self) -> (Box<dyn ActorSender>, Receiver<Envelope>) {
        let (tx, rx) = mpsc::channel(10);
        (
            Box::new(StandardActorSender {
                tx,
                filter: Box::new(|event| {
                    !matches!(event.event, ActorEvent::PartialMessageGenerated(_))
                }),
            }),
            rx
        )
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> anyhow::Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());

        let mut buffer = Vec::new();
        let mut messages = Vec::new();
        let mut response_requested = false;

        while rx.recv_many(&mut buffer, 100).await > 0 {
            debug!("AgentActor {} received actor events: {:?}", self.id(), buffer);
            let last_security = buffer.last().unwrap().security.clone();
            let last_context = buffer.last().unwrap().context.clone();
            for event in &mut buffer {
                match &mut event.event {
                    ActorEvent::FullResponseGenerated(m) => {
                        messages.append(m);
                    }
                    ActorEvent::ResponseRequested => {
                        response_requested = true;
                    }
                    _ => {}
                }
            }
            buffer.clear();

            if response_requested && !messages.is_empty() {
                self.request_response(&mut messages, &last_context, &last_security, &tx).await;
                messages.clear();
                response_requested = false;
            }
        }

        info!("AgentActor {} has finished finished", self.id());
        Ok(())
    }

}

