use crate::actor::Actor;
use crate::message::MessageProcessor;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::message::Message;
use caelumora_model::security::SecurityContext;
use log::{debug, info};
use std::mem;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

#[derive(Debug)]
pub struct MessageProcessorActor {
    id: ActorId,
    kind: String,
    processor: Box<dyn MessageProcessor>,
    run_lock: Mutex<()>,
}

impl MessageProcessorActor {
    pub fn new(processor: Box<dyn MessageProcessor>) -> Self {
        MessageProcessorActor {
            id: processor.id(),
            kind: processor.kind(),
            processor,
            run_lock: Mutex::new(()),
        }
    }
}

impl MessageProcessorActor {
    async fn request_response(
        &self,
        messages: &mut Vec<Message>,
        event_context: &EventContext,
        event_security: &SecurityContext,
        tx: &dyn EventSender,
    ) -> Result<()> {
        let mut response = self
            .processor
            .process_messages(mem::take(messages), event_security)
            .await;
        while let Some(event_result) = response.next().await {
            let ev = event_result.context(format!("Actor {} failed to process messages", self.id))?;
            let ev2 = Envelope::new(
                event_security.next(self.id),
                event_context.next(self.id, self.name()),
                ActorEvent::from_message_event(ev),
            );
            tx.send(ev2)
                .await
                .context(format!("Actor {} failed to send response event", self.id))?;
        }
        Ok(())
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

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    async fn wakeup(&self) -> Result<()> {
        Ok(())
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| !matches!(event.event, ActorEvent::PartialMessageGenerated(_))
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> anyhow::Result<()> {
        let _guard = self
            .run_lock
            .try_lock()
            .map_err(|_| anyhow!("MessageProcessorActor {} already running", self.id()))?;

        info!("MessageProcessorActor {} is starting", self.id());

        let mut messages = Vec::new();
        let mut response_requested = false;
        let mut last_context: Option<EventContext> = None;
        let mut last_security: Option<SecurityContext> = None;

        loop {
            let Some(event) = rx.recv().await else { break };
            debug!("MessageProcessorActor {} received event: {:?}", self.id(), event);

            last_security = Some(event.security.clone());
            last_context = Some(event.context.clone());

            match event.event {
                ActorEvent::FullResponseGenerated(mut m) => {
                    messages.append(&mut m);
                }
                ActorEvent::ResponseRequested => {
                    response_requested = true;
                }
                _ => {}
            }

            if response_requested && !messages.is_empty() {
                if let (Some(ctx), Some(sec)) = (last_context.take(), last_security.take()) {
                    self.request_response(&mut messages, &ctx, &sec, &*tx).await?;
                }
                messages.clear();
                response_requested = false;
            }
        }

        info!("MessageProcessorActor {} has finished", self.id());
        Ok(())
    }
}
