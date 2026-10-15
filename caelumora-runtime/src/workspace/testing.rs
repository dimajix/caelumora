//! What the tests of the components that work on a workspace share: a
//! running session whose channel "main" an agent takes part in, with a
//! workspace attached that its storage gives a shell.

use crate::testing::TestRuntime;
use crate::runtime::RuntimeManager;
use crate::session::SessionLoop;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::config::{BackendConfig, WorkspacesConfig};
use serde_json::json;
use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
use caelumora_model::scope::Scope;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::user::UserId;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::workspace::Workspace;
use std::sync::Arc;
use caelumora_model::config::AppConfig;
use chrono::Utc;
use std::path::PathBuf;
use tempfile::TempDir;

/// The largest file the workspace takes.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub struct WithWorkspace {
    /// Where the storage keeps the workspace's files.
    pub files: TempDir,
    pub session: Arc<SessionLoop>,
    pub agent: AgentId,
    /// What the agent acts in: for the workspace's owner.
    pub context: SecurityContext,
    pub workspace: Workspace,
    /// What it runs on: where skills are made (issue 22).
    pub runtime: Arc<RuntimeManager>,
}

impl WithWorkspace {

    /// Where the workspace's files lie on the host.
    pub fn dir(&self) -> PathBuf {
        self.files.path().join(self.workspace.id.to_string())
    }
}

pub async fn session_with_workspace() -> WithWorkspace {
    let store = Arc::new(MemoryStore::new());
    let files = tempfile::tempdir().unwrap();
    let internal = Classification::general(Confidentiality::Internal);
    let storage = BackendConfig::new("local", json!({ "root": files.path(), "shell": true, "destination": { "level": "internal" } }));
    let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
    let runtime = TestRuntime { store: Arc::clone(&store), config: AppConfig { workspaces, ..Default::default() }, ..Default::default() }.build();
    let system = SecurityContext::system();

    let owner = UserId::new();
    let as_owner = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
    let workspace = runtime.workspace_manager().create(&as_owner, "docs", Scope::UserScope { user_id: owner }, "dev", Some("Drafts"), internal.clone()).await.unwrap();

    let personality = Personality {
        description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
        kind: Default::default(), roles: None, introduction: None, components: Default::default(),
        security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, deleted: None,
    };
    store.entity_stores().personalities.create(&system, &personality).await.unwrap();
    let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), personality.id, ActorId::User(owner));
    store.entity_stores().agents.create(&system, &agent).await.unwrap();
    let session_id = SessionId::new();
    let channel = Channel::build(session_id, ActorId::User(owner), Name::new("main").unwrap(), Default::default(), internal, None, vec![Participant::User(owner), Participant::Agent(agent.id)]);
    let now = Utc::now();
    let session = Session {
        summary: None, briefing: None,
        approval_rules: Vec::new(),
        workspace: Some(workspace.id),
        id: session_id, name: Name::new("s").unwrap(), participants: channel.participants.clone(), channels: vec![channel.id],
        commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now, security_attributes: Default::default(),
        deleted: None,
    };
    store.entity_stores().sessions.create(&system, &session).await.unwrap();
    store.entity_stores().channels.create(&system, &channel).await.unwrap();
    let session = SessionLoop::new(session, vec![channel], Arc::clone(&runtime)).await.unwrap();
    let context = SecurityContext::new(ActorId::User(owner), session_id, session.channels()[0].id()).next(ActorId::Agent(agent.id), ActorId::User(owner), ActorId::User(owner));
    WithWorkspace { files, session, agent: agent.id, context, workspace, runtime }
}
