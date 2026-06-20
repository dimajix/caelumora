use async_trait::async_trait;
use std::sync::Arc;
use crate::security::SecurityManager;
use crate::agent::{Agent, AgentId, AgentState, Personality, PersonalityId};
use crate::session::{Session, SessionId, SessionState};
use crate::store::{StateStore, StoreError, StoreResult, EntityStore};
use crate::template::{SessionTemplate, SessionTemplateId};
use crate::user::{User, UserId};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use crate::meta::Scope;
use log::info;

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

    fn ensure_layout(&self) -> StoreResult<()> {
        let dirs = [
            self.personality_dir(),
            self.session_templates_dir(),
            self.agents_dir(),
            self.sessions_dir(),
            self.users_dir(),
            self.agent_states_dir(),
            self.session_states_dir(),
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

    fn agent_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agent_state")
    }

    fn session_states_dir(&self) -> PathBuf {
        self.root.join("runtime").join("session_state")
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

    fn user_path(&self, id: UserId) -> PathBuf {
        self.users_dir().join(format!("{id}.json"))
    }

    fn agent_state_path(&self, id: AgentId) -> PathBuf {
        self.agent_states_dir().join(format!("{id}.json"))
    }

    fn session_state_path(&self, id: SessionId) -> PathBuf {
        self.session_states_dir().join(format!("{id}.json"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Persisted<T> {
    schema_version: u32,
    persisted_at: DateTime<Utc>,
    #[serde(flatten)]
    value: T,
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> StoreResult<()> {
    let payload = serde_json::to_vec_pretty(value).map_err(to_backend_error)?;
    let mut tmp_name: OsString = path
        .file_name()
        .ok_or_else(|| StoreError::Backend(format!("invalid path: {}", path.display())))?
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = path.with_file_name(tmp_name);
    fs::write(&tmp_path, payload).map_err(to_backend_error)?;
    fs::rename(&tmp_path, path).map_err(to_backend_error)?;
    Ok(())
}

fn read_json_opt<T: DeserializeOwned>(path: &Path) -> StoreResult<Option<T>> {
    match fs::read_to_string(path) {
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

fn read_all_from_dir<T: DeserializeOwned>(dir: &Path) -> StoreResult<Vec<T>> {
    let mut values = Vec::new();
    let entries = fs::read_dir(dir).map_err(to_backend_error)?;
    for entry in entries {
        let path = entry.map_err(to_backend_error)?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Some(value) = read_json_opt(&path)? {
            values.push(value);
        }
    }
    Ok(values)
}

fn delete_if_exists(path: &Path) -> StoreResult<bool> {
    match fs::remove_file(path) {
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
        if !self.security_manager.check(ctx, ActionType::GetPersonality(id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.personality_path(id);
        info!("Reading personality template from {}", path.display());
        let loaded = read_json_opt::<Persisted<Personality>>(&path)?.map(|p| p.value);
        require_entity(loaded, "agent_template", id.to_string())
    }

    async fn find_personality_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Personality> {
        if !self.security_manager.check(ctx, ActionType::FindPersonality).await {
            return Err(StoreError::PermissionDenied);
        }
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
        if !self.security_manager.check(ctx, ActionType::ListPersonalities).await {
            return Err(StoreError::PermissionDenied);
        }
        Ok(read_all_from_dir::<Persisted<Personality>>(&self.personality_dir())?
            .into_iter()
            .map(|p| p.value)
            .collect())
    }

    async fn find_session_template_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        if !self.security_manager.check(ctx, ActionType::FindSessionTemplate).await {
            return Err(StoreError::PermissionDenied);
        }
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
        if !self.security_manager.check(ctx, ActionType::ListSessionTemplates).await {
            return Err(StoreError::PermissionDenied);
        }
        Ok(read_all_from_dir::<Persisted<SessionTemplate>>(&self.session_templates_dir())?
            .into_iter()
            .map(|p| p.value)
            .collect())
    }

    async fn put_personality(&self, ctx: &SecurityContext, value: &Personality) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::PutPersonality(value)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.personality_path(value.id);
        info!("Writing personality template to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn delete_personality(&self, ctx: &SecurityContext, id: PersonalityId) -> StoreResult<bool> {
        if !self.security_manager.check(ctx, ActionType::DeletePersonality(id)).await {
            return Err(StoreError::PermissionDenied);
        }
        delete_if_exists(&self.personality_path(id))
    }

    async fn put_session_template(&self, ctx: &SecurityContext, value: &SessionTemplate) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::PutSessionTemplate(value)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.session_template_path(&value.metadata.name);
        info!("Writing session template to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn delete_session_template(&self, ctx: &SecurityContext, id: SessionTemplateId) -> StoreResult<bool> {
        if !self.security_manager.check(ctx, ActionType::DeleteSessionTemplate(id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let template = self
            .list_session_templates(ctx).await?
            .into_iter()
            .find(|t| t.id == id);

        match template {
            Some(t) => delete_if_exists(&self.session_template_path(&t.metadata.name)),
            None => Ok(false),
        }
    }

    // Agents (durable definitions)
    async fn create_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::CreateAgent(value)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.agent_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
        }
        info!("Creating agent at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn get_agent(&self, ctx: &SecurityContext, id: AgentId) -> StoreResult<Agent> {
        if !self.security_manager.check(ctx, ActionType::GetAgent(id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.agent_path(id);
        info!("Reading agent from {}", path.display());
        let loaded = read_json_opt::<Persisted<Agent>>(&path)?.map(|p| p.value);
        require_entity(loaded, "agent", id.to_string())
    }

    async fn find_agent_by_name(&self, ctx: &SecurityContext, name: &str, scope: &Scope) -> StoreResult<Agent> {
        if !self.security_manager.check(ctx, ActionType::FindAgent).await {
            return Err(StoreError::PermissionDenied);
        }
        let agents = read_all_from_dir::<Persisted<Agent>>(&self.agents_dir())?;
        require_entity(
            agents
                .into_iter()
                .map(|p| p.value)
                .find(|a| a.metadata.name == name && scope.contains(&a.metadata.scope)),
            "agent",
            name.to_string(),
        )
    }

    async fn update_agent(&self, ctx: &SecurityContext, value: &Agent) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::UpdateAgent(value.id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.agent_path(value.id);
        if !path.exists() {
            return Err(StoreError::NotFound {
                entity: "agent",
                id: value.id.to_string(),
            });
        }
        info!("Updating agent at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    // Sessions (durable definitions)
    async fn create_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::CreateSession(value)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.session_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
        }
        info!("Creating session at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn get_session(&self, ctx: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        if !self.security_manager.check(ctx, ActionType::GetSession(id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.session_path(id);
        info!("Reading session from {}", path.display());
        let loaded = read_json_opt::<Persisted<Session>>(&path)?.map(|p| p.value);
        require_entity(loaded, "session", id.to_string())
    }

    async fn update_session(&self, ctx: &SecurityContext, value: &Session) -> StoreResult<()> {
        if !self.security_manager.check(ctx, ActionType::UpdateSession(value.id)).await {
            return Err(StoreError::PermissionDenied);
        }
        let path = self.session_path(value.id);
        if !path.exists() {
            return Err(StoreError::NotFound {
                entity: "session",
                id: value.id.to_string(),
            });
        }
        info!("Updating session at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn list_sessions(&self, ctx: &SecurityContext) -> StoreResult<Vec<Session>> {
        if !self.security_manager.check(ctx, ActionType::ListSessions).await {
            return Err(StoreError::PermissionDenied);
        }
        let dir = self.sessions_dir();
        let sessions: Vec<Persisted<Session>> = read_all_from_dir(&dir)?;
        Ok(sessions.into_iter().map(|p| p.value).collect())
    }

    // Users (durable definitions)
    async fn create_user(&self, _ctx: &SecurityContext, value: &User) -> StoreResult<()> {
        let path = self.user_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("user {} already exists", value.id)));
        }
        info!("Creating user at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }

    async fn get_user(&self, _ctx: &SecurityContext, id: UserId) -> StoreResult<User> {
        let path = self.user_path(id);
        info!("Reading user from {}", path.display());
        let loaded = read_json_opt::<Persisted<User>>(&path)?.map(|p| p.value);
        require_entity(loaded, "user", id.to_string())
    }

    async fn find_user_by_name(&self, _ctx: &SecurityContext, name: &str) -> StoreResult<User> {
        let _ = _ctx;
        let dir = self.users_dir();
        let users: Vec<Persisted<User>> = read_all_from_dir(&dir)?;
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
        info!("Updating user at {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value,
            },
        )
    }
}


#[async_trait]
impl StateStore for JsonFileStore {
    async fn get_agent_state(&self, agent_id: AgentId) -> StoreResult<AgentState> {
        let path = self.agent_state_path(agent_id);
        info!("Reading agent state from {}", path.display());
        let loaded = read_json_opt::<Persisted<AgentState>>(&path)?.map(|p| p.value);
        Ok(loaded.unwrap_or_default())
    }

    async fn put_agent_state(&self, agent_id: AgentId, state: &AgentState) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        info!("Writing agent state to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        )
    }

    async fn get_session_state(&self, session_id: SessionId) -> StoreResult<SessionState> {
        let path = self.session_state_path(session_id);
        info!("Reading session state from {}", path.display());
        let loaded = read_json_opt::<Persisted<SessionState>>(&path)?.map(|p| p.value);
        Ok(loaded.unwrap_or_default())
    }

    async fn put_session_state(&self, session_id: SessionId, state: &SessionState) -> StoreResult<()> {
        let path = self.session_state_path(session_id);
        info!("Writing session state to {}", path.display());
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                persisted_at: Utc::now(),
                value: state,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::meta::{Metadata, Scope};
    use tempfile::tempdir;
    use uuid::Uuid;
    use super::*;
    use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate};

    fn make_personality() -> Personality {
        Personality {
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
            id: SessionTemplateId(Uuid::new_v4()),
            metadata: Metadata {
                name: "pairing".to_string(),
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            channels: vec![ChannelTemplate {
                name: "general".to_string(),
                moderator: "broadcast".to_string(),
                participants: vec![ParticipantTemplate::User],
            }],
        }
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
            id: SessionId(Uuid::new_v4()),
            metadata: crate::meta::Metadata {
                name: "session-1".to_string(),
                scope: Scope::GlobalScope,
                ..crate::meta::Metadata::default()
            },
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let s2 = Session {
            id: SessionId(Uuid::new_v4()),
            metadata: crate::meta::Metadata {
                name: "session-2".to_string(),
                scope: Scope::GlobalScope,
                ..crate::meta::Metadata::default()
            },
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        store.create_session(&ctx, &s1).await.unwrap();
        store.create_session(&ctx, &s2).await.unwrap();

        let sessions = store.list_sessions(&ctx).await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.id == s1.id));
        assert!(sessions.iter().any(|s| s.id == s2.id));
    }
}
