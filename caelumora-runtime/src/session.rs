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
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

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
                        let channel_id = res?;
                        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStopped(self.id, channel_id));
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
        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStarted(self.id, id));
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
    handles: DashMap<SessionId, JoinHandle<()>>,
    runtime_manager: Arc<RuntimeManager>,
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            runtime_manager,
        }
    }

    /// Build the runtime loop for an (already-persisted) session, register it, spawn its
    /// run task, and announce that the session has started running.
    async fn spawn_session(&self, session: Session) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let session_loop = Arc::new(SessionLoop::new(session, self.runtime_manager.clone()).await?);
        self.sessions.insert(id, session_loop.clone());

        let loop_for_task = session_loop.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                log::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.runtime_manager.event_bus().publish(GlobalEvent::SessionStarted(id));
        Ok(session_loop)
    }

    /// Create a brand-new session: persist it, announce its existence (and that of its
    /// channels), then start it.
    pub async fn create_session(&self, ctx: &SecurityContext, session: Session) -> Result<Arc<SessionLoop>> {
        let id = session.id;

        self.runtime_manager.entity_store().create_session(ctx, &session).await?;

        let event_bus = self.runtime_manager.event_bus();
        event_bus.publish(GlobalEvent::SessionCreated(id));
        for channel in &session.channels {
            event_bus.publish(GlobalEvent::ChannelCreated(id, channel.id));
        }

        self.spawn_session(session).await
    }

    /// Load an already-persisted session and start it. Fires `SessionStarted`, but not
    /// `SessionCreated` — the session already exists.
    pub async fn load_session(&self, id: SessionId, security_context: &SecurityContext) -> Result<Arc<SessionLoop>> {
        let session = self.runtime_manager.entity_store().get_session(security_context, id).await?;
        self.spawn_session(session).await
    }

    pub fn get_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|s| s.clone())
    }

    /// Stop a running session's loop without deleting its durable definition.
    pub fn stop_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        if let Some((_, handle)) = self.handles.remove(&id) {
            handle.abort();
        }
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if removed.is_some() {
            self.runtime_manager.event_bus().publish(GlobalEvent::SessionStopped(id));
        }
        removed
    }

    /// Stop and permanently delete a session together with all of its channels.
    pub async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> Result<bool> {
        // Capture the channel list before the runtime loop is torn down.
        let channels: Vec<ChannelId> = if let Some(session_loop) = self.get_session(id) {
            session_loop.get_session().channels.iter().map(|c| c.id).collect()
        } else {
            match self.runtime_manager.entity_store().get_session(ctx, id).await {
                Ok(session) => session.channels.iter().map(|c| c.id).collect(),
                Err(_) => Vec::new(),
            }
        };

        self.stop_session(id);

        let event_bus = self.runtime_manager.event_bus();
        for channel_id in &channels {
            event_bus.publish(GlobalEvent::ChannelDeleted(id, *channel_id));
        }

        let removed = self.runtime_manager.entity_store().delete_session(ctx, id).await?;
        event_bus.publish(GlobalEvent::SessionDeleted(id));
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::channel::Channel;
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;
    use uuid::Uuid;

    fn test_runtime() -> Arc<RuntimeManager> {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        Arc::new(RuntimeManager::new(store.clone(), store, secret_manager).unwrap())
    }

    fn make_session(name: &str) -> Session {
        let session_id = SessionId(Uuid::new_v4());
        let channel = Channel {
            id: ChannelId(Uuid::new_v4()),
            session_id,
            name: "main".to_string(),
            moderator: "broadcast".to_string(),
            participants: Vec::new(),
            security_attributes: SecurityAttributes::default(),
        };
        Session {
            id: session_id,
            metadata: Metadata {
                name: name.to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: vec![channel],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes::default(),
        }
    }

    /// Drain every event currently buffered on the receiver.
    fn drain(rx: &mut tokio::sync::mpsc::Receiver<GlobalEvent>) -> Vec<GlobalEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn create_session_emits_created_then_started() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let mut rx = runtime.event_bus().subscribe_all();
        let ctx = SecurityContext::default();

        let session = make_session("s1");
        let session_id = session.id;
        let channel_id = session.channels[0].id;

        manager.create_session(&ctx, session).await.unwrap();

        // The manager emits these synchronously, in order, before returning.
        let events = drain(&mut rx);
        let created = events.iter().position(|e| matches!(e, GlobalEvent::SessionCreated(id) if *id == session_id));
        let ch_created = events.iter().position(|e| matches!(e, GlobalEvent::ChannelCreated(sid, cid) if *sid == session_id && *cid == channel_id));
        let started = events.iter().position(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id));

        assert!(created.is_some(), "expected SessionCreated, got {:?}", events);
        assert!(ch_created.is_some(), "expected ChannelCreated, got {:?}", events);
        assert!(started.is_some(), "expected SessionStarted, got {:?}", events);
        assert!(created < ch_created && ch_created < started, "wrong order: {:?}", events);

        // It is now persisted and running.
        assert!(runtime.entity_store().get_session(&ctx, session_id).await.is_ok());
        assert!(manager.get_session(session_id).is_some());

        manager.stop_session(session_id);
    }

    #[tokio::test]
    async fn delete_session_emits_stopped_then_deleted() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();

        let session = make_session("s2");
        let session_id = session.id;
        let channel_id = session.channels[0].id;
        manager.create_session(&ctx, session).await.unwrap();

        // Subscribe after creation so we only observe the deletion events.
        let mut rx = runtime.event_bus().subscribe_all();
        assert!(manager.delete_session(&ctx, session_id).await.unwrap());

        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e, GlobalEvent::SessionStopped(id) if *id == session_id));
        let ch_deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(sid, cid) if *sid == session_id && *cid == channel_id));
        let deleted = events.iter().position(|e| matches!(e, GlobalEvent::SessionDeleted(id) if *id == session_id));

        assert!(stopped.is_some(), "expected SessionStopped, got {:?}", events);
        assert!(ch_deleted.is_some(), "expected ChannelDeleted, got {:?}", events);
        assert!(deleted.is_some(), "expected SessionDeleted, got {:?}", events);
        assert!(stopped < ch_deleted && ch_deleted < deleted, "wrong order: {:?}", events);

        assert!(runtime.entity_store().get_session(&ctx, session_id).await.is_err());
        assert!(manager.get_session(session_id).is_none());
    }

    #[tokio::test]
    async fn load_session_emits_started_but_not_created() {
        let runtime = test_runtime();
        let manager = SessionManager::new(runtime.clone());
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through create_session.
        let session = make_session("s3");
        let session_id = session.id;
        runtime.entity_store().create_session(&ctx, &session).await.unwrap();

        let mut rx = runtime.event_bus().subscribe_all();
        manager.load_session(session_id, &ctx).await.unwrap();

        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, GlobalEvent::SessionStarted(id) if *id == session_id)),
            "expected SessionStarted, got {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e, GlobalEvent::SessionCreated(_))),
            "load must not emit SessionCreated, got {:?}", events
        );

        manager.stop_session(session_id);
    }
}
