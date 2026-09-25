use crate::channel::ChannelLoop;
use crate::runtime::{ChannelRuntime, RuntimeManager};
use anyhow::{anyhow, Result};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::store::StoreError;
use tracing::info;
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
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let mut channels = HashMap::new();
        for channel in &session.channels {
            let runtime = ChannelRuntime::new(runtime_manager.clone(), commands.clone());
            let cl = ChannelLoop::new(channel.clone(), runtime).await?;
            channels.insert(cl.id(), cl);
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

    /// Renames the session on behalf of `context`.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the
    /// session, or whatever else storing it reports.
    pub async fn update_name(&self, context: &SecurityContext, new_name: String) -> Result<()> {
        Ok(self.session_controller.update_name(context, new_name).await?)
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

    pub async fn cancel_channel(&self, channel_id: ChannelId, context: &SecurityContext) -> Result<()> {
        if let Some(c) = self.channel_by_id(channel_id) {
            c.cancel(context).await?;
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
                        let attributes = self.channels.get(&channel_id).map(|c| c.security_attributes()).unwrap_or_default();
                        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStopped(self.id, channel_id), attributes);
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
        self.runtime_manager.event_bus().publish(GlobalEvent::ChannelStarted(self.id, id), channel.security_attributes());
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
        let attributes = session.security_attributes.clone();
        let session_loop = Arc::new(SessionLoop::new(session, self.runtime_manager.clone()).await?);
        self.sessions.insert(id, session_loop.clone());

        let loop_for_task = session_loop.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.runtime_manager.event_bus().publish(GlobalEvent::SessionStarted(id), attributes);
        Ok(session_loop)
    }

    /// Create a brand-new session: persist it, announce its existence (and that of its
    /// channels), then start it.
    pub async fn create_session(&self, ctx: &SecurityContext, session: Session) -> Result<Arc<SessionLoop>> {
        let id = session.id;

        self.runtime_manager.entity_store().create_session(ctx, &session).await?;

        let event_bus = self.runtime_manager.event_bus();
        event_bus.publish(GlobalEvent::SessionCreated(id), session.security_attributes.clone());
        for channel in &session.channels {
            event_bus.publish(GlobalEvent::ChannelCreated(id, channel.id), channel.security_attributes.clone());
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

    /// The session `id` for `context`: the running one, or loaded from the
    /// store and started. A running session is asked about like a stored
    /// one - `GetSession` with its attributes - so that it being in memory is
    /// no way around the policy (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; whatever loading or starting it reports otherwise.
    pub async fn open_session(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        match self.get_session(id) {
            Some(session_loop) => {
                self.may(context, ActionType::GetSession(&session_loop.get_session().security_attributes), id, StoreError::NotFound { entity: "session", id: id.to_string() }).await?;
                Ok(session_loop)
            }
            None => self.load_session(id, context).await,
        }
    }

    /// Asks the `SecurityManager` about `action` on session `id`, answering a
    /// refusal with `refusal`.
    async fn may(&self, context: &SecurityContext, action: ActionType<'_>, id: SessionId, refusal: StoreError) -> Result<()> {
        if self.runtime_manager.security_manager().check(context, action).await {
            return Ok(());
        }
        info!("Refused access to session {} for {}", id, context.initiator_id);
        Err(refusal.into())
    }

    /// Stop a running session's loop without deleting its durable definition.
    pub fn stop_session(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        if let Some((_, handle)) = self.handles.remove(&id) {
            handle.abort();
        }
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            let attributes = session_loop.get_session().security_attributes.clone();
            self.runtime_manager.event_bus().publish(GlobalEvent::SessionStopped(id), attributes);
        }
        removed
    }

    /// Stop and permanently delete a session together with all of its channels.
    ///
    /// Asked about before anything happens - a refused deletion must not
    /// stop the session on the way (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `ctx` may not
    /// read it, `StoreError::PermissionDenied` if they may read but not
    /// delete it.
    pub async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> Result<bool> {
        let session: Session = match self.get_session(id) {
            Some(session_loop) => (**session_loop.get_session()).clone(),
            None => self.runtime_manager.entity_store().get_session(ctx, id).await?,
        };
        let not_found = || StoreError::NotFound { entity: "session", id: id.to_string() };
        self.may(ctx, ActionType::GetSession(&session.security_attributes), id, not_found()).await?;
        self.may(ctx, ActionType::DeleteSession(&session.security_attributes), id, StoreError::PermissionDenied).await?;
        // Captured before the runtime loop is torn down.
        // The events carry the attributes as they were: once deleted, the
        // session is nowhere to look them up (issue 39).
        let channels: Vec<(ChannelId, SecurityAttributes)> = session.channels.iter()
            .map(|c| (c.id, c.security_attributes.clone()))
            .collect();

        self.stop_session(id);

        let event_bus = self.runtime_manager.event_bus();
        for (channel_id, attributes) in channels {
            event_bus.publish(GlobalEvent::ChannelDeleted(id, channel_id), attributes);
        }

        let removed = self.runtime_manager.entity_store().delete_session(ctx, id).await?;
        event_bus.publish(GlobalEvent::SessionDeleted(id), session.security_attributes.clone());
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::command::default_commands;
    use caelumora_model::channel::Channel;
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;
    use uuid::Uuid;

    fn test_runtime() -> Arc<RuntimeManager> {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        Arc::new(RuntimeManager::new(store.clone(), store, secret_manager, Arc::new(DummySecurityManager::new()), default_commands()).unwrap())
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
            commands: Default::default(),
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
    fn drain(rx: &mut crate::event::EventSubscription) -> Vec<GlobalEvent> {
        let mut out = Vec::new();
        while let Some(event) = rx.try_recv() {
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

    #[tokio::test]
    async fn a_session_has_the_global_commands_and_its_own_but_no_name_twice() {
        use caelumora_model::command::HELP_COMMAND_KIND;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let runtime = test_runtime();
        let names = |executor: crate::command::CommandExecutor| async move {
            executor.visible(&SecurityContext::default()).await.into_iter().map(|c| c.name).collect::<Vec<_>>()
        };

        let session = make_session("plain");
        let executor = runtime.new_command_executor(&session).await.expect("the global commands build");
        assert_eq!(names(executor).await, vec!["help", "wakeup"]);

        // A second /help from the session's own configuration - with the same
        // component id, which is fine across the two configurations.
        let mut session = make_session("clashing");
        session.commands = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                security_attributes: Default::default(),
                id: "help".to_string(),
                kind: HELP_COMMAND_KIND.to_string(),
                params: serde_json::Value::Null,
                deps: Vec::new(),
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let error = runtime.new_command_executor(&session).await.expect_err("/help twice");
        assert!(error.to_string().contains("/help"), "{error}");
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
        use caelumora_model::user::UserId;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands()).unwrap());
        let manager = SessionManager::new(runtime);

        let as_user = |id| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes { owner_id: ActorId::User(alice), contributors: vec![ActorId::User(bob)] };
        let id = session.id;
        manager.create_session(&as_user(alice), session).await.expect("alice creates her own session");

        let refused = manager.open_session(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(crate::error::is_not_found(&refused, "session"), "not refused - absent: {refused}");
        manager.open_session(&as_user(bob), id).await.expect("a contributor opens it");

        let refused = manager.delete_session(&as_user(bob), id).await.expect_err("a contributor may not delete it");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(manager.get_session(id).is_some(), "a refused deletion must not stop the session on the way");

        manager.delete_session(&as_user(alice), id).await.expect("the owner deletes it");
        assert!(manager.get_session(id).is_none());
    }
}
