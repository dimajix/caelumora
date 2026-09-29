use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::meta::Scope;
use crate::security::SecurityManager;
use crate::store_impl::access::Access;
use crate::session::{Session, SessionId, SessionState};
use crate::store::{EntityStore, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub struct JsonFileStore {
    root: PathBuf,
    security_manager: Arc<dyn SecurityManager>,
}

impl JsonFileStore {
    pub fn new(root: impl Into<PathBuf>, security_manager: Arc<dyn SecurityManager>) -> StoreResult<Self> {
        let root = root.into();
        info!("Creating JSON file store at {:?}", root);
        let store = Self {
            root,
            security_manager,
        };
        store.ensure_layout()?;
        Ok(store)
    }

    fn access<'a>(&'a self, ctx: &'a SecurityContext) -> Access<'a> {
        Access::new(self.security_manager.as_ref(), ctx)
    }

    fn ensure_layout(&self) -> StoreResult<()> {
        let dirs = [
            self.personality_dir(),
            self.session_templates_dir(),
            self.agents_dir(),
            self.sessions_dir(),
            self.channels_dir(),
            self.users_dir(),
            self.agent_states_dir(),
            self.agent_channel_states_dir(),
            self.session_states_dir(),
            self.channel_states_dir(),
            self.components_dir(),
        ];

        for dir in dirs {
            fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(())
    }

    fn personality_dir(&self) -> PathBuf {
        self.root.join("templates").join("agents")
    }

    fn session_templates_dir(&self) -> PathBuf {
        self.root.join("templates").join("sessions")
    }

    fn agents_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agents")
    }

    fn users_dir(&self) -> PathBuf {
        self.root.join("runtime").join("users")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.root.join("runtime").join("sessions")
    }

    fn channels_dir(&self) -> PathBuf {
        self.root.join("runtime").join("channels")
    }

    fn agent_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent_state")
    }

    fn agent_channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent_channel_state")
    }

    fn session_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("session_state")
    }

    fn channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("channel_state")
    }

    fn components_dir(&self) -> PathBuf {
        self.root.join("templates").join("components")
    }

    fn personality_path(&self, id: PersonalityId) -> PathBuf {
        self.personality_dir().join(format!("{id}.json"))
    }

    fn session_template_path(&self, name: &str) -> PathBuf {
        self.session_templates_dir()
            .join(format!("{}.json", sanitize_file_component(name)))
    }

    fn agent_path(&self, id: AgentId) -> PathBuf {
        self.agents_dir().join(format!("{id}.json"))
    }

    fn session_path(&self, id: SessionId) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn channel_path(&self, id: ChannelId) -> PathBuf {
        self.channels_dir().join(format!("{id}.json"))
    }

    fn user_path(&self, id: UserId) -> PathBuf {
        self.users_dir().join(format!("{id}.json"))
    }

    fn agent_channel_state_path(&self, agent_id: AgentId, channel_id: ChannelId) -> PathBuf {
        self.agent_channel_state_dir(channel_id)
            .join(format!("{}.json", agent_id))
    }

    fn agent_channel_state_dir(&self, channel_id: ChannelId) -> PathBuf {
        self.agent_channel_states_dir().join(channel_id.to_string())
    }

    fn agent_state_path(&self, id: AgentId) -> PathBuf {
        self.agent_states_dir().join(format!("{id}.json"))
    }

    fn session_state_path(&self, id: SessionId) -> PathBuf {
        self.session_states_dir().join(format!("{id}.json"))
    }

    fn channel_state_path(&self, id: ChannelId) -> PathBuf {
        self.channel_states_dir().join(format!("{id}.json"))
    }

    fn component_path(&self, id: &str) -> PathBuf {
        self.components_dir()
            .join(format!("{}.json", sanitize_file_component(id)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Persisted<T> {
    schema_version: u32,
    persisted_at: DateTime<Utc>,
    #[serde(flatten)]
    value: T,
}

/// Writes `value` to `path` atomically: into a temporary file beside it,
/// then renamed over it. The temporary name is unique per write, so that two
/// writers of the same file do not share one (issue 62): each rename
/// succeeds, and the last one wins.
async fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> StoreResult<()> {
    let payload = serde_json::to_vec_pretty(value).map_err(to_backend_error)?;
    let mut tmp_name: OsString = path
        .file_name()
        .ok_or_else(|| StoreError::Backend(format!("invalid path: {}", path.display())))?
        .to_os_string();
    tmp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let tmp_path = path.with_file_name(tmp_name);
    if let Err(err) = tokio::fs::write(&tmp_path, payload).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    if let Err(err) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(to_backend_error(err));
    }
    Ok(())
}

async fn read_json_opt<T: DeserializeOwned>(path: &Path) -> StoreResult<Option<T>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(to_backend_error)?;
            Ok(Some(parsed))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(to_backend_error(err)),
    }
}

