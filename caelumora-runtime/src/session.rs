use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use caelumora_model::session::{Session, SessionTemplate};
use dashmap::DashMap;
use futures::future::{join_all, ready};
use futures::stream::{once, BoxStream};
use std::collections::HashMap;
use std::sync::Arc;
use async_stream::try_stream;
use futures::{FutureExt, StreamExt, TryStreamExt};
use tokio::sync::mpsc;
use uuid::Uuid;
use crate::runner::LoopRunner;


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
        todo!()
    }

    async fn route_messages(&mut self, messages: Vec<Message>) -> BoxStream<'static, Result<MessageEvent>> {
        let routes = self.moderator.route(messages.as_slice());
        let agents = &self.agents;

        // First route all messages to all agents...
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

        // Wait until we receive a BoxStream from all agents
        let joined = join_all(futures).await;

        // And now process each BoxStream
        let tx = self.tx.clone();
        let stream = try_stream! {
            for mut result in joined {
                let mut all_messages = Vec::new();
                while let Some(msg) = result.try_next().await? {
                    match &msg {
                        MessageEvent::Partial(_) => {}
                        MessageEvent::Complete(msg) => {
                            //self.session.history.push(msg.clone());
                            all_messages.push(msg.clone())
                        }
                        MessageEvent::EndOfTurn => {}
                    }
                    yield msg;
                }
                if !all_messages.is_empty() {
                    if let Err(_e) = tx.send(all_messages) {
                        Err(anyhow::anyhow!("Failed to send messages back into SessionLoop"))?
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
    async fn send_messages(
        &mut self,
        messages: Vec<Message>,
    ) -> BoxStream<'static, Result<MessageEvent>> {
        let mut state = LoopState::new(self);

        // Save message into session history
        //state.session.history.extend(messages.to_vec());

        if let Err(e) = state.tx.send(messages) {
            return once(ready(Err(anyhow::anyhow!("Failed to send command"))))
                .boxed();
        }

        let stream = try_stream! {
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
        todo!()
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

    pub fn create_session_loop(&self, template: &SessionTemplate) -> Result<Arc<SessionLoop>> {
        let session = Session::new(template)?;
        let session_loop = SessionLoop::new(session, self.runtime_manager.clone())?;
        let session_ref = Arc::new(session_loop);
        let id = session_ref.id();
        self.sessions.insert(id, session_ref.clone());
        Ok(session_ref)
    }

    pub fn get_session(&self, id: Uuid) -> Option<Session> {
        self.sessions.get(&id).map(|s| s.session().clone())
    }

    pub fn list_sessions(&self) -> Vec<Session> {
        todo!()
    }
}
