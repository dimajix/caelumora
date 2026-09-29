use crate::agent::{Agent, AgentChannelState, AgentId, AgentState, Personality, PersonalityId};
use crate::channel::{Channel, ChannelId, ChannelState};
use crate::component::ComponentDescriptor;
use crate::security::{Secured, SecurityManager};
use crate::store_impl::access::{Access, Actions, AGENTS, CHANNELS, COMPONENTS, PERSONALITIES, SESSIONS, SESSION_TEMPLATES};
use crate::session::{Session, SessionId};
use crate::store::{EntityStore, StateStore, StoreError, StoreResult};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId};
use async_trait::async_trait;
use tracing::{debug, info, warn};
use serde::de::DeserializeOwned;
use serde::Serialize;
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

    fn channel_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("channel_state")
    }

    fn components_dir(&self) -> PathBuf {
        self.root.join("templates").join("components")
    }

    fn personality_path(&self, id: PersonalityId) -> PathBuf {
        self.personality_dir().join(format!("{id}.json"))
    }

    fn session_template_path(&self, id: SessionTemplateId) -> PathBuf {
        self.session_templates_dir().join(format!("{id}.json"))
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

    fn channel_state_path(&self, id: ChannelId) -> PathBuf {
        self.channel_states_dir().join(format!("{id}.json"))
    }

    fn component_path(&self, id: &str) -> PathBuf {
        self.components_dir()
            .join(format!("{}.json", sanitize_file_component(id)))
    }
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

use crate::security::SecurityContext;

/// The five operations of every entity kind (issue 93), each document a
/// file at `path` and checked with its kind's `Actions`.
impl JsonFileStore {
    async fn create_file<T: Secured + Serialize + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        self.access(ctx).create(value, kind.create).await?;
        if tokio::fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::Conflict(format!("{} {} already exists", kind.entity, id)));
        }
        debug!("Writing {} {} to {}", kind.entity, id, path.display());
        write_json_atomic(&path, value).await
    }

    async fn get_file<T: Secured + DeserializeOwned + Send>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String) -> StoreResult<T> {
        let found = read_json_opt::<T>(&path).await?;
        self.access(ctx).read(found, kind.get, kind.entity, id).await
    }

    async fn update_file<T: Secured + Serialize + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf, id: String, value: &T) -> StoreResult<()> {
        let stored = read_json_opt::<T>(&path).await?;
        self.access(ctx).change(stored.as_ref(), kind.get, kind.update, kind.entity, id.clone()).await?;
        debug!("Writing {} {} to {}", kind.entity, id, path.display());
        write_json_atomic(&path, value).await
    }

    async fn delete_file<T: Secured + DeserializeOwned + Send + Sync>(&self, ctx: &SecurityContext, kind: &Actions, path: PathBuf) -> StoreResult<bool> {
        let stored = read_json_opt::<T>(&path).await?;
        if !self.access(ctx).delete(stored.as_ref(), kind.get, kind.delete).await? {
            return Ok(false);
        }
        debug!("Deleting {} at {}", kind.entity, path.display());
        delete_if_exists(&path).await
    }

    async fn list_dir<T: Secured + DeserializeOwned + Send>(&self, ctx: &SecurityContext, kind: &Actions, dir: PathBuf) -> StoreResult<Vec<T>> {
        let access = self.access(ctx);
        access.allow(kind.list).await?;
        let all = read_all_from_dir::<T>(&dir).await?;
        Ok(access.filter(all, kind.get).await)
    }
}