fn require_entity<T>(value: Option<T>, entity: &'static str, id: String) -> StoreResult<T> {
    value.ok_or(StoreError::NotFound { entity, id })
}

/// Every `*.json` file in `dir`; temporary files of writes in progress, or
/// left behind by a crash, are not among them.
async fn read_all_from_dir<T: DeserializeOwned>(dir: &Path) -> StoreResult<Vec<T>> {
    let mut values = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await.map_err(to_backend_error)?;
    loop {
        let path = match entries.next_entry().await {
            Ok(Some(e)) => e.path(),
            Ok(None) => break,
            Err(err) => {
                warn!("Failed to read directory entry in {:?}: {}", dir, err);
                continue;
            }
        };

        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        match read_json_opt(&path).await {
            Ok(Some(value)) => values.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!("Failed to read JSON file {:?}: {}", path, err);
            }
        }
    }
    Ok(values)
}

async fn delete_if_exists(path: &Path) -> StoreResult<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

async fn delete_dir_if_exists(path: &Path) -> StoreResult<bool> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(to_backend_error(err)),
    }
}

fn to_backend_error(err: impl Display) -> StoreError {
    StoreError::Backend(err.to_string())
}

fn sanitize_file_component(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

use crate::security::{ActionType, SecurityContext};

#[async_trait]
impl EntityStore for JsonFileStore {
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        let path = self.personality_path(id);
        debug!("Reading personality template from {}", path.display());
        let loaded = read_json_opt::<Persisted<Personality>>(&path).await?.map(|p| p.value);
        self.access(ctx).read(loaded, |a| ActionType::GetPersonality(a), "agent_template", id.to_string()).await
    }

    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Personality> {
        self.access(ctx).allow(ActionType::FindPersonality).await?;
        let templates = self.list_personalities(ctx).await?;
        require_entity(
            templates
                .into_iter()
                .find(|t| t.metadata.name == name && scope.contains(&t.metadata.scope)),
            "agent_template",
            name.to_string(),
        )
    }

    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListPersonalities).await?;
        let all = read_all_from_dir::<Persisted<Personality>>(&self.personality_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .collect();
        Ok(access.filter(all, |a| ActionType::GetPersonality(a)).await)
    }

    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        self.access(ctx).allow(ActionType::FindSessionTemplate).await?;
        let templates = self.list_session_templates(ctx).await?;
        require_entity(
            templates
                .into_iter()
                .find(|t| t.metadata.name == name && scope.contains(&t.metadata.scope)),
            "session_template",
            name.to_string(),
        )
    }

    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListSessionTemplates).await?;
        let all = read_all_from_dir::<Persisted<SessionTemplate>>(&self.session_templates_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .collect();
        Ok(access.filter(all, |a| ActionType::GetSessionTemplate(a)).await)
    }

    async fn put_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        let path = self.personality_path(value.id);
        let stored = read_json_opt::<Persisted<Personality>>(&path).await?.map(|p| p.value);
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetPersonality(a), |a| ActionType::CreatePersonality(a), |a| ActionType::UpdatePersonality(a),
            "agent_template", value.id.to_string(),
        ).await?;
        debug!("Writing personality template to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        let path = self.personality_path(id);
        let stored = read_json_opt::<Persisted<Personality>>(&path).await?.map(|p| p.value);
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetPersonality(a), |a| ActionType::DeletePersonality(a)).await? {
            return Ok(false);
        }
        delete_if_exists(&path).await
    }

    async fn put_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        let path = self.session_template_path(&value.metadata.name);
        let stored = read_json_opt::<Persisted<SessionTemplate>>(&path).await?.map(|p| p.value);
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetSessionTemplate(a), |a| ActionType::CreateSessionTemplate(a), |a| ActionType::UpdateSessionTemplate(a),
            "session_template", value.id.to_string(),
        ).await?;
        debug!("Writing session template to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        let template = read_all_from_dir::<Persisted<SessionTemplate>>(&self.session_templates_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .find(|t| t.id == id);
        if !self.access(ctx).delete(template.as_ref(), |a| ActionType::GetSessionTemplate(a), |a| ActionType::DeleteSessionTemplate(a)).await? {
            return Ok(false);
        }
        match template {
            Some(t) => delete_if_exists(&self.session_template_path(&t.metadata.name)).await,
            None => Ok(false),
        }
    }

    // Agents (durable definitions)
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.access(ctx).create(value, |a| ActionType::CreateAgent(a)).await?;
        let path = self.agent_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
        }
        debug!("Creating agent at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        let path = self.agent_path(id);
        debug!("Reading agent from {}", path.display());
        let loaded = read_json_opt::<Persisted<Agent>>(&path).await?.map(|p| p.value);
        self.access(ctx).read(loaded, |a| ActionType::GetAgent(a), "agent", id.to_string()).await
    }

    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Agent> {
        let access = self.access(ctx);
        access.allow(ActionType::FindAgent).await?;
        let agents = read_all_from_dir::<Persisted<Agent>>(&self.agents_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .collect();
        require_entity(
            access.filter(agents, |a| ActionType::GetAgent(a)).await
                .into_iter()
                .find(|a| a.metadata.name == name && scope.contains(&a.metadata.scope)),
            "agent",
            name.to_string(),
        )
    }

    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        let path = self.agent_path(value.id);
        let stored = read_json_opt::<Persisted<Agent>>(&path).await?.map(|p| p.value);
        self.access(ctx).change(stored.as_ref(), |a| ActionType::GetAgent(a), |a| ActionType::UpdateAgent(a), "agent", value.id.to_string()).await?;
        debug!("Updating agent at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    // Sessions (durable definitions)
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.access(ctx).create(value, |a| ActionType::CreateSession(a)).await?;
        let path = self.session_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
        }
        debug!("Creating session at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        let path = self.session_path(id);
        debug!("Reading session from {}", path.display());
        let loaded = read_json_opt::<Persisted<Session>>(&path).await?.map(|p| p.value);
        self.access(ctx).read(loaded, |a| ActionType::GetSession(a), "session", id.to_string()).await
    }

    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        let path = self.session_path(value.id);
        let stored = read_json_opt::<Persisted<Session>>(&path).await?.map(|p| p.value);
        self.access(ctx).change(stored.as_ref(), |a| ActionType::GetSession(a), |a| ActionType::UpdateSession(a), "session", value.id.to_string()).await?;
        debug!("Updating session at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        // The state of the session and its channels is the `StateStore`'s,
        // which may be another store: the caller deletes it there.
        let session = read_json_opt::<Persisted<Session>>(&self.session_path(id)).await?.map(|p| p.value);
        if !self.access(ctx).delete(session.as_ref(), |a| ActionType::GetSession(a), |a| ActionType::DeleteSession(a)).await? {
            return Ok(false);
        }

        let removed = delete_if_exists(&self.session_path(id)).await?;
        info!("Deleted session {}", id);
        Ok(removed)
    }

    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListSessions).await?;
        let sessions = read_all_from_dir::<Persisted<Session>>(&self.sessions_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .collect();
        Ok(access.filter(sessions, |a| ActionType::GetSession(a)).await)
    }

    // Channels (durable definitions, issue 70)
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.access(ctx).create(value, |a| ActionType::CreateChannel(a)).await?;
        let path = self.channel_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("channel {} already exists", value.id)));
        }
        debug!("Creating channel at {}", path.display());
        write_json_atomic(&path, &Persisted { schema_version: 1, persisted_at: Utc::now(), value }).await
    }

    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        let loaded = read_json_opt::<Persisted<Channel>>(&self.channel_path(id)).await?.map(|p| p.value);
        self.access(ctx).read(loaded, |a| ActionType::GetChannel(a), "channel", id.to_string()).await
    }

    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        let path = self.channel_path(value.id);
        let stored = read_json_opt::<Persisted<Channel>>(&path).await?.map(|p| p.value);
        self.access(ctx).change(stored.as_ref(), |a| ActionType::GetChannel(a), |a| ActionType::UpdateChannel(a), "channel", value.id.to_string()).await?;
        debug!("Updating channel at {}", path.display());
        write_json_atomic(&path, &Persisted { schema_version: 1, persisted_at: Utc::now(), value }).await
    }

    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        // Its state is the `StateStore`'s: the caller deletes it there.
        let channel = read_json_opt::<Persisted<Channel>>(&self.channel_path(id)).await?.map(|p| p.value);
        if !self.access(ctx).delete(channel.as_ref(), |a| ActionType::GetChannel(a), |a| ActionType::DeleteChannel(a)).await? {
            return Ok(false);
        }
        let removed = delete_if_exists(&self.channel_path(id)).await?;
        info!("Deleted channel {}", id);
        Ok(removed)
    }

    // Users (durable definitions)
    async fn create_user(&self, _ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let path = self.user_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("user {} already exists", value.id)));
        }
        debug!("Creating user at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn get_user(&self, _ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        let path = self.user_path(id);
        debug!("Reading user from {}", path.display());
        let loaded = read_json_opt::<Persisted<User>>(&path).await?.map(|p| p.value);
        require_entity(loaded, "user", id.to_string())
    }

    async fn find_user_by_name(&self, _ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let dir = self.users_dir();
        let users: Vec<Persisted<User>> = read_all_from_dir(&dir).await?;
        require_entity(
            users
                .into_iter()
                .map(|p| p.value)
                .find(|t| t.username == name),
            "user",
            name.to_string(),
        )
    }

    async fn update_user(&self, _ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let path = self.user_path(value.id);
        if !path.exists() {
            return Err(StoreError::NotFound { entity: "user", id: value.id.to_string() });
        }
        debug!("Updating user at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor> {
        let path = self.component_path(id);
        debug!("Reading component from {}", path.display());
        let loaded = read_json_opt::<Persisted<ComponentDescriptor>>(&path).await?.map(|p| p.value);
        self.access(ctx).read(loaded, |a| ActionType::GetComponent(a), "component", id.to_string()).await
    }

    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>> {
        let access = self.access(ctx);
        access.allow(ActionType::ListComponents).await?;
        let all = read_all_from_dir::<Persisted<ComponentDescriptor>>(&self.components_dir()).await?
            .into_iter()
            .map(|p| p.value)
            .collect();
        Ok(access.filter(all, |a| ActionType::GetComponent(a)).await)
    }

    async fn put_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        let path = self.component_path(&value.id);
        let stored = read_json_opt::<Persisted<ComponentDescriptor>>(&path).await?.map(|p| p.value);
        self.access(ctx).put(
            stored.as_ref(), value,
            |a| ActionType::GetComponent(a), |a| ActionType::CreateComponent(a), |a| ActionType::UpdateComponent(a),
            "component", value.id.clone(),
        ).await?;
        debug!("Writing component to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        ).await
    }

    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        let path = self.component_path(id);
        let stored = read_json_opt::<Persisted<ComponentDescriptor>>(&path).await?.map(|p| p.value);
        if !self.access(ctx).delete(stored.as_ref(), |a| ActionType::GetComponent(a), |a| ActionType::DeleteComponent(a)).await? {
            return Ok(false);
        }
        delete_if_exists(&path).await
    }
}


