use crate::channel::ChannelLoop;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use caelumora_model::session::{Channel, ChannelId, Session, SessionId};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::{AbortHandle, JoinSet};

struct ChannelHandle {
    id: ChannelId,
    abort_handle: AbortHandle,
}

#[derive(Debug)]
pub struct SessionLoop {
    session: Arc<Session>,
    runtime_manager: Arc<RuntimeManager>,
    run_lock: Mutex<()>
}

impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let session_ref = Arc::new(session);

        Ok(Self {
            session: session_ref,
            runtime_manager,
            run_lock: Mutex::new(())
        })
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Channel> {
        self.session.channels.iter().find(|c| c.id == channel_id).map(|c| c.clone())
    }
    pub fn channel_by_name(&self, name: &str) -> Option<Channel> {
        self.session.channels.iter().find(|c| c.name == name).map(|c| c.clone())
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        let channels = self.spawn_channels(&mut join_set);

        while let Some(res) = join_set.join_next().await {
            let idx = res?;
        }

        Ok(())
    }

    fn spawn_channels(&self, join_set: &mut JoinSet<ChannelId>) -> Result<HashMap<ChannelId, ChannelHandle>> {
        let mut channels = HashMap::new();
        for channel in &self.session.channels {
            let handle = self.spawn_channel(channel.clone(), join_set)?;
            channels.insert(channel.id, handle);
        };

        Ok(channels)
    }

    fn spawn_channel(&self, channel: Channel, join_set: &mut JoinSet<ChannelId>) -> Result<ChannelHandle> {
        let id = channel.id;
        let cl = ChannelLoop::new(&self.session, channel, self.runtime_manager.clone())?;
        let abort_handle = join_set.spawn(async move {
            let _ = cl.run().await;
            id
        });

        let handle = ChannelHandle {
            id,
            abort_handle,
        };
        Ok(handle)
    }
}


pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
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
        let id = session.id;
        let session_loop = SessionLoop::new(session, self.runtime_manager.clone())?;
        let session_ref = Arc::new(session_loop);
        self.sessions.insert(id, session_ref.clone());
        Ok(session_ref)
    }

    pub fn get_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|s| s.clone())
    }
}
