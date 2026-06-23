use crate::actor::{Actor, ActorEvent, ActorSender, Envelope, EventContext, StandardActorSender};
use crate::agent::AgentLoop;
use crate::event::GlobalEvent;
use anyhow::anyhow;
use async_trait::async_trait;
use caelumora_model::message::{ActorId, Message};
use caelumora_model::security::SecurityContext;
use log::{debug, info, warn};
use std::mem;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

#[derive(Debug)]
pub struct AgentActor {
    id: ActorId,
    kind: String,
    agent: AgentLoop,
    run_lock: Mutex<()>
}

impl AgentActor {
    pub fn new(agent: AgentLoop) -> Self {
        AgentActor {
            id: agent.id(),
            kind: "agent".to_string(),
            agent,
            run_lock: Mutex::new(())
        }
    }
}

impl AgentActor {
    async fn handle_actor_envelopes(
        &self,
        res: usize,
        buffer: &mut Vec<Envelope>,
        tx: &Sender<Envelope>
    ) -> bool {
        if res == 0 {
            return true;
        }

        debug!("AgentActor {} received actor events: {:?}", self.id(), buffer);
        let last_security = buffer.last().unwrap().security.clone();
        let last_context = buffer.last().unwrap().context.clone();
        let mut response_requested = false;

        for event in buffer.iter_mut() {
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
            self.request_response(&last_context, &last_security, tx).await;
        }
        false
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) {
        if let Some(global_event) = maybe_global {
            match global_event {
                GlobalEvent::AgentUpdated(_, _, id, event) => {
                    if id == self.id.as_agent_id().unwrap() {
                        let _ = self.agent.process_update(event).await;
                    }
                }
                _ => {}
            }
        }
    }

    async fn send_messages(&self, messages:&mut Vec<Message>, _event_context: &EventContext, event_security: &SecurityContext) {
        self.agent.send_messages(mem::take(messages), event_security).await;
    }

    async fn request_response(&self, event_context: &EventContext, event_security: &SecurityContext, tx: &Sender<Envelope>) {
        let ev2 = Envelope::new(
            event_security.next(self.id),
            event_context.next(self.id, self.agent.name()),
            ActorEvent::ResponseStarted
        );
        if let Err(e) = tx.send(ev2).await {
            warn!("Actor {} cannot send response because of an error: {}", self.id, e);
            return
        }

        let mut response = self.agent.request_response(event_security).await;
        while let Some(event_result) = response.next().await {
            match event_result {
                Ok(ev) => {
                    let ev2 = Envelope::new(
                        event_security.next(self.id),
                        event_context.next(self.id, self.agent.name()),
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
        self.agent.name()
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

        let agent_id = self.id.as_agent_id().unwrap();
        let mut global_rx = self.agent.runtime_manager().event_bus().subscribe_agent(agent_id);
        let mut buffer = Vec::new();

        loop {
            tokio::select! {
                res = rx.recv_many(&mut buffer, 100) => {
                    if self.handle_actor_envelopes(res, &mut buffer, &tx).await {
                        break;
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global).await;
                }
            }
        }

        info!("AgentActor {} has finished", self.id());
        Ok(())
    }

    async fn cancel(&self) -> anyhow::Result<()> {
        self.agent.cancel();
        Ok(())
    }
}