#[async_trait]
impl StateStore for JsonFileStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        let path = self.agent_channel_state_path(agent_id, channel_id);
        debug!("Reading agent channel state from {}", path.display());
        let loaded = read_json_opt::<Persisted<AgentChannelState>>(&path).await?.map(|p| p.value);
        Ok(loaded.unwrap_or_default())
    }

    async fn put_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId, state: &AgentChannelState) -> StoreResult<()> {
        let path = self.agent_channel_state_path(agent_id, channel_id);
        debug!("Writing agent channel state to {}", path.display());
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(to_backend_error)?;
        }
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        ).await
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let path = self.agent_state_path(agent_id);
        debug!("Reading agent state from {}", path.display());
        let loaded = read_json_opt::<Persisted<AgentState>>(&path).await?.map(|p| p.value);
        Ok(loaded.unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        debug!("Writing agent state to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        ).await
    }

    async fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState> {
        let path = self.session_state_path(session_id);
        debug!("Reading session state from {}", path.display());
        let loaded = read_json_opt::<Persisted<SessionState>>(&path).await?.map(|p| p.value);
        Ok(loaded.unwrap_or_default())
    }

    async fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()> {
        let path = self.session_state_path(session_id);
        debug!("Writing session state to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        ).await
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let path = self.channel_state_path(channel_id);
        debug!("Reading channel state from {}", path.display());
        let loaded = read_json_opt::<Persisted<ChannelState>>(&path).await?.map(|p| p.value);
        loaded.ok_or(StoreError::NotFound {
            entity: "channel_state",
            id: channel_id.to_string(),
        })
    }

    async fn put_channel_state(&self, channel_id: ChannelId, state: &ChannelState) -> StoreResult<()> {
        let path = self.channel_state_path(channel_id);
        debug!("Writing channel state to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        ).await
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        debug!("Deleting the state of channel {}", channel_id);
        delete_if_exists(&self.channel_state_path(channel_id)).await?;
        delete_dir_if_exists(&self.agent_channel_state_dir(channel_id)).await?;
        Ok(())
    }

    async fn delete_session_state(&self, session_id: SessionId) -> StoreResult<()> {
        debug!("Deleting the state of session {}", session_id);
        delete_if_exists(&self.session_state_path(session_id)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{Metadata, Scope};
    use crate::security::SecurityAttributes;
    use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate};
    use tempfile::tempdir;
    use uuid::Uuid;

    fn make_personality() -> Personality {
        Personality {
            reasoning_confidentiality: crate::confidentiality::Confidentiality::Public,
            security_attributes: Default::default(),
            id: PersonalityId(Uuid::new_v4()),
            metadata: Metadata {
                name: "assistant".to_string(),
                description: "Helpful assistant".to_string(),
                ..Metadata::default()
            },
            components: crate::component::ComponentConfig::default(),
        }
    }

    fn make_session_template() -> SessionTemplate {
        SessionTemplate {
            security_attributes: Default::default(),
            commands: Default::default(),
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: "pairing".to_string(),
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            participants: vec![ParticipantTemplate::User{ name: "user".to_string() }],
            channels: vec![ChannelTemplate {
                name: "general".to_string(),
                moderator: "broadcast".to_string(),
                confidentiality: crate::confidentiality::Confidentiality::CHANNEL_DEFAULT,
                participants: vec!["user".to_string()],
            }],
        }
    }

    #[tokio::test]
    async fn concurrent_writes_of_one_file_all_succeed_and_leave_no_temporary_files() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = Arc::new(JsonFileStore::new(dir.path(), security_manager).unwrap());
        let (agent, channel) = (AgentId::new(), ChannelId::new());

        let mut writes = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let store = store.clone();
            writes.spawn(async move { store.put_agent_channel_state(agent, channel, &AgentChannelState::default()).await });
        }
        while let Some(result) = writes.join_next().await {
            result.unwrap().expect("no writer loses its temporary file to another");
        }

        let parent = store.agent_channel_state_path(agent, channel).parent().unwrap().to_path_buf();
        let names: Vec<String> = std::fs::read_dir(parent).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [format!("{agent}.json")], "only the file itself");
        store.get_agent_channel_state(agent, channel).await.expect("readable");
    }

    #[tokio::test]
    async fn template_roundtrip_works() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let personality = make_personality();
        store.put_personality(&ctx, &personality).await.unwrap();

        let found = store.get_personality(&ctx, personality.id).await.unwrap();
        assert_eq!(found.metadata.name, personality.metadata.name);

        let session_template = make_session_template();
        store.put_session_template(&ctx, &session_template).await.unwrap();
        let found_session = store
            .find_session_template_by_name(
                &ctx,
                &session_template.metadata.name,
                &session_template.metadata.scope,
            )
            .await
            .unwrap();
        assert_eq!(
            found_session.metadata.name,
            session_template.metadata.name
        );
    }

    #[tokio::test]
    async fn find_honors_containment() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let mut personality = make_personality();
        let user_id = UserId(Uuid::new_v4());
        let user_scope = Scope::UserScope { user_id };
        personality.metadata.scope = user_scope.clone();
        
        store.put_personality(&ctx, &personality).await.unwrap();

        // Search with UserScope should find it
        store.find_personality_by_name(&ctx, &personality.metadata.name, &user_scope).await.unwrap();

        // Search with GlobalScope should ALSO find it
        store.find_personality_by_name(&ctx, &personality.metadata.name, &Scope::GlobalScope).await.unwrap();

        // Search with different UserScope should NOT find it
        let other_user = Scope::UserScope { user_id: UserId(Uuid::new_v4()) };
        store.find_personality_by_name(&ctx, &personality.metadata.name, &other_user).await.unwrap_err();
    }

    #[tokio::test]
    async fn template_list_and_delete_work() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let mut t1 = make_personality();
        t1.metadata.name = "assistant-a".to_string();
        let mut t2 = make_personality();
        t2.metadata.name = "assistant-b".to_string();

        store.put_personality(&ctx, &t1).await.unwrap();
        store.put_personality(&ctx, &t2).await.unwrap();

        let templates = store.list_personalities(&ctx).await.unwrap();
        assert_eq!(templates.len(), 2);

        assert!(store.delete_personality(&ctx, t1.id).await.unwrap());
        assert!(!store.delete_personality(&ctx, t1.id).await.unwrap());
        let err = store.get_personality(&ctx, t1.id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "agent_template", .. }));
    }

    #[tokio::test]
    async fn state_roundtrip_works() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let template = make_personality();
        let agent = Agent::new(
            "assistant-1".to_string(),
            Scope::GlobalScope,
            &template,
            crate::actor::ActorId::None,
        );
        store.create_agent(&ctx, &agent).await.unwrap();

        let loaded = store.get_agent(&ctx, agent.id).await.unwrap();
        assert_eq!(loaded.metadata.name, agent.metadata.name);

        let state = AgentState::default();
        store.put_agent_state(agent.id, &state).await.unwrap();
        store.get_agent_state(agent.id).await.unwrap();
    }

    #[tokio::test]
    async fn create_conflict_and_update_not_found_are_reported() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let template = make_personality();
        let agent = Agent::new(
            "assistant-1".to_string(),
            Scope::GlobalScope,
            &template,
            crate::actor::ActorId::None,
        );

        store.create_agent(&ctx, &agent).await.unwrap();
        let conflict = store.create_agent(&ctx, &agent).await.unwrap_err();
        assert!(matches!(conflict, StoreError::Conflict(_)));

        let missing_agent = Agent {
            id: AgentId(Uuid::new_v4()),
            ..agent
        };
        let not_found = store.update_agent(&ctx, &missing_agent).await.unwrap_err();
        assert!(matches!(not_found, StoreError::NotFound { entity: "agent", .. }));
    }

    #[tokio::test]
    async fn session_list_works() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let s1 = Session {
            commands: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session-1".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        let s2 = Session {
            commands: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session-2".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };

        store.create_session(&ctx, &s1).await.unwrap();
        store.create_session(&ctx, &s2).await.unwrap();

        let sessions = store.list_sessions(&ctx).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == s1.id));
        assert!(sessions.iter().any(|s| s.id == s2.id));
    }

    #[tokio::test]
    async fn deleting_a_session_leaves_its_state_to_the_state_store() {
        use crate::channel::{Channel, ChannelId};

        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let session_id = SessionId(Uuid::new_v4());
        let channel = Channel {
            id: ChannelId(Uuid::new_v4()),
            session_id,
            name: "general".to_string(),
            moderator: "broadcast".to_string(),
            confidentiality: crate::confidentiality::Confidentiality::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
            participants: Vec::new(),
            security_attributes: SecurityAttributes::default(),
        };
        let channel_id = channel.id;
        let session = Session {
            commands: Default::default(),
            id: session_id,
            metadata: Metadata {
                name: "to-delete".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: vec![channel.id],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        store.create_session(&ctx, &session).await.unwrap();

        // Write out session state and channel state so we can assert they get removed.
        store.put_session_state(session_id, &SessionState::default()).await.unwrap();
        let agent_id = AgentId(Uuid::new_v4());
        store
            .put_agent_channel_state(agent_id, channel_id, &AgentChannelState::default())
            .await
            .unwrap();

        assert!(store.session_path(session_id).exists());
        assert!(store.session_state_path(session_id).exists());
        assert!(store.agent_channel_state_dir(channel_id).exists());

        assert!(store.delete_session(&ctx, session_id).await.unwrap());
        assert!(!store.session_path(session_id).exists());
        assert!(store.session_state_path(session_id).exists(), "the state may live in another store");

        store.put_channel_state(channel_id, &ChannelState::new(channel_id)).await.unwrap();
        store.delete_channel_state(channel_id).await.unwrap();
        store.delete_session_state(session_id).await.unwrap();
        assert!(!store.channel_state_path(channel_id).exists());
        assert!(!store.agent_channel_state_dir(channel_id).exists());
        assert!(!store.session_state_path(session_id).exists());
        store.delete_channel_state(channel_id).await.expect("nothing to delete is no error");

        // Deleting again reports that nothing was removed.
        assert!(!store.delete_session(&ctx, session_id).await.unwrap());
        let err = store.get_session(&ctx, session_id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "session", .. }));
    }

    #[tokio::test]
    async fn list_sessions_skips_corrupted_files() {
        let dir = tempdir().unwrap();
        let security_manager = Arc::new(crate::security::DummySecurityManager::new());
        let store = JsonFileStore::new(dir.path(), security_manager).unwrap();
        let ctx = SecurityContext::default();

        let s1 = Session {
            commands: Default::default(),
            id: SessionId(Uuid::new_v4()),
            metadata: Metadata {
                name: "session-1".to_string(),
                scope: Scope::GlobalScope,
                ..Metadata::default()
            },
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
        };
        store.create_session(&ctx, &s1).await.unwrap();

        // Create a corrupted JSON file
        let corrupted_path = store.sessions_dir().join("corrupted.json");
        fs::write(corrupted_path, "invalid json").unwrap();

        // This should NOT fail, it should return s1 and log a warning
        let sessions = store.list_sessions(&ctx).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, s1.id);
    }
}
