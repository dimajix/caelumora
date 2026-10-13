//! `SessionManager`: starting, stopping, deleting, restoring and purging sessions.

use caelumora_model::store::query::{Filter, Query};
use caelumora_model::value::name::Name;
use crate::runtime::RuntimeManager;
use crate::event::EventBus;
use anyhow::Result;
use caelumora_model::channel::{Channel, ChannelField};
use caelumora_model::confidentiality::Classification;
use caelumora_model::event::GlobalEvent;
use caelumora_model::actor::ActorId;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes, SecurityContext, SecurityManager, Subject, JOIN};
use crate::view::{Viewer, Views};
use caelumora_model::store::{EntityStores, StoreError};
use tracing::{info, warn};
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::group::{Group, GroupId};
use crate::group::{keep_participants, purge_participants};
use caelumora_model::template::{ScopeTemplate, SessionTemplateId, WorkspaceTemplate};
use caelumora_model::workspace::WorkspaceChoice;
use crate::scoped::ScopedEntities;
use crate::workspace::WorkspaceManager;
use crate::agent::manager::AgentManager;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use super::{SessionError, SessionLoop, purge_channel_document, require_attachable, require_classification};
use caelumora_model::removal::Stored;
use caelumora_model::session;

pub struct SessionManager {
    sessions: DashMap<SessionId, Arc<SessionLoop>>,
    handles: DashMap<SessionId, JoinHandle<()>>,
    /// One lock per session id, held while that session is being started,
    /// so that a session starts only once however many requests ask for it
    /// at the same time (issue 62): two loops of one session would run two
    /// actors per agent and channel, which overwrite each other's state.
    starts: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    /// What builds the loops of the sessions it starts.
    pub(super) runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    security: Arc<dyn SecurityManager>,
    stores: EntityStores,
    events: Arc<EventBus>,
    workspaces: Arc<WorkspaceManager>,
    /// What makes the agents a template names.
    agents: Arc<AgentManager>,
    /// What goes with a session deleted, restored or purged (issue 178).
    scoped: Arc<ScopedEntities>,
}

/// A session as [`SessionManager::find`] found it for a context that may
/// read it: running or not, each asked about alike.
#[expect(clippy::large_enum_variant, reason = "a value returned once and taken apart at once")]
pub(crate) enum Found {
    /// It runs: its loop.
    Running(Arc<SessionLoop>),
    /// It does not run: its document, from the store.
    Stored(Session),
}

impl Found {
    /// The session as it is now.
    fn session(&self) -> Session {
        match self {
            Found::Running(session_loop) => (**session_loop.session()).clone(),
            Found::Stored(session) => session.clone(),
        }
    }
}

impl SessionManager {
    pub fn new(runtime_manager: Arc<RuntimeManager>) -> Self {
        Self {
            sessions: DashMap::new(),
            handles: DashMap::new(),
            starts: DashMap::new(),
            security: Arc::clone(runtime_manager.security_manager()),
            stores: runtime_manager.entity_stores().clone(),
            events: Arc::clone(runtime_manager.event_bus()),
            workspaces: Arc::clone(runtime_manager.workspace_manager()),
            agents: Arc::clone(runtime_manager.agent_manager()),
            scoped: Arc::clone(runtime_manager.scoped_entities()),
            runtime_manager,
        }
    }

    /// The lock held while session `id` is started.
    fn start_lock(&self, id: SessionId) -> Arc<tokio::sync::Mutex<()>> {
        self.starts.entry(id).or_default().clone()
    }

    /// Build the runtime loop for an (already-persisted) session, register it, spawn its
    /// run task, and announce that the session has started running. Called
    /// with the session's start lock held.
    async fn spawn_session(&self, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let attributes = session.security_attributes.clone();
        let session_loop = SessionLoop::new(session, channels, Arc::clone(&self.runtime_manager)).await?;
        self.sessions.insert(id, Arc::clone(&session_loop));

        let loop_for_task = Arc::clone(&session_loop);
        let handle = tokio::spawn(async move {
            if let Err(e) = loop_for_task.run().await {
                tracing::error!("Session loop for {} failed: {}", id, e);
            }
        });
        self.handles.insert(id, handle);

        self.events.publish(GlobalEvent::custom::<Session>(&id, session::STARTED, &()), attributes);
        Ok(session_loop)
    }