#[async_trait]
impl EntityStore for JsonFileStore {
    // Personalities
    async fn create_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.create_file(ctx, &PERSONALITIES, self.personality_path(value.id), value.id.to_string(), value).await
    }
    async fn get_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<Personality> {
        self.get_file(ctx, &PERSONALITIES, self.personality_path(id), id.to_string()).await
    }
    async fn update_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        self.update_file(ctx, &PERSONALITIES, self.personality_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        self.delete_file::<Personality>(ctx, &PERSONALITIES, self.personality_path(id)).await
    }
    async fn list_personalities(&self, ctx: &SecurityContext) -> StoreResult<Vec<Personality>> {
        self.list_dir(ctx, &PERSONALITIES, self.personality_dir()).await
    }

    // Session templates
    async fn create_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.create_file(ctx, &SESSION_TEMPLATES, self.session_template_path(value.id), value.id.to_string(), value).await
    }
    async fn get_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<SessionTemplate> {
        self.get_file(ctx, &SESSION_TEMPLATES, self.session_template_path(id), id.to_string()).await
    }
    async fn update_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        self.update_file(ctx, &SESSION_TEMPLATES, self.session_template_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        self.delete_file::<SessionTemplate>(ctx, &SESSION_TEMPLATES, self.session_template_path(id)).await
    }
    async fn list_session_templates(&self, ctx: &SecurityContext) -> StoreResult<Vec<SessionTemplate>> {
        self.list_dir(ctx, &SESSION_TEMPLATES, self.session_templates_dir()).await
    }

    // Agents
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.create_file(ctx, &AGENTS, self.agent_path(value.id), value.id.to_string(), value).await
    }
    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        self.get_file(ctx, &AGENTS, self.agent_path(id), id.to_string()).await
    }
    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        self.update_file(ctx, &AGENTS, self.agent_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<bool> {
        self.delete_file::<Agent>(ctx, &AGENTS, self.agent_path(id)).await
    }
    async fn list_agents(&self, ctx: &SecurityContext) -> StoreResult<Vec<Agent>> {
        self.list_dir(ctx, &AGENTS, self.agents_dir()).await
    }

    // Sessions - their state is the `StateStore`'s, which may be another
    // store: whoever deletes a session deletes it there.
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.create_file(ctx, &SESSIONS, self.session_path(value.id), value.id.to_string(), value).await
    }
    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.get_file(ctx, &SESSIONS, self.session_path(id), id.to_string()).await
    }
    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        self.update_file(ctx, &SESSIONS, self.session_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<bool> {
        self.delete_file::<Session>(ctx, &SESSIONS, self.session_path(id)).await
    }
    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>> {
        self.list_dir(ctx, &SESSIONS, self.sessions_dir()).await
    }

    // Channels (issue 70) - their state likewise.
    async fn create_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.create_file(ctx, &CHANNELS, self.channel_path(value.id), value.id.to_string(), value).await
    }
    async fn get_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<Channel> {
        self.get_file(ctx, &CHANNELS, self.channel_path(id), id.to_string()).await
    }
    async fn update_channel(&self, ctx: &SecurityContext, value: &Channel) -> StoreResult<()> {
        self.update_file(ctx, &CHANNELS, self.channel_path(value.id), value.id.to_string(), value).await
    }
    async fn delete_channel(&self, ctx: &SecurityContext, id: ChannelId) -> StoreResult<bool> {
        self.delete_file::<Channel>(ctx, &CHANNELS, self.channel_path(id)).await
    }
    async fn list_channels(&self, ctx: &SecurityContext) -> StoreResult<Vec<Channel>> {
        self.list_dir(ctx, &CHANNELS, self.channels_dir()).await
    }

    // Users - not checked yet (issue 94).
    async fn create_user(&self, _ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let path = self.user_path(value.id);
        if tokio::fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::Conflict(format!("user {} already exists", value.id)));
        }
        write_json_atomic(&path, value).await
    }
    async fn get_user(&self, _ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        require_entity(read_json_opt::<User>(&self.user_path(id)).await?, "user", id.to_string())
    }
    async fn update_user(&self, _ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let path = self.user_path(value.id);
        if !tokio::fs::try_exists(&path).await.map_err(to_backend_error)? {
            return Err(StoreError::NotFound { entity: "user", id: value.id.to_string() });
        }
        write_json_atomic(&path, value).await
    }
    async fn delete_user(&self, _ctx: &SecurityContext, id: UserId) -> StoreResult<bool> {
        delete_if_exists(&self.user_path(id)).await
    }
    async fn list_users(&self, _ctx: &SecurityContext) -> StoreResult<Vec<User>> {
        read_all_from_dir(&self.users_dir()).await
    }
    async fn find_user_by_name(&self, _ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let users: Vec<User> = read_all_from_dir(&self.users_dir()).await?;
        require_entity(users.into_iter().find(|u| u.username == name), "user", name.to_string())
    }

    // Components
    async fn create_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        self.create_file(ctx, &COMPONENTS, self.component_path(&value.id), value.id.clone(), value).await
    }
    async fn get_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<ComponentDescriptor> {
        self.get_file(ctx, &COMPONENTS, self.component_path(id), id.to_string()).await
    }
    async fn update_component(&self, ctx: &SecurityContext, value: &ComponentDescriptor) -> StoreResult<()> {
        self.update_file(ctx, &COMPONENTS, self.component_path(&value.id), value.id.clone(), value).await
    }
    async fn delete_component(&self, ctx: &SecurityContext, id: &str) -> StoreResult<bool> {
        self.delete_file::<ComponentDescriptor>(ctx, &COMPONENTS, self.component_path(id)).await
    }
    async fn list_components(&self, ctx: &SecurityContext) -> StoreResult<Vec<ComponentDescriptor>> {
        self.list_dir(ctx, &COMPONENTS, self.components_dir()).await
    }
}

