use crate::agent::{Agent, AgentState, Personality};
use crate::session::{Session, SessionState};
use crate::store::{StateStore, StoreError, StoreResult, TemplateStore};
use crate::template::{SessionTemplate};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use crate::meta::Scope;

#[derive(Debug)]
pub struct JsonFileStore {
    root: PathBuf,
}

impl JsonFileStore {
    pub fn new(root: impl Into<PathBuf>) -> StoreResult<Self> {
        let store = Self { root: root.into() };
        store.ensure_layout()?;
        Ok(store)
    }

    fn ensure_layout(&self) -> StoreResult<()> {
        let dirs = [
            self.agent_templates_dir(),
            self.session_templates_dir(),
            self.agents_dir(),
            self.sessions_dir(),
            self.agent_states_dir(),
            self.session_states_dir(),
        ];

        for dir in dirs {
            fs::create_dir_all(dir).map_err(to_backend_error)?;
        }
        Ok(())
    }

    fn agent_templates_dir(&self) -> PathBuf {
        self.root.join("templates").join("agents")
    }

    fn session_templates_dir(&self) -> PathBuf {
        self.root.join("templates").join("sessions")
    }

    fn agents_dir(&self) -> PathBuf {
        self.root.join("runtime").join("agents")
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

    fn agent_template_path(&self, id: Uuid) -> PathBuf {
        self.agent_templates_dir().join(format!("{id}.json"))
    }

    fn session_template_path(&self, name: &str) -> PathBuf {
        self.session_templates_dir()
            .join(format!("{}.json", sanitize_file_component(name)))
    }

    fn agent_path(&self, id: Uuid) -> PathBuf {
        self.agents_dir().join(format!("{id}.json"))
    }

    fn session_path(&self, id: Uuid) -> PathBuf {
        self.sessions_dir().join(format!("{id}.json"))
    }

    fn agent_state_path(&self, id: Uuid) -> PathBuf {
        self.agent_states_dir().join(format!("{id}.json"))
    }

    fn session_state_path(&self, id: Uuid) -> PathBuf {
        self.session_states_dir().join(format!("{id}.json"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Persisted<T> {
    schema_version: u32,
    updated_at: DateTime<Utc>,
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

impl TemplateStore for JsonFileStore {
    fn get_personality(&self, id: Uuid) -> StoreResult<Personality> {
        let path = self.agent_template_path(id);
        let loaded = read_json_opt::<Persisted<Personality>>(&path)?.map(|p| p.value);
        require_entity(loaded, "agent_template", id.to_string())
    }

    fn find_personality_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Personality> {
        let templates = self.list_personalities()?;
        require_entity(
            templates
                .into_iter()
                .find(|t| t.metadata.name == name && t.metadata.scope == *scope),
            "agent_template",
            name.to_string(),
        )
    }

    fn list_personalities(&self) -> StoreResult<Vec<Personality>> {
        Ok(read_all_from_dir::<Persisted<Personality>>(&self.agent_templates_dir())?
            .into_iter()
            .map(|p| p.value)
            .collect())
    }

    fn find_session_template_by_name(&self, name: &str, scope: &Scope) -> StoreResult<SessionTemplate> {
        let templates = self.list_session_templates()?;
        require_entity(
            templates
                .into_iter()
                .find(|t| t.metadata.name == name && t.metadata.scope == *scope),
            "session_template",
            name.to_string(),
        )
    }

    fn list_session_templates(&self) -> StoreResult<Vec<SessionTemplate>> {
        Ok(read_all_from_dir::<Persisted<SessionTemplate>>(&self.session_templates_dir())?
            .into_iter()
            .map(|p| p.value)
            .collect())
    }

    fn put_personality(&self, value: &Personality) -> StoreResult<()> {
        let path = self.agent_template_path(value.id);
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn delete_personality(&self, id: Uuid) -> StoreResult<bool> {
        delete_if_exists(&self.agent_template_path(id))
    }

    fn put_session_template(&self, value: &SessionTemplate) -> StoreResult<()> {
        let path = self.session_template_path(&value.metadata.name);
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn delete_session_template(&self, id: Uuid) -> StoreResult<bool> {
        let template = self
            .list_session_templates()?
            .into_iter()
            .find(|t| t.id == id);

        match template {
            Some(t) => delete_if_exists(&self.session_template_path(&t.metadata.name)),
            None => Ok(false),
        }
    }
}



impl StateStore for JsonFileStore {
    fn create_agent(&self, value: &Agent) -> StoreResult<()> {
        let path = self.agent_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("agent {} already exists", value.id)));
        }
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn get_agent(&self, id: Uuid) -> StoreResult<Agent> {
        let path = self.agent_path(id);
        let loaded = read_json_opt::<Persisted<Agent>>(&path)?.map(|p| p.value);
        require_entity(loaded, "agent", id.to_string())
    }

    fn find_agent_by_name(&self, name: &str, scope: &Scope) -> StoreResult<Option<Agent>> {
        let agents = read_all_from_dir::<Persisted<Agent>>(&self.agents_dir())?;
        Ok(agents
            .into_iter()
            .map(|p| p.value)
            .find(|a| a.metadata.name == name && a.metadata.scope == *scope))
    }

    fn update_agent(&self, value: &Agent) -> StoreResult<()> {
        let path = self.agent_path(value.id);
        if !path.exists() {
            return Err(StoreError::NotFound {
                entity: "agent",
                id: value.id.to_string(),
            });
        }
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn create_session(&self, value: &Session) -> StoreResult<()> {
        let path = self.session_path(value.id);
        if path.exists() {
            return Err(StoreError::Conflict(format!("session {} already exists", value.id)));
        }
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn get_session(&self, id: Uuid) -> StoreResult<Session> {
        let path = self.session_path(id);
        let loaded = read_json_opt::<Persisted<Session>>(&path)?.map(|p| p.value);
        require_entity(loaded, "session", id.to_string())
    }

    fn update_session(&self, value: &Session) -> StoreResult<()> {
        let path = self.session_path(value.id);
        if !path.exists() {
            return Err(StoreError::NotFound {
                entity: "session",
                id: value.id.to_string(),
            });
        }
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value,
            },
        )
    }

    fn get_agent_state(&self, agent_id: Uuid) -> StoreResult<AgentState> {
        let path = self.agent_state_path(agent_id);
        let loaded = read_json_opt::<Persisted<AgentState>>(&path)?.map(|p| p.value);
        require_entity(loaded, "agent_state", agent_id.to_string())
    }

    fn put_agent_state(&self, agent_id: Uuid, state: &AgentState) -> StoreResult<()> {
        let path = self.agent_state_path(agent_id);
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value: state,
            },
        )
    }

    fn get_session_state(&self, session_id: Uuid) -> StoreResult<SessionState> {
        let path = self.session_state_path(session_id);
        let loaded = read_json_opt::<Persisted<SessionState>>(&path)?.map(|p| p.value);
        require_entity(loaded, "session_state", session_id.to_string())
    }

    fn put_session_state(&self, session_id: Uuid, state: &SessionState) -> StoreResult<()> {
        let path = self.session_state_path(session_id);
        write_json_atomic(
            &path,
            &Persisted {
                schema_version: 1,
                updated_at: Utc::now(),
                value: state,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::meta::{Metadata, Scope};
    use tempfile::tempdir;

    use super::*;
    use crate::template::{ChannelTemplate, ParticipantTemplate, ScopeTemplate};

    fn make_personality() -> Personality {
        Personality {
            id: Uuid::new_v4(),
            metadata: Metadata {
                name: "assistant".to_string(),
                description: "Helpful assistant".to_string(),
                ..Metadata::default()
            },
        }
    }

    fn make_session_template() -> SessionTemplate {
        SessionTemplate {
            id: Uuid::new_v4(),
            metadata: Metadata {
                name: "pairing".to_string(),
                ..Metadata::default()
            },
            scope: ScopeTemplate::GlobalScope,
            channels: vec![ChannelTemplate {
                name: "general".to_string(),
                participants: vec![ParticipantTemplate::User],
            }],
        }
    }

    #[tokio::test]
    async fn template_roundtrip_works() {
        let dir = tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).unwrap();

        let personality = make_personality();
        store.put_personality(&personality).unwrap();

        let found = store.get_personality(personality.id).unwrap();
        assert_eq!(found.metadata.name, personality.metadata.name);

        let session_template = make_session_template();
        store.put_session_template(&session_template).unwrap();
        let found_session = store
            .find_session_template_by_name(
                &session_template.metadata.name,
                &session_template.metadata.scope,
            )
            .unwrap();
        assert_eq!(
            found_session.metadata.name,
            session_template.metadata.name
        );
    }

    #[tokio::test]
    async fn template_list_and_delete_work() {
        let dir = tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).unwrap();

        let mut t1 = make_personality();
        t1.metadata.name = "assistant-a".to_string();
        let mut t2 = make_personality();
        t2.metadata.name = "assistant-b".to_string();

        store.put_personality(&t1).unwrap();
        store.put_personality(&t2).unwrap();

        let templates = store.list_personalities().unwrap();
        assert_eq!(templates.len(), 2);

        assert!(store.delete_personality(t1.id).unwrap());
        assert!(!store.delete_personality(t1.id).unwrap());
        let err = store.get_personality(t1.id).unwrap_err();
        assert!(matches!(err, StoreError::NotFound { entity: "agent_template", .. }));
    }

    #[tokio::test]
    async fn state_roundtrip_works() {
        let dir = tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).unwrap();

        let template = make_personality();
        let agent = Agent::new(
            "assistant-1".to_string(),
            Scope::GlobalScope,
            &template,
        );
        store.create_agent(&agent).unwrap();

        let loaded = store.get_agent(agent.id).unwrap();
        assert_eq!(loaded.metadata.name, agent.metadata.name);

        let state = AgentState::default();
        store.put_agent_state(agent.id, &state).unwrap();
        store.get_agent_state(agent.id).unwrap();
    }

    #[tokio::test]
    async fn create_conflict_and_update_not_found_are_reported() {
        let dir = tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).unwrap();

        let template = make_personality();
        let agent = Agent::new(
            "assistant-1".to_string(),
            Scope::GlobalScope,
            &template,
        );

        store.create_agent(&agent).unwrap();
        let conflict = store.create_agent(&agent).unwrap_err();
        assert!(matches!(conflict, StoreError::Conflict(_)));

        let missing_agent = Agent {
            id: Uuid::new_v4(),
            ..agent
        };
        let not_found = store.update_agent(&missing_agent).unwrap_err();
        assert!(matches!(not_found, StoreError::NotFound { entity: "agent", .. }));
    }
}
