use crate::actor::{Actor, ActorEvent, MessageProcessorActor};
use crate::agent::AgentLoop;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use caelumora_model::session::{Channel, Participant, Session};
use log::{debug, info};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use uuid::Uuid;


struct ActorHandle {
    task: JoinHandle<()>,
    request_tx: Sender<ActorEvent>,
}


#[derive(Debug)]
pub struct ChannelLoop {
    channel_id: Uuid,
    session_id: Uuid,
    channel: Arc<Channel>,
    runtime_manager: Arc<RuntimeManager>,
    moderator: Box<dyn Moderator>,
    recorder: Box<dyn Recorder>,
    event_tx: Sender<ActorEvent>,
    event_rx: Receiver<ActorEvent>,
    max_route_decisions: i32
}

impl ChannelLoop {
    pub fn new(session: &Session, channel: Channel, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::channel(10);
        let moderator = runtime_manager.moderator(&"".to_string())?;
        let recorder = runtime_manager.recorder(&"".to_string())?;

        Ok(Self {
            channel_id: channel.id,
            session_id: session.id,
            channel: Arc::new(channel),
            runtime_manager,
            moderator,
            recorder,
            event_tx,
            event_rx,
            max_route_decisions: 100,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Channel {}/{} is starting", self.channel.name, self.channel_id);
        let actors = Arc::new(self.spawn_actors()?);

        while let Some(event) = self.event_rx.recv().await {
            if let Err(e) = self.route_event(&actors, &event).await {
                todo!()
            }
        }

        info!("Channel {}/{} stopped", self.channel.name, self.channel_id);
        Ok(())
    }

    fn spawn_actors(&self) -> Result<HashMap<Uuid,ActorHandle>> {
        let mut actors = HashMap::new();
        for p in &self.channel.participants {
            match p {
                Participant::User => {
                    todo!()
                },
                Participant::Agent(agent) => {
                    let l = AgentLoop::new(agent.clone(), self.runtime_manager.clone())?;
                    let (msg_tx, msg_rx) = mpsc::channel(10);
                    let mut actor= MessageProcessorActor::new(Box::new(l));

                    let event_tx = self.event_tx.clone();
                    let task = tokio::spawn(async move {
                        actor.run(msg_rx, event_tx).await;
                    });

                    let handle = ActorHandle {
                        task,
                        request_tx: msg_tx,
                    };

                    self.moderator.add_agent(agent)?;
                    actors.insert(agent.id.clone(), handle);
                }
            }
        };

        Ok(actors)
    }

    async fn route_event(&self, actors: &Arc<HashMap<Uuid,ActorHandle>>, event: &ActorEvent) -> Result<()> {
        let routes = self.moderator.route(&event);

        if event.context.span_depth >= self.max_route_decisions {
            return Err(anyhow::anyhow!("Too many routings, stopping further propagation"));
        }

        // First, route all messages to all agents...
        for route in routes {
            // Do not propagate message to sender again
            let target_id = route.0;
            let mut target_event = route.1;
            target_event.context.channel_id = self.channel_id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = event.context.span_id;
            target_event.context.span_depth = event.context.span_depth + 1;
            if target_event.context.sender_id != target_id {
                if let Some(actor) = actors.get(&target_id) {
                    debug!("route_messages: send event from={} to target={target_id}", target_event.context.sender_id);
                    if let Err(e) = actor.request_tx.try_send(target_event) {
                        todo!()
                    }
                }
            }
        }

        Ok(())
    }
}