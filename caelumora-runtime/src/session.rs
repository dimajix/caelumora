use crate::channel::ChannelLoop;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use caelumora_model::message::Message;
use caelumora_model::session::{ChannelId, Session, SessionId};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tokio::task::{AbortHandle, JoinSet};
use crate::actor::Envelope;

struct ChannelHandle {
    id: ChannelId,
    abort_handle: AbortHandle,
}

#[derive(Debug)]
pub struct SessionLoop {
    session: Arc<Session>,
    runtime_manager: Arc<RuntimeManager>,
    channels: HashMap<ChannelId,Arc<ChannelLoop>>,
    run_lock: Mutex<()>
}

impl SessionLoop {
    pub fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let mut channels = HashMap::new();
        for channel in &session.channels {
            let cl = ChannelLoop::new(&session, channel.clone(), runtime_manager.clone())?;
            channels.insert(cl.id(), Arc::new(cl));
        }
        let session_ref = Arc::new(session);

        Ok(Self {
            session: session_ref,
            runtime_manager,
            channels,
            run_lock: Mutex::new(())
        })
    }

    pub fn id(&self) -> SessionId { self.session.id }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.values().find(|c| c.id() == channel_id).map(|c| c.clone())
    }
    pub fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        self.channels.values().find(|c| c.name() == name).map(|c| c.clone())
    }

    pub fn get_channel_history(&self, channel_id: ChannelId) -> Option<Vec<Message>> {
        self.channel_by_id(channel_id).map(|c| c.history_blocking())
    }

    pub fn subscribe_to_channel(&self, channel_id: ChannelId) -> Option<broadcast::Receiver<Envelope>> {
        self.channel_by_id(channel_id).map(|c| c.subscribe())
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        let _channels = self.spawn_channels(&mut join_set);

        while let Some(res) = join_set.join_next().await {
            let _idx = res?;
        }

        Ok(())
    }

    fn spawn_channels(&self, join_set: &mut JoinSet<ChannelId>) -> Result<HashMap<ChannelId, ChannelHandle>> {
        let mut channels = HashMap::new();
        for (id, channel) in &self.channels {
            let handle = self.spawn_channel(channel.clone(), join_set)?;
            channels.insert(id.clone(), handle);
        };

        Ok(channels)
    }

    fn spawn_channel(&self, channel: Arc<ChannelLoop>, join_set: &mut JoinSet<ChannelId>) -> Result<ChannelHandle> {
        let id = channel.id();
        let abort_handle = join_set.spawn(async move {
            let _ = channel.run().await;
            channel.id()
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
