use crate::channel::ChannelLoop;
use crate::runtime::RuntimeManager;
use anyhow::{anyhow, Result};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, Session, SessionController, SessionId};
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
    id: SessionId,
    session_controller: Arc<SessionController>,
    runtime_manager: Arc<RuntimeManager>,
    channels: HashMap<ChannelId,Arc<ChannelLoop>>,
    run_lock: Mutex<()>
}

impl SessionLoop {
    pub async fn new(session: Session, runtime_manager: Arc<RuntimeManager>) -> Result<Self> {
        let id = session.id;
        let mut channels = HashMap::new();
        for channel in &session.channels {
            let cl = ChannelLoop::new(channel.clone(), runtime_manager.clone()).await?;
            channels.insert(cl.id(), Arc::new(cl));
        }

        let session_controller = runtime_manager.controller_manager().new_session_controller(session).await?;

        Ok(Self {
            id,
            session_controller,
            runtime_manager,
            channels,
            run_lock: Mutex::new(())
        })
    }

    pub fn id(&self) -> SessionId {
        self.session_controller.id()
    }

    pub fn get_session(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session_controller.load()
    }

    pub fn name(&self) -> String {
        self.session_controller.name()
    }

    pub async fn update_name(&self, new_name: String) {
        self.session_controller.update_name(new_name).await;
    }

    pub fn channels(&self) -> Vec<Arc<ChannelLoop>> {
        self.channels.values().cloned().collect()
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.values().find(|c| c.id() == channel_id).map(|c| c.clone())
    }
    pub async fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        for channel in self.channels.values() {
            if channel.name() == name {
                return Some(channel.clone());
            }
        }
        None
    }

    pub async fn cancel_channel(&self, channel_id: ChannelId) -> Result<()> {
        if let Some(c) = self.channel_by_id(channel_id) {
            c.cancel().await?;
            Ok(())
        } else {
            Err(anyhow!("Channel not found"))
        }
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        let _channels = self.spawn_channels(&mut join_set);
        let mut global_rx = self.runtime_manager.event_bus().subscribe_session(self.id());

        loop {
            tokio::select! {
                res = join_set.join_next() => {
                    if let Some(res) = res {
                        let _idx = res?;
                    } else {
                        break;
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global).await;
                }
            }
        }

        Ok(())
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) {
        if let Some(global_event) = maybe_global {
            self.session_controller.process_global_event(&global_event).await;
        }
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
    pub sessions: DashMap<SessionId, Arc<SessionLoop>>,
    runtime_manager: Arc<RuntimeManager>,
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            runtime_manager,
        }
    }

    pub async fn create_session(&self, session: Session) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let session_loop = SessionLoop::new(session, self.runtime_manager.clone()).await?;
        let session_ref = Arc::new(session_loop);
        self.sessions.insert(id, session_ref.clone());
        Ok(session_ref)
    }

    pub async fn load_session(&self, id: SessionId) -> Result<Arc<SessionLoop>> {
        let session = self.runtime_manager.entity_store().get_session(&SecurityContext::default(), id).await.expect("failed to load session");
        self.create_session(session).await
    }

    pub fn get_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|s| s.clone())
    }
}
