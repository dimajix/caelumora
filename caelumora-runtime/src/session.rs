use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::message::{Message, MessageEvent, MessageProcessor};
use caelumora_model::session::{Session, SessionTemplate};
use dashmap::DashMap;
use futures::future::join_all;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;
use crate::runner::LoopRunner;


pub struct SessionLoop {
    session: Session,
    runtime: Arc<RuntimeManager>,
    agents: HashMap<Uuid, LoopRunner>,
    moderator: Box<dyn Moderator>,
    recorder: Box<dyn Recorder>,
}

#[async_trait]
impl MessageProcessor for SessionLoop {
    async fn send_messages(
        &mut self,
        messages: &[Message],
    ) -> BoxStream<'static, Result<MessageEvent>> {
        // Save message into session history
        self.session.history.extend(messages.to_vec());

        // 1. Process message by the target agent
        //let responses = self.route_message(message).await?;
        //for resp in responses.iter() {
        // 2. Recursively process all received messages
        //    self.send_message(resp).await?;
        //}

        //Ok(responses)
        todo!()
    }
}

impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        // TODO
        todo!()
    }

    pub fn run() {

    }

    async fn route_messages(&mut self, messages: &[Message]) -> Result<Vec<Message>> {
        // 2. Route responses to other agents based on orchestration strategy
        let routes = self.moderator.route(messages);
        let agents = self.agents;

        let mut futures = Vec::new();
        for route in routes.iter() {
            // Do not propagate message to sender again
            let tid = route.0;
            let rm = route.1;
            if !rm.iter().any(|m| m.sender_id == *tid) {
                let agent_loop = agents
                    .get_mut(tid)
                    .ok_or_else(|| anyhow!("Target agent not found"))?;
                let f = agent_loop.send_messages(rm);
                futures.push(f);
            }
        }

        let joined = join_all(futures).await;

        let mut responses: Vec<Message> = Vec::new();
        for result in joined {
            //let target_responses = result?;
            //responses.extend(target_responses);
        }

        Ok(responses)
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