#[async_trait]
impl StateStore for JsonFileStore {
    async fn get_agent_channel_state(&self, agent_id: AgentId, channel_id: ChannelId) -> StoreResult<AgentChannelState> {
        let path = self.agent_channel_state_path(agent_id, channel_id);
        debug!("Reading agent channel state from {}", path.display());
        let loaded = read_json_opt::<AgentChannelState>(&path).await?;
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
            state,
        ).await
    }

    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let path = self.agent_state_path(agent_id);
        debug!("Reading agent state from {}", path.display());
        let loaded = read_json_opt::<AgentState>(&path).await?;
        Ok(loaded.unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        debug!("Writing agent state to {}", path.display());
        write_json_atomic(
            &path,
            state,
        ).await
    }

    async fn get_channel_state(&self, channel_id: ChannelId) -> StoreResult<ChannelState> {
        let path = self.channel_state_path(channel_id);
        debug!("Reading channel state from {}", path.display());
        let loaded = read_json_opt::<ChannelState>(&path).await?;
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
            state,
        ).await
    }

    async fn delete_channel_state(&self, channel_id: ChannelId) -> StoreResult<()> {
        debug!("Deleting the state of channel {}", channel_id);
        delete_if_exists(&self.channel_state_path(channel_id)).await?;
        delete_dir_if_exists(&self.agent_channel_state_dir(channel_id)).await?;
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
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
        store.create_personality(&ctx, &personality).await.unwrap();

        let found = store.get_personality(&ctx, personality.id).await.unwrap();
        assert_eq!(found.metadata.name, personality.metadata.name);

        let session_template = make_session_template();
        store.create_session_template(&ctx, &session_template).await.unwrap();
        let found_session = store.get_session_template(&ctx, session_template.id).await.unwrap();
        assert_eq!(
            found_session.metadata.name,
            session_template.metadata.name
        );
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

        store.create_personality(&ctx, &t1).await.unwrap();
        store.create_personality(&ctx, &t2).await.unwrap();

        let templates = store.list_personalities(&ctx).await.unwrap();
        assert_eq!(templates.len(), 2);

        assert!(store.delete_personality(&ctx, t1.id).await.unwrap());
        assert!(!store.delete_personality(&ctx, t1.id).await.unwrap());
        let err = store.get_personality(&ctx, t1.id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "personality", .. }));
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

        // Write out channel state so we can assert it gets removed.
        let agent_id = AgentId(Uuid::new_v4());
        store
            .put_agent_channel_state(agent_id, channel_id, &AgentChannelState::default())
            .await
            .unwrap();

        assert!(store.session_path(session_id).exists());
        assert!(store.agent_channel_state_dir(channel_id).exists());

        assert!(store.delete_session(&ctx, session_id).await.unwrap());
        assert!(!store.session_path(session_id).exists());

        store.put_channel_state(channel_id, &ChannelState::new(channel_id)).await.unwrap();
        store.delete_channel_state(channel_id).await.unwrap();
        assert!(!store.channel_state_path(channel_id).exists());
        assert!(!store.agent_channel_state_dir(channel_id).exists());
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
