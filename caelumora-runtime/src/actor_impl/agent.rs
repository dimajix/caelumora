use std::mem;
use anyhow::anyhow;
use async_trait::async_trait;
use log::{debug, info, warn};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use caelumora_model::message::{ActorId, Message};
use crate::actor::{Actor, ActorEvent, Envelope, EventContext};
use crate::agent::AgentLoop;
use crate::security::SecurityContext;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    name: String,
    agent: AgentLoop,
    run_lock: Mutex<()>
}

impl AgentActor {
    pub fn new(agent: AgentLoop) -> Self {
        AgentActor {
            id: agent.id(),
            kind: "agent".to_string(),
            name: agent.name(),
            agent,
            run_lock: Mutex::new(())
        }
    }
}

impl AgentActor {
    async fn send_messages(&self, messages:&mut Vec<Message>, event_context: &EventContext, event_security: &SecurityContext) {
        self.agent.send_messages(mem::take(messages), event_security).await;
    }
    async fn request_response(&self, event_context: &EventContext, event_security: &SecurityContext, tx: &Sender<Envelope>) {
        let mut response = self.agent.request_response(event_security).await;
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
impl Actor for AgentActor {
    fn id(&self) -> ActorId {
        self.id
    }
    fn kind(&self) -> String {
        self.kind.clone()
    }
    fn name(&self) -> String {
        self.name.clone()
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> anyhow::Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("AgentActor {} already running", self.id()))?;

        info!("AgentActor {} is starting", self.id());

        let mut buffer = Vec::new();
        let mut response_requested = false;

        while rx.recv_many(&mut buffer, 100).await > 0 {
            debug!("AgentActor {} received actor events: {:?}", self.id(), buffer);
            let last_security = buffer.last().unwrap().security.clone();
            let last_context = buffer.last().unwrap().context.clone();
            for event in &mut buffer {
                match &mut event.event {
                    ActorEvent::FullResponseGenerated(m) => {
                        self.send_messages(m, &last_context, &last_security).await;
                    }
                    ActorEvent::ResponseRequested => {
                        response_requested = true;
                    }
                    _ => {}
                }
            }
            buffer.clear();

            if response_requested {
                self.request_response(&last_context, &last_security, &tx).await;
                response_requested = false;
            }
        }

        info!("AgentActor {} has finished finished", self.id());
        Ok(())
    }

}
