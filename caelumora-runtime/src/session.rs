use crate::agent::AgentLoop;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runner::LoopRunner;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use async_stream::try_stream;
use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use caelumora_model::session::{ParticipantKind, Session};
use dashmap::DashMap;
use futures::future::join_all;
use futures::stream::{BoxStream, FuturesUnordered};
use futures::{StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;


struct StreamState {
    stream: BoxStream<'static, Result<MessageEvent>>,
    all_messages: Vec<Message>,
}

impl StreamState {
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


struct LoopState {
    session: Arc<Session>,
    agents: Arc<HashMap<Uuid, LoopRunner>>,
    moderator: Arc<Box<dyn Moderator>>,
    recorder: Arc<Box<dyn Recorder>>,
    rx: mpsc::UnboundedReceiver<Vec<Message>>,
    tx: mpsc::UnboundedSender<Vec<Message>>,
}

impl LoopState {
    fn new(aloop: &SessionLoop) -> Self {
        let (tx,rx) = mpsc::unbounded_channel();
        Self {
            session: aloop.session.clone(),
            agents: aloop.agents.clone(),
            moderator: aloop.moderator.clone(),
            recorder: aloop.recorder.clone(),
            rx,
            tx
        }
    }

    async fn route_messages(&mut self, messages: Vec<Message>) -> BoxStream<'static, Result<MessageEvent>> {
        let routes = self.moderator.route(messages.as_slice());
        let agents = &self.agents;

        // First, route all messages to all agents...
        let futures = routes.into_iter().flat_map(|route| {
            // Do not propagate message to sender again
            let target_id = route.0;
            let target_messages = route.1;
            return if !target_messages.is_empty() && !target_messages.iter().any(|m| m.sender_id == target_id) {
                if let Some(agent_loop) = agents.get(&target_id) {
                    Some(agent_loop.send_messages(target_messages))
                } else {
                    None
                }
            } else {
                None
            }
        });

        // And now process each BoxStream
        let mut pending = FuturesUnordered::new();

        // Wait until we receive a BoxStream from all agents
        for result in join_all(futures).await {
            let state = StreamState::new(result);
            pending.push(state.next_event());
        }

        let tx = self.tx.clone();
        let stream = try_stream! {
            while let Some(result) = pending.next().await {
                let (state, event) = result?;

                match event {
                    Some(event) => {
                        yield event;
                        pending.push(state.next_event());
                    }
                    None => {
                        tx.send(state.all_messages())
                            .map_err(|_| anyhow::anyhow!("Failed to send messages back into SessionLoop"))?
                    }
                }
            }
        };
        Box::pin(stream)
    }
}


pub struct SessionLoop {
    session: Arc<Session>,
    agents: Arc<HashMap<Uuid, LoopRunner>>,
    moderator: Arc<Box<dyn Moderator>>,
    recorder: Arc<Box<dyn Recorder>>,
}

#[async_trait]
impl MessageProcessor for SessionLoop {
    async fn process_messages(
        &mut self,
        messages: Vec<Message>,
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let mut state = LoopState::new(self);

        // Save message into session history
        //state.session.history.extend(messages.to_vec());

        // Stream results
        let stream = try_stream! {
            if let Err(_e) = state.tx.send(messages) {
                Err(anyhow::anyhow!("Failed to send command"))?
            }

            while let Some(msg) = state.rx.recv().await {
                let mut routed = state.route_messages(msg).await;

                while let Some(event) = routed.try_next().await? {
                    yield event;
                }
            }
        };
        Box::pin(stream)
    }
}

impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        // TODO
        let mut agents = HashMap::new();
        for p in session.participants.iter() {
            match &p.kind {
                ParticipantKind::User => {},
                ParticipantKind::Agent(agent) => {
                    let l = AgentLoop::new(agent.clone(), runtime_manager.clone());
                    let runner = LoopRunner::new(Box::new(l));
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
