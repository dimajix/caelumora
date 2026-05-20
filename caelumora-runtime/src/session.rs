use crate::channel::ChannelLoop;
use crate::runtime::RuntimeManager;
use anyhow::Result;
use caelumora_model::message::MessageProcessor;
use caelumora_model::session::Session;
use dashmap::DashMap;
use futures::StreamExt;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug)]
pub struct SessionLoop {
    session: Arc<Session>,
    channels: Vec<ChannelLoop>,
}


impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let channels = Vec::new();
        for c in &session.channels {
            let l = ChannelLoop::new(&session, c.clone(), runtime_manager.clone());
        }

        Ok(Self {
            session: Arc::new(session),
            channels,
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn id(&self) -> Uuid {
        self.session.id
    }

    pub async fn run(&mut self) -> Result<()> {
        todo!()
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