    /// Session `id` as `context` may read it: its loop if it runs, its
    /// document from the store if not. A running session is asked about
    /// like a stored one - `GetSession` with its attributes - so that it
    /// being in memory is no way around the policy (issue 11).
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context`
    /// may not read it, alike whether it runs.
    pub(crate) async fn find(&self, context: &SecurityContext, id: SessionId) -> Result<Found> {
        let running = self.sessions.get(&id).map(|entry| entry.clone());
        match running {
            Some(session_loop) => {
                let attributes = session_loop.session().security_attributes.clone();
                self.security
                    .require(context, ActionType::get(Session::KIND, &attributes), |_| SessionError::NotFound(id)).await?;
                Ok(Found::Running(session_loop))
            }
            None => Ok(Found::Stored(self.stores.sessions.get(context, id).await.map_err(store_error(id))?)),
        }
    }

    /// Creates a session from the template `template_id` on behalf of
    /// `context`, whose user owns it and is the template's "user", names it
    /// `name` - the template's name if `None` - and starts it (issue 88).
    /// Its workspace is `workspace` where one is chosen (issue 266) - none,
    /// one of the user's own, or one made for it alone -, else the
    /// template's. Once the session is made, the template's agents are found
    /// by name in their scope, or made, and taken into it and their
    /// channels, as `context`; if that fails, the session is taken back,
    /// with what was made in its scope.
    ///
    /// # Errors
    /// `SessionError::OwnerNotAUser` if `context` acts for no user; a
    /// `StoreError` if the template, a user or a personality it names does
    /// not exist or may not be read; a `ValueError` if `name` is no name; a
    /// `TemplateError` if the template cannot be made into a session;
    /// what attaching an existing workspace or making one reports;
    /// whatever creating the session reports.
    pub async fn create_from_template(&self, context: &SecurityContext, template_id: SessionTemplateId, name: Option<&str>, workspace: Option<WorkspaceChoice>) -> Result<Arc<SessionLoop>> {
        let owner = context.principal_id.as_user_id().ok_or(SessionError::OwnerNotAUser(context.principal_id))?;
        // Before anything is found or made for it.
        let name = Name::optional(name)?;
        let template = self.stores.session_templates.get(context, template_id).await.map_err(|e| match e {
            StoreError::NotFound { .. } => SessionError::TemplateNotFound(template_id).into(),
            e => anyhow::Error::new(e),
        })?;
        let id = SessionId::new();
        // Its own verb before anything of others is asked or anything is
        // found or made for it (issue 312): who may not create the session
        // learns nothing of the agents, the workspace or anyone's clearance.
        self.security.require(context, ActionType::create(Session::KIND, &Session::attributes_of(id, owner, &template.access)), StoreError::PermissionDenied).await?;
        // One of the user's chosen is asked about before anything is made.
        let existing = match &workspace {
            Some(WorkspaceChoice::Existing { id: chosen, share }) => {
                require_attachable(self.runtime_manager.workspace_manager(), self.security.as_ref(), context, *chosen, id).await?;
                Some((*chosen, *share))
            }
            _ => None,
        };
        let (participants, agents) = crate::template::resolve_participants(&self.stores, context, &template, id, owner).await?;
        let name = name.unwrap_or_else(|| template.name.name.clone());
        // What is to be found or made: the template's, unless one was chosen.
        let wanted = match workspace {
            None => template.workspace.clone(),
            Some(WorkspaceChoice::New { storage, classification }) =>
                Some(WorkspaceTemplate { name: name.clone(), scope: ScopeTemplate::SessionScope, storage, classification, description: None }),
            Some(WorkspaceChoice::None | WorkspaceChoice::Existing { .. }) => None,
        };
        let (mut session, channels) = template.build(id, owner, name, participants)?;
        let workspaces = self.runtime_manager.workspace_manager();
        let workspace = match &wanted {
            Some(wanted) => Some(crate::template::resolve_workspace(workspaces, context, wanted, id, owner).await?),
            None => existing.map(|(chosen, _)| (chosen, false)),
        };
        session.workspace = workspace.as_ref().map(|(found, _)| *found);
        let established = async {
            let session_loop = self.establish(context, session, channels).await?;
            if let Err(e) = crate::template::take_in_agents(self.stores.agents.as_ref(), &self.agents, context, &template, &session_loop, agents).await {
                self.take_back(id).await;
                return Err(e);
            }
            Ok(session_loop)
        }.await;
        match established {
            Ok(session_loop) => {
                // Shared once the session it is shared with exists (issue 23).
                if let Some((chosen, Some(grant))) = existing {
                    workspaces.share(context, chosen, Subject::Group(GroupId::of_session(id)), grant).await?;
                }
                Ok(session_loop)
            }
            Err(e) => {
                // One made for this session goes with what failed.
                if let Some((made, true)) = workspace {
                    let system = SecurityContext::system();
                    if let Err(cleanup) = async { workspaces.delete(&system, made).await?; workspaces.purge(&system, made).await }.await {
                        warn!(workspace = %made, "The workspace made for a session that could not be created stays: {cleanup}");
                    }
                }
                Err(e)
            }
        }
    }

    /// Takes back the session `id`, just made and not yet complete, in the
    /// runtime's name - making it was permitted: deleted and purged, with
    /// what was made in its scope.
    async fn take_back(&self, id: SessionId) {
        let system = SecurityContext::system();
        if let Err(undo) = async { self.delete_session(&system, id).await?; self.purge_session(&system, id).await }.await {
            warn!(session = %id, "The session that could not be completed stays: {undo}");
        }
    }

    /// Creates a brand-new session with its `channels`, as a template
    /// would: asks to create it (`CreateSession`) before anything else.
    #[cfg(test)]
    pub(crate) async fn create_session(&self, context: &SecurityContext, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        self.security.require(context, ActionType::create(Session::KIND, &session.security_attributes), StoreError::PermissionDenied).await?;
        self.establish(context, session, channels).await
    }

    /// Establishes the brand-new `session` with its `channels`, whose
    /// creation `context` was asked for (`CreateSession`) before anything
    /// was found or made for it: persists them - the channels as documents
    /// of their own (issue 70) -, announces their existence, then starts
    /// the session. The classifications are validated and every
    /// participant's admission asked first; the store asks to create the
    /// session again and each channel as it stores them.
    async fn establish(&self, context: &SecurityContext, session: Session, channels: Vec<Channel>) -> Result<Arc<SessionLoop>> {
        let id = session.id;
        let start = self.start_lock(id);
        let _starting = start.lock().await;

        for channel in &channels {
            require_classification(&self.runtime_manager, &channel.classification)?;
        }
        for channel in &channels {
            crate::channel::require_admitted(&self.runtime_manager, context, channel).await?;
        }
        self.stores.sessions.create(context, &session).await?;
        // Each with the group of its participants (issue 286), which it is
        // read by.
        let made = async {
            keep_participants(self.stores.groups.as_ref(), Group::of_session(&session)).await?;
            for channel in &channels {
                self.stores.channels.create(context, channel).await?;
                keep_participants(self.stores.groups.as_ref(), Group::of_channel(channel)).await?;
            }
            Ok::<_, StoreError>(())
        }.await;
        if let Err(error) = made {
            // Nothing half-made stays behind; the session's creation was
            // permitted, so it is taken back in the runtime's name - what
            // was not made yet is not found, which is no matter.
            let system = SecurityContext::system();
            for channel in &channels {
                let _ = purge_channel_document(&self.stores, channel.id).await;
            }
            let _ = purge_participants(self.stores.groups.as_ref(), GroupId::of_session(id)).await;
            if self.stores.sessions.delete(&system, id).await.is_ok() {
                let _ = self.stores.sessions.purge(&system, id).await;
            }
            return Err(error.into());
        }

        let event_bus = &self.events;
        event_bus.publish(GlobalEvent::created::<Session>(&id), session.security_attributes.clone());
        for channel in &channels {
            event_bus.publish_classified(GlobalEvent::ChannelCreated(id, channel.id), channel.security_attributes.clone(), std::slice::from_ref(&channel.classification));
        }

        self.spawn_session(session, channels).await
    }

    /// The channels `session` lists, loaded in the runtime's name: starting a
    /// session the caller may read follows from an action already permitted,
    /// and it must not fail because the caller is not in one of its channels.
    async fn load_channels(&self, session: &Session) -> Result<Vec<Channel>> {
        let system = SecurityContext::system();
        let mut channels = Vec::with_capacity(session.channels.len());
        for id in &session.channels {
            channels.push(self.stores.channels.get(&system, *id).await?);
        }
        Ok(channels)
    }

    /// The session `id` for `context`: the running one, or loaded from the
    /// store and started - which announces it as `session::STARTED`, not
    /// as created, as the session exists already.
    ///
    /// Started once, however many ask at the same time (issue 62): whoever
    /// comes second waits for the first and gets the session it started -
    /// if they may read it.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it; whatever loading or starting it reports otherwise.
    pub async fn open_session(&self, context: &SecurityContext, id: SessionId) -> Result<Arc<SessionLoop>> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        match self.find(context, id).await? {
            Found::Running(session_loop) => Ok(session_loop),
            Found::Stored(session) => {
                let channels = self.load_channels(&session).await?;
                self.spawn_session(session, channels).await
            }
        }
    }

    /// Takes `participant` - whom `context` acts for: themselves, or their
    /// own agent - into session `id` by their own act, and with `into` into
    /// that channel (issue 221, `SessionLoop::join`). The session is opened
    /// in the runtime's name if an entry of its access list lets `context`
    /// join it, or they may read it: who may join reads nothing of it before
    /// they did.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such session or `context` may
    /// neither join nor read it; as `SessionLoop::join` otherwise.
    pub async fn join(&self, context: &SecurityContext, id: SessionId, participant: ActorId, into: Option<ChannelId>) -> Result<Arc<SessionLoop>> {
        let not_found = || SessionError::NotFound(id);
        let system = SecurityContext::system();
        let attributes = self.find(&system, id).await.map_err(|_| not_found())?.session().security_attributes;
        let reachable = self.security.check(context, ActionType::custom(Session::KIND, &JOIN, &attributes)).await
            || self.security.may_read_session(context, &attributes).await;
        if !reachable {
            return Err(not_found().into());
        }
        let session_loop = self.open_session(&system, id).await?;
        session_loop.join(context, participant, into).await?;
        Ok(session_loop)
    }

    /// Stops session `id` on behalf of `context` without deleting it: its
    /// channels and actors stop, for everybody in it (`StopSession`).
    /// Returns whether it ran.
    ///
    /// Waits for a start of the same session that is under way (issue 62):
    /// otherwise that start could register the session after it was stopped,
    /// and it would run on.
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may read but not
    /// stop it - whether it runs or not.
    pub async fn stop_session(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let found = self.find(context, id).await?;
        let attributes = found.session().security_attributes;
        self.security
            .require(context, ActionType::StopSession(&attributes), StoreError::PermissionDenied).await?;
        Ok(matches!(found, Found::Running(_)) && self.stop_running(id).await.is_some())
    }

    /// Stops session `id` if it runs, and with it the commands started in
    /// its channels (issue 254). Called with its start lock held.
    async fn stop_running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        if let Some((_, handle)) = self.handles.remove(&id) {
            handle.abort();
        }
        let removed = self.sessions.remove(&id).map(|(_, s)| s);
        if let Some(session_loop) = &removed {
            let session = Arc::clone(&session_loop.session());
            self.workspaces.end_commands_in(&session.channels).await;
            self.events.publish(GlobalEvent::custom::<Session>(&id, session::STOPPED, &()), session.security_attributes.clone());
        }
        removed
    }

    /// Stops and deletes a session. Deleting marks (issue 164): the session
    /// is kept, with its channels and their state, hidden until it is
    /// restored (`restore_session`) or purged (`purge_session`). What lies
    /// in its scope is deleted with it, in `context`'s name (issue 320).
    ///
    /// Asked about before anything happens - a refused deletion must not
    /// stop the session on the way (issue 11).
    ///
    /// # Errors
    /// `StoreError::NotFound` if there is no such session or `context` may
    /// not read it, `StoreError::PermissionDenied` if they may read but not
    /// delete it, `SessionError::ScopeNotEmpty` if something of its scope
    /// is left that they may not delete - what they may is deleted, the
    /// session is kept; whatever else the store reports, which keeps the
    /// session too.
    pub async fn delete_session(&self, context: &SecurityContext, id: SessionId) -> Result<bool> {
        // Held until the session is gone from the store, so that no request
        // starts it again from there in the meantime (issue 62).
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let session = self.find(context, id).await?.session();
        self.security
            .require(context, ActionType::delete(Session::KIND, &session.security_attributes), StoreError::PermissionDenied).await?;
        // Captured before the runtime loop is torn down.
        // The events carry the attributes as they were: once deleted, the
        // session is nowhere to look them up (issue 39).
        let channels: Vec<(ChannelId, SecurityAttributes, Classification)> = self.load_channels(&session).await?.into_iter()
            .map(|c| (c.id, c.security_attributes, c.classification))
            .collect();

        // Marked, not removed (issue 164): its channels and their state stay
        // as they are, hidden with it - a channel is reached only through
        // its session - until it is purged. Marked before what lies in its
        // scope, so that all of that is deleted with it, not before it: it
        // comes back with it (`restore_session`, issue 178).
        if !self.stores.sessions.delete(context, id).await? {
            return Ok(false);
        }
        // What lies in its scope and in its channels' goes as far as
        // `context` may delete it (issue 320). If something is left, the
        // mark is taken away again - in the runtime's name: the deletion was
        // permitted - and the session runs on if it ran, without what was
        // deleted, and `context` is told what is left. Its agents take part
        // in no other session (`Scope::admits`): they leave nothing.
        if let Err(left) = self.scoped.delete_lying_in(context, &Scope::SessionScope { session_id: id }).await {
            if let Err(undo) = self.stores.sessions.restore(&SecurityContext::system(), id).await {
                warn!("Session {} could not be deleted and not be unmarked again: {}", id, undo);
            }
            return Err(left);
        }

        self.stop_running(id).await;

        let event_bus = &self.events;
        for (channel_id, attributes, classification) in channels {
            event_bus.publish_classified(GlobalEvent::ChannelDeleted(id, channel_id), attributes, &[classification]);
        }
        event_bus.publish(GlobalEvent::deleted::<Session>(&id), session.security_attributes.clone());
        self.starts.remove(&id);
        Ok(true)
    }

    /// Restores the deleted session `id` on behalf of `context` (issue 164):
    /// its mark taken away, with the channels it listed when it was
    /// deleted. It is not started; opening it starts it, as any stored
    /// session.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session or
    /// `context` may not see it (`ReadDeleted`, asked first, issue 304),
    /// `StoreError::PermissionDenied` if they may but not restore it.
    pub async fn restore_session(&self, context: &SecurityContext, id: SessionId) -> Result<Session> {
        // The lock of deleting and purging it: a purge never meets it half
        // restored.
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        // When it was deleted, read before the mark goes - as `context`, so
        // that whoever may not see it learns nothing more: what was deleted
        // with it comes back with it (issue 178).
        let deleted_at = self.stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?.deleted.map(|removal| removal.at);
        let session = self.stores.sessions.restore(context, id).await.map_err(store_error(id))?;
        if let Some(since) = deleted_at {
            if let Err(e) = self.scoped.restore_lying_in(context, &Scope::SessionScope { session_id: id }, since).await {
                warn!("Session {} was restored, but not what lies in its scope: {}", id, e);
            }
        }
        self.events.publish(GlobalEvent::restored::<Session>(&id), session.security_attributes.clone());
        info!("Session {} restored by {}", id, context.caller_id);
        Ok(session)
    }

    /// Removes the deleted session `id` for good on behalf of `context`
    /// (issue 164): its document, and - in the runtime's name, since the
    /// purge was permitted - the documents and the state of its channels,
    /// those it listed and those deleted from it before. Only what was
    /// deleted is purged; the store asks `Purge`. Under the lock of
    /// deleting and restoring it, so that what is restored meanwhile is not
    /// purged.
    ///
    /// # Errors
    /// `SessionError::NotFound` if there is no such deleted session or
    /// `context` may not see it, `StoreError::PermissionDenied` if they may
    /// but not purge it.
    pub async fn purge_session(&self, context: &SecurityContext, id: SessionId) -> Result<()> {
        let start = self.start_lock(id);
        let _starting = start.lock().await;
        let session = self.stores.sessions.get_deleted(context, id).await.map_err(store_error(id))?;
        if !self.stores.sessions.purge(context, id).await? {
            return Err(SessionError::NotFound(id).into());
        }
        let mut channels = session.channels.clone();
        let of_it = Query::filtered(Filter::eq(ChannelField::Session, id));
        channels.extend(self.stores.channels.list_deleted(&SecurityContext::system(), &of_it).await?.items.into_iter().map(|c| c.id));
        // The state lives in the `StateStore`, which may be another store
        // than the session's.
        let state_store = self.runtime_manager.state_store();
        for channel_id in channels {
            purge_channel_document(&self.stores, channel_id).await?;
            state_store.delete_channel_state(channel_id).await?;
        }
        // And what lay in its scope and in its channels' (issue 178).
        self.scoped.purge_lying_in(&Scope::SessionScope { session_id: id }).await?;
        self.starts.remove(&id);
        info!("Session {} purged by {}", id, context.caller_id);
        Ok(())
    }

    /// The root of the views (issue 163): every session `viewer` may
    /// read, and from there their channels and members, running or not.
    pub fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::new(&self.runtime_manager, Arc::<SessionManager>::clone(self), viewer)
    }

    /// The loop of session `id`, if it runs.
    pub(crate) fn running(&self, id: SessionId) -> Option<Arc<SessionLoop>> {
        self.sessions.get(&id).map(|entry| entry.clone())
    }
}

/// `error` of the store about session `id`: that there is none - or none
/// the caller may see - is the session's own error, what the request named
/// and did not find (issue 323).
fn store_error(id: SessionId) -> impl FnOnce(StoreError) -> anyhow::Error {
    move |error| match error {
        StoreError::NotFound { .. } => SessionError::NotFound(id).into(),
        error => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::value::name::Name;
    use crate::runtime::RuntimeManager;
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::Agent;
    use crate::agent::manager::AgentError;
    use caelumora_model::channel::Channel;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::value::language::Language;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::{SecurityAttributes, SecurityContext};
    use crate::view::Viewer;
    use caelumora_model::store::StoreError;
    use caelumora_model::scope::Scope;
    use std::sync::Arc;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use crate::session::{SessionError, SessionManager};
    use crate::session::testing::*;
    use caelumora_model::confidentiality::ConfidentialityRules;
    use caelumora_model::config::AppConfig;
    use crate::testing::test_registry;
    use caelumora_model::event::EntityChange;
    use caelumora_model::session::Session;
    use caelumora_model::session;

    #[tokio::test]
    async fn create_session_emits_created_then_started() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let mut rx = runtime.event_bus().subscribe_all();
        let ctx = SecurityContext::default();

        let session = make_session("s1", ActorId::None);
        let session_id = session.id;
        let channel_id = session.channels[0];

        create(&manager, &ctx, session).await.unwrap();

        // The manager emits these synchronously, in order, before returning.
        let events = drain(&mut rx);
        let created = events.iter().position(|e| e.about::<Session>() == Some((session_id, &EntityChange::Created)));
        let ch_created = events.iter().position(|e| matches!(e, GlobalEvent::ChannelCreated(sid, cid) if *sid == session_id && *cid == channel_id));
        let started = events.iter().position(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id));

        assert!(created.is_some(), "expected SessionCreated, got {:?}", events);
        assert!(ch_created.is_some(), "expected ChannelCreated, got {:?}", events);
        assert!(started.is_some(), "expected SessionStarted, got {:?}", events);
        assert!(created < ch_created && ch_created < started, "wrong order: {:?}", events);

        // It is now persisted and running.
        assert!(runtime.entity_stores().sessions.get(&ctx, session_id).await.is_ok());
        assert!(manager.sessions.contains_key(&session_id));

        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_emits_stopped_then_deleted() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();

        let session = make_session("s2", ActorId::None);
        let session_id = session.id;
        let channel_id = session.channels[0];
        create(&manager, &ctx, session).await.unwrap();

        // Subscribe after creation so we only observe the deletion events.
        let mut rx = runtime.event_bus().subscribe_all();
        assert!(manager.delete_session(&ctx, session_id).await.unwrap());

        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STOPPED, .. })) if id == session_id));
        let ch_deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(sid, cid) if *sid == session_id && *cid == channel_id));
        let deleted = events.iter().position(|e| e.about::<Session>() == Some((session_id, &EntityChange::Deleted)));

        assert!(stopped.is_some(), "expected SessionStopped, got {:?}", events);
        assert!(ch_deleted.is_some(), "expected ChannelDeleted, got {:?}", events);
        assert!(deleted.is_some(), "expected SessionDeleted, got {:?}", events);
        assert!(stopped < ch_deleted && ch_deleted < deleted, "wrong order: {:?}", events);

        assert!(runtime.entity_stores().sessions.get(&ctx, session_id).await.is_err());
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn opening_a_stored_session_emits_started_but_not_created() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();

        // Persist a session directly, without going through create_session.
        let session = make_session("s3", ActorId::None);
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        let mut rx = runtime.event_bus().subscribe_all();
        manager.open_session(&ctx, session_id).await.unwrap();

        let events = drain(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id)),
            "expected SessionStarted, got {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e.about::<Session>(), Some((_, EntityChange::Created)))),
            "load must not emit SessionCreated, got {:?}", events
        );

        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_asked_for_at_the_same_time_starts_once() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();
        let session = make_session("concurrent", ActorId::None);
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        let mut rx = runtime.event_bus().subscribe_all();
        let (first, second, third) = tokio::join!(
            manager.open_session(&ctx, session_id),
            manager.open_session(&ctx, session_id),
            manager.open_session(&ctx, session_id),
        );
        let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
        assert!(Arc::ptr_eq(&first, &second) && Arc::ptr_eq(&first, &third), "one loop for all of them");

        let started = drain(&mut rx).into_iter().filter(|e| matches!(e.about::<Session>(), Some((id, EntityChange::Custom { name: session::STARTED, .. })) if id == session_id)).count();
        assert_eq!(started, 1, "the session started once");
        manager.stop_session(&SecurityContext::system(), session_id).await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_waits_for_a_start_under_way_and_then_stops_what_it_started() {
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let ctx = SecurityContext::default();
        let session = make_session("stopped", ActorId::None);
        let session_id = session.id;
        persist(&runtime, &ctx, &session).await;

        // A start under way holds the lock ...
        let start = manager.start_lock(session_id);
        let starting = start.lock().await;
        let stopping = tokio::spawn({
            let manager = Arc::clone(&manager);
            async move { manager.stop_session(&SecurityContext::system(), session_id).await.unwrap() }
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!stopping.is_finished(), "the stop waits for the start");

        // ... and registers the session before it lets go: the stop then
        // finds it, and nothing keeps running.
        let channels = vec![main_channel(&session)];
        manager.spawn_session(session, channels).await.unwrap();
        drop(starting);
        assert!(stopping.await.unwrap(), "the stop stopped what the start started");
        assert!(!manager.sessions.contains_key(&session_id));
    }

    #[tokio::test]
    async fn a_session_has_the_global_commands_and_its_own_but_no_name_twice() {
        use caelumora_model::command::HELP_COMMAND_KIND;
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        let runtime = test_runtime();
        let names = |executor: crate::command::CommandExecutor<dyn crate::command::ChannelCommandProvider>| async move {
            executor.visible(&SecurityContext::default(), &SecurityAttributes::default()).await.into_iter().map(|c| c.name).collect::<Vec<_>>()
        };

        let session = make_session("plain", ActorId::None);
        let executor = runtime.new_command_executor(&session).await.expect("the global commands build");
        assert_eq!(names(executor).await, vec!["help", "nudge", "wakeup"]);

        // A second /help from the session's own configuration - with the same
        // component id, which is fine across the two configurations.
        let mut session = make_session("clashing", ActorId::None);
        session.commands = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                id: "help".to_string(),
                kind: HELP_COMMAND_KIND.to_string(),
                params: serde_json::Value::Null,
                tags: Vec::new(),
                fail_policy: Default::default(),
            }],
        };
        let error = runtime.new_command_executor(&session).await.expect_err("/help twice");
        assert!(error.to_string().contains("/help"), "{error}");
    }

    #[tokio::test]
    async fn a_deleted_agent_rests_in_its_channels_keeps_its_name_and_comes_back_restored() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::Participant;
        use caelumora_model::scope::Scope;
        use std::time::Duration;
        /// Waits until `holds` does, at most two seconds: a running channel
        /// follows the agent as the session learns of it.
        async fn until(holds: impl Fn() -> bool, what: &str) {
            let waited = async { while !holds() { tokio::time::sleep(Duration::from_millis(10)).await } };
            tokio::time::timeout(Duration::from_secs(2), waited).await.unwrap_or_else(|_| panic!("{what}"));
        }
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let agents = runtime.agent_manager();
        let sessions = &runtime.entity_stores().sessions;
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Scope::GlobalScope).unwrap(), kind: Default::default(),
            roles: None, description: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        runtime.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let helper = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        runtime.entity_stores().agents.create(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);

        // In a running session's channel, and in a stopped one's.
        let mut running = make_session("running", ActorId::None);
        running.participants = vec![Participant::Agent(helper.id)];
        let running = create(&manager, &ctx, running).await.unwrap();
        let talk = running.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();
        let mut stopped = make_session("stopped", ActorId::None);
        stopped.participants = vec![Participant::Agent(helper.id)];
        sessions.create(&system, &stopped).await.unwrap();
        runtime.entity_stores().channels.create(&system, &Channel { participants: vec![Participant::Agent(helper.id)], ..main_channel(&stopped) }).await.unwrap();

        assert!(agents.delete(&ctx, helper.id).await.expect("deleted"));
        until(|| !talk.actor_ids().contains(&actor), "its actor stops in the running channel").await;
        let rests = |channel: &Channel| channel.participants.iter().any(|p| p.actor_id() == actor) && !channel.former.iter().any(|f| f.participant.actor_id() == actor);
        assert!(rests(&talk.channel()), "it rests in the running channel");
        assert!(rests(&runtime.entity_stores().channels.get(&system, stopped.channels[0]).await.unwrap()), "and in the stopped one");
        assert!(runtime.entity_stores().agents.get(&system, helper.id).await.is_err(), "it is not there");
        let views = manager.views(Viewer::new(ctx.clone()));
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let former = reached.member(actor).await.unwrap();
        assert!(former.former() && former.name() == Some("helper"), "what it said keeps its sender");
        assert_eq!(former.left().map(|left| left.by), Some(ctx.caller_id), "left as its mark says");
        assert!(!reached.members().await.iter().any(|m| m.id() == actor), "no member");
        assert!(reached.former_members().await.iter().any(|m| m.id() == actor), "but a former one");

        // A session that starts meanwhile leaves it out.
        let opened = manager.open_session(&ctx, stopped.id).await.unwrap();
        let main = opened.channel_by_id(stopped.channels[0]).unwrap();
        assert!(!main.actor_ids().contains(&actor), "not run where it rests");

        agents.restore(&ctx, helper.id).await.expect("restored");
        until(|| talk.actor_ids().contains(&actor), "it runs again in the running channel").await;
        until(|| main.actor_ids().contains(&actor), "and in the one started meanwhile").await;
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        assert!(!reached.member(actor).await.unwrap().former(), "a member again");
        assert!(agents.restore(&ctx, helper.id).await.is_err(), "not deleted");
        assert!(!agents.purge(&ctx, helper.id).await.unwrap(), "only what was deleted is purged");

        // Removed from a channel while it rests: restored, it is not back there.
        assert!(agents.delete(&ctx, helper.id).await.unwrap());
        until(|| !main.actor_ids().contains(&actor), "its actor stops").await;
        main.remove_participant(&ctx, actor).await.expect("removed while it rests");
        assert!(main.channel().former.iter().any(|f| f.participant.actor_id() == actor), "a former participant there");

        // Its name taken meanwhile: not restored.
        let again = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        runtime.entity_stores().agents.create(&system, &again).await.unwrap();
        let taken = agents.restore(&ctx, helper.id).await.expect_err("its name is taken");
        assert!(matches!(taken, AgentError::Store(StoreError::NameTaken { entity: "agent" })), "{taken}");

        // Purged: a former member still, not knowing when it left.
        assert!(agents.purge(&ctx, helper.id).await.expect("purged"));
        assert!(runtime.entity_stores().agents.get_deleted(&system, helper.id).await.is_err(), "nothing left to restore");
        let reached = views.channel_of(running.id(), talk.id()).await.unwrap().there().unwrap();
        let purged = reached.member(actor).await.unwrap();
        assert!(purged.former() && purged.left().is_none(), "former, without when");
        assert!(!agents.delete(&ctx, helper.id).await.unwrap(), "nothing to delete");
        manager.stop_session(&system, running.id()).await.unwrap();
        manager.stop_session(&system, opened.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_that_missed_an_agent_being_deleted_finds_out_as_it_looks_again() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::channel::Participant;
        use caelumora_model::scope::Scope;
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let (system, ctx) = (SecurityContext::system(), SecurityContext::default());
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Scope::GlobalScope).unwrap(), kind: Default::default(),
            roles: None, description: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        runtime.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let helper = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        runtime.entity_stores().agents.create(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);
        let mut session = make_session("running", ActorId::None);
        session.participants = vec![Participant::Agent(helper.id)];
        let session = create(&manager, &ctx, session).await.unwrap();
        let talk = session.create_channel(&ctx, spec("talk", vec![actor])).await.unwrap();

        // Deleted with nothing announced, as if the session lost the event.
        assert!(talk.actor_ids().contains(&actor));
        let mut rx = runtime.event_bus().subscribe_all();
        runtime.entity_stores().agents.delete(&system, helper.id).await.unwrap();
        session.follow_agents(None).await;
        assert!(!talk.actor_ids().contains(&actor), "looking again, it stops the actor");
        let announced = drain(&mut rx);
        assert!(announced.iter().any(|e| matches!(e, GlobalEvent::AgentChannelLeft(s, c, a) if *s == session.id() && *c == talk.id() && *a == helper.id)), "as a leave: {announced:?}");

        runtime.entity_stores().agents.restore(&system, helper.id).await.unwrap();
        session.follow_agents(None).await;
        assert!(talk.actor_ids().contains(&actor), "and starts it again");
        let announced = drain(&mut rx);
        assert!(announced.iter().any(|e| matches!(e, GlobalEvent::AgentChannelJoined(_, c, a) if *c == talk.id() && *a == helper.id)), "as a join: {announced:?}");
        manager.stop_session(&system, session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_is_created_from_a_template_for_its_owner() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::approval::ApprovalStrategy;
        use caelumora_model::channel::Participant;
        use caelumora_model::template::{ChannelTemplate, ParticipantTemplate, SessionTemplate, SessionTemplateId};
        use caelumora_model::user::{User, UserId};

        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let owner = UserId::new();
        runtime.entity_stores().users.create(&SecurityContext::system(), &User { id: owner, username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None }).await.unwrap();
        let template = SessionTemplate {
            access: Vec::new(),
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            id: SessionTemplateId::new(),
            name: ScopedName::new("pairing", Default::default()).unwrap(),
            description: None,
            participants: vec![
                ParticipantTemplate::User { name: "user".to_string() },
                ParticipantTemplate::Approver { name: "approval".to_string(), strategy: ApprovalStrategy::All },
            ],
            channels: vec![ChannelTemplate {
                access: Vec::new(),
                summary: None, briefing: None, language: None,
                approval_rules: Vec::new(),
                uses_workspace: true,
                name: Name::new("main").unwrap(),
                components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
                classification: Classification::CHANNEL_DEFAULT,
                participants: vec!["user".to_string(), "approval".to_string()],
            }],
            security_attributes: Default::default(),
            commands: Default::default(),
            privacy: Default::default(),
            deleted: None,
        };
        runtime.entity_stores().session_templates.create(&SecurityContext::system(), &template).await.unwrap();
        let hers = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
        let mut profile = caelumora_model::user::UserProfile::new(owner, None);
        profile.preferred_language = Some(Language::new("de").unwrap());
        runtime.entity_stores().user_profiles.create(&hers, &profile).await.unwrap();

        let session = manager.create_from_template(&hers, template.id, None, None).await.expect("created and started");
        assert_eq!(session.name(), "pairing", "named after the template unless named");
        assert_eq!(session.session().security_attributes.owner_id, ActorId::User(owner));
        let main = session.channel_by_name("main").expect("the template's channel runs");
        assert!(main.user_actor(owner).is_some(), "the owner is the template's \"user\"");
        assert_eq!(main.channel().language, None, "not in the one the owner prefers: that is the frontend's to set (issue 331)");
        let stored = runtime.entity_stores().channels.get(&hers, main.id()).await.unwrap();
        assert!(stored.participants.iter().any(|p| matches!(p, Participant::Approver(a) if a.strategy == ApprovalStrategy::All)), "the template's approver");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();

        let missing = manager.create_from_template(&hers, SessionTemplateId::new(), None, None).await.expect_err("no such template");
        assert!(matches!(missing.downcast_ref::<SessionError>(), Some(SessionError::TemplateNotFound(_))), "{missing}");
        let not_a_user = manager.create_from_template(&SecurityContext::system(), template.id, None, None).await.expect_err("the system owns no session");
        assert!(matches!(not_a_user.downcast_ref::<SessionError>(), Some(SessionError::OwnerNotAUser(_))), "{not_a_user}");
    }

    #[tokio::test]
    async fn a_deleted_session_keeps_its_state_until_it_is_purged_and_can_be_restored() {
        use caelumora_model::store::StateStore;
        let entities = Arc::new(MemoryStore::new());
        let states = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&states), Arc::<MemoryStore>::clone(&entities), secret_manager, Arc::new(DummySecurityManager::new()), Arc::new(ConfidentialityRules::default()), &AppConfig::default(), test_registry(Arc::<MemoryStore>::clone(&entities))).unwrap());
        let manager = SessionManager::new(runtime);
        let ctx = SecurityContext::default();
        let session = make_session("two-stores", ActorId::None);
        let (id, channel_id) = (session.id, session.channels[0]);
        create(&manager, &ctx, session).await.unwrap();
        assert!(states.get_channel_state(channel_id).await.is_ok(), "the channel keeps its state in the state store");

        manager.delete_session(&ctx, id).await.unwrap();
        assert!(entities.entity_stores().sessions.get(&ctx, id).await.is_err(), "not there");
        assert!(states.get_channel_state(channel_id).await.is_ok(), "but kept, with its channels' state (issue 164)");
        manager.restore_session(&ctx, id).await.expect("restored");
        manager.open_session(&ctx, id).await.expect("and opened as any stored session");
        assert!(manager.restore_session(&ctx, id).await.is_err(), "not deleted");
        assert!(manager.purge_session(&ctx, id).await.is_err(), "only what was deleted is purged");

        // A channel deleted from it before goes with it, too.
        let side = manager.open_session(&ctx, id).await.unwrap().create_channel(&ctx, spec("side", vec![])).await.unwrap().id();
        manager.open_session(&ctx, id).await.unwrap().delete_channel(&ctx, side).await.unwrap();
        assert!(states.get_channel_state(side).await.is_ok(), "kept with the deleted channel");
        manager.delete_session(&ctx, id).await.unwrap();
        manager.purge_session(&ctx, id).await.expect("purged");
        assert!(entities.entity_stores().sessions.get_deleted(&ctx, id).await.is_err(), "nothing left to restore");
        assert!(states.get_channel_state(channel_id).await.is_err(), "deleted from the state store, not only from the entity store");
        assert!(entities.entity_stores().channels.get(&ctx, channel_id).await.is_err(), "its channels go with it (issue 70)");
        assert!(entities.entity_stores().channels.get_deleted(&ctx, side).await.is_err(), "also those deleted from it before");
        assert!(states.get_channel_state(side).await.is_err(), "with their state");
    }

    #[tokio::test]
    async fn what_lies_in_a_scope_goes_with_its_session_or_channel() {
        use caelumora_model::agent::{Personality, PersonalityId};
        use caelumora_model::user::UserId;

        let runtime = test_runtime();
        let personalities = Arc::clone(&runtime.entity_stores().personalities);
        let agents = Arc::clone(&runtime.entity_stores().agents);
        let manager = SessionManager::new(runtime);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session = make_session("scoped", ActorId::None);
        let id = session.id;
        let running = create(&manager, &ctx, session).await.unwrap();
        let side = running.create_channel(&ctx, spec("side", vec![])).await.unwrap().id();

        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("planner", Scope::SessionScope { session_id: id }).unwrap(), kind: Default::default(),
            roles: None, description: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        personalities.create(&system, &personality).await.unwrap();
        let agent_in = |name: &str, scope| Agent::new(ScopedName::new(name, scope).unwrap(), personality.id, ActorId::None);
        let sessions = agent_in("helper", Scope::SessionScope { session_id: id });
        let sides = agent_in("aside", Scope::ChannelScope { session_id: id, channel_id: side });
        let users = agent_in("own", Scope::UserScope { user_id: UserId::new() });
        let earlier = agent_in("earlier", Scope::SessionScope { session_id: id });
        for agent in [&sessions, &sides, &users, &earlier] {
            agents.create(&system, agent).await.unwrap();
        }
        agents.delete(&system, earlier.id).await.unwrap();
        let present = |agent: &Agent| {
            let (agents, id) = (Arc::clone(&agents), agent.id);
            async move { agents.get(&SecurityContext::system(), id).await.is_ok() }
        };

        running.delete_channel(&ctx, side).await.unwrap();
        assert!(!present(&sides).await, "the channel's agent goes with it");
        assert!(present(&sessions).await && present(&users).await, "the session's and the user's stay");
        running.restore_channel(&ctx, side).await.unwrap();
        assert!(present(&sides).await, "and comes back with it");

        manager.delete_session(&ctx, id).await.unwrap();
        assert!(!present(&sessions).await && !present(&sides).await, "the session's and its channels' go with it");
        assert!(personalities.get(&system, personality.id).await.is_err(), "so do its personalities");
        assert!(present(&users).await, "the user's stays");

        manager.restore_session(&ctx, id).await.unwrap();
        assert!(present(&sessions).await && present(&sides).await, "back with it");
        assert!(personalities.get(&system, personality.id).await.is_ok());
        assert!(!present(&earlier).await, "but not what was deleted before it");

        // A name taken meanwhile (issue 250): that one stays deleted, the
        // others come back.
        manager.delete_session(&ctx, id).await.unwrap();
        let taker = agent_in("Aside", Scope::ChannelScope { session_id: id, channel_id: side });
        agents.create(&system, &taker).await.unwrap();
        manager.restore_session(&ctx, id).await.unwrap();
        assert!(!present(&sides).await, "the one whose name is taken stays deleted");
        assert!(present(&sessions).await && present(&taker).await, "the others are back");

        manager.delete_session(&ctx, id).await.unwrap();
        manager.purge_session(&ctx, id).await.unwrap();
        for agent in [&sessions, &sides, &earlier, &taker] {
            assert!(agents.get_deleted(&system, agent.id).await.is_err(), "{} is purged with it", agent.name);
        }
        assert!(personalities.get_deleted(&system, personality.id).await.is_err());
        assert!(present(&users).await, "the user's is not");
    }

    #[tokio::test]
    async fn a_running_session_is_no_way_around_the_policy() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserId;

        let (_, manager) = owned_runtime(Default::default());

        let as_user = |id| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
        let mut session = make_session("hers", ActorId::User(alice));
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(ActorId::User(bob)), caelumora_model::security::Grant::Read);
        let id = session.id;
        create(&manager, &as_user(alice), session).await.expect("alice creates her own session");

        let refused = manager.open_session(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::NotFound(_))), "not refused - absent: {refused}");
        manager.open_session(&as_user(bob), id).await.expect("a contributor opens it");

        let refused = manager.delete_session(&as_user(bob), id).await.expect_err("a contributor may not delete it");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(manager.sessions.contains_key(&id), "a refused deletion must not stop the session on the way");

        let refused = manager.stop_session(&as_user(carol), id).await.expect_err("carol may not see it");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::NotFound(_))), "not refused - absent: {refused}");
        let refused = manager.stop_session(&as_user(bob), id).await.expect_err("a contributor may not stop it");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(manager.sessions.contains_key(&id), "it runs on");
        assert!(manager.stop_session(&as_user(alice), id).await.unwrap(), "the owner stops it");
        assert!(!manager.stop_session(&as_user(alice), id).await.unwrap(), "stopped already");

        manager.delete_session(&as_user(alice), id).await.expect("the owner deletes it");
        assert!(!manager.sessions.contains_key(&id));
    }
}
