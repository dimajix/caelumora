use crate::agent::AgentLoop;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runner::LoopRunner;
use crate::runtime::RuntimeManager;
use crate::session::SessionEvent::{AgentEvent, RouteRequest};
use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use caelumora_model::session::{Participant, Session};
use dashmap::DashMap;
use futures::future::join_all;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use log::info;
use tokio::sync::mpsc;
use uuid::Uuid;

enum SessionEvent {
    AgentEvent(MessageEvent),
    RouteRequest(Vec<Message>)
}


struct MessageStream {
    stream: BoxStream<'static, Result<MessageEvent>>,
    all_messages: Vec<Message>,
}

impl MessageStream {
    fn new(stream: BoxStream<'static, Result<MessageEvent>>) -> Self {
        Self {
            stream,
            all_messages: Vec::new(),
        }
    }

    async fn next_event(mut self) -> Result<(Self, Option<MessageEvent>)> {
        let event = self.stream.try_next().await?;
        if let Some(MessageEvent::Complete(message)) = &event {
            self.all_messages.push(message.clone());
        }

        Ok((self, event))
    }

    fn all_messages(
        self,
    ) -> Vec<Message> {
        self.all_messages
    }
}


struct RouterState {
    agents: Arc<HashMap<Uuid, LoopRunner>>,
    moderator: Arc<Box<dyn Moderator>>,
    num_route_decisions: i32,
    max_route_decisions: i32
}
impl RouterState {
    fn new(lp:&SessionLoop) -> Self {
        Self {
            agents: lp.agents.clone(),
            moderator: lp.moderator.clone(),
            num_route_decisions: 0,
            max_route_decisions: 3
        }
    }

    async fn route_messages(&mut self, messages: Vec<Message>, tx:mpsc::UnboundedSender<SessionEvent>) -> Result<()> {
        let routes = self.moderator.route(messages.as_slice());
        let agents = self.agents.clone();
        info!("route_messages: process messages={:?}", messages);

        if self.num_route_decisions >= self.max_route_decisions {
            return Err(anyhow::anyhow!("Failed to send messages back into SessionLoop"));
        }
        self.num_route_decisions = self.num_route_decisions + 1;

        // First, route all messages to all agents...
        let futures = routes.into_iter().flat_map(|route| {
            // Do not propagate message to sender again
            let target_id = route.0;
            let target_messages = route.1;
            return if !target_messages.is_empty() && !target_messages.iter().any(|m| m.sender_id == target_id) {
                if let Some(agent_loop) = agents.get(&target_id) {
                    info!("route_messages: send msg {:?} to agent {target_id}", target_messages);
                    Some(agent_loop.send_messages(target_messages))
                } else {
                    None
                }
            } else {
                None
            }
        });

        // Wait until we receive a BoxStream from all agents
        let mut pending = FuturesUnordered::new();
        for result in join_all(futures).await {
            let state = MessageStream::new(result);
            pending.push(state.next_event());
        }

        while let Some(result) = pending.next().await {
            let (stream, event) = result?;

            match event {
                Some(event) => {
                    tx.send(AgentEvent(event))
                        .map_err(|_| anyhow::anyhow!("Failed to send messages back into SessionLoop"))?;
                    pending.push(stream.next_event());
                }
                None => {
                    // If that was the last event of that stream, request routing of the full event set again
                    info!("route_messages: submit messages={:?}", messages);
                    tx.send(RouteRequest(stream.all_messages()))
                        .map_err(|_| anyhow::anyhow!("Failed to send messages back into SessionLoop"))?
                }
            }
        }

        Ok(())
    }

}


#[derive(Debug, Clone)]
pub struct SessionLoop {
    session: Arc<Session>,
    agents: Arc<HashMap<Uuid, LoopRunner>>,
    moderator: Arc<Box<dyn Moderator>>,
    recorder: Arc<Box<dyn Recorder>>,
}

#[async_trait]
impl MessageProcessor for SessionLoop {
    fn id(&self) -> Uuid {
        self.session.id
    }

    fn kind(&self) -> String {
        "session".to_string()
    }

    fn name(&self) -> String {
        self.session.metadata.name.clone()
    }

    async fn process_messages(
        &mut self,
        messages: Vec<Message>,
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let mut state = RouterState::new(self);
        let (tx,mut rx) = mpsc::unbounded_channel();

        let stream = try_stream! {
            // Send initial messages as request
            info!("process_messages: submit messages={:?}", messages);
            if let Err(_e) = tx.send(RouteRequest(messages)) {
                Err(anyhow::anyhow!("Failed to send command"))?
            }

            // Now process each event as it arrives in the channel
            while let Some(msg) = rx.recv().await {
                match msg {
                    RouteRequest(msg) => {
                        // TODO: Apply recorder
                        state.route_messages(msg, tx.clone()).await?;
                    }
                    AgentEvent(ev) => {
                        yield ev
                    }
                }
            }
        };
        Box::pin(stream)
    }
}

impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let mut agents = HashMap::new();
        for p in session.participants.iter() {
            match &p {
                Participant::User => {},
                Participant::Agent(agent) => {
                    let l = AgentLoop::new(agent.clone(), runtime_manager.clone())?;
                    let (runner, _) = LoopRunner::spawn(Box::new(l));
                    agents.insert(agent.id.clone(), runner);
                }
            }
        };
        let moderator = runtime_manager.moderator(&"".to_string())?;
        let recorder = runtime_manager.recorder(&"".to_string())?;
        Ok(Self {
            session: Arc::new(session),
            agents: Arc::new(agents),
            moderator: Arc::new(moderator),
            recorder: Arc::new(recorder),
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn id(&self) -> Uuid {
        self.session.id
    }
}


pub struct SessionManager {
    sessions: DashMap<Uuid, Arc<SessionLoop>>,
    runtime_manager: Arc<RuntimeManager>,
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            runtime_manager,
        }
    }

    pub fn create_session(&self, session: Session) -> Result<Arc<SessionLoop>> {
        let session_loop = SessionLoop::new(session, self.runtime_manager.clone())?;
        let session_ref = Arc::new(session_loop);
        let id = session_ref.id();
        self.sessions.insert(id, session_ref.clone());
        Ok(session_ref)
    }

    pub fn get_session(&self, id: Uuid) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|s| s.clone())
    }
}
