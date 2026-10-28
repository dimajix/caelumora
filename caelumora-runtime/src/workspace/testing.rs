//! What the tests of the components that work on a workspace share: a
//! running session whose channel "main" an agent takes part in, with a
//! workspace attached that its storage gives a shell.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use crate::testing::TestRuntime;
use crate::runtime::RuntimeManager;
use crate::session::{SessionLoop, SessionManager};
use crate::session::testing::user;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::approval::{ApprovalStrategy, Approver};
use caelumora_model::channel::{Channel, InternalActor, Participant};
use caelumora_model::config::{BackendConfig, WorkspacesConfig};
use serde_json::json;
use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
use caelumora_model::scope::Scope;
use caelumora_model::security::SecurityContext;
use caelumora_model::project::ProjectId;
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::Persistence;
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

/// A session of a stored user with the channel "main" of the user and an agent, the workspace
/// "docs" of the user attached, made as the runtime makes a session (`SessionManager::create`,
/// issue 486) and running.
pub async fn session_with_workspace() -> WithWorkspace {
    let store = Persistence::in_memory();
    let files = tempfile::tempdir().unwrap();
    let internal = Classification::general(Confidentiality::Internal);
    let storage = BackendConfig::new("local", json!({ "root": files.path(), "shell": true, "destination": { "level": "internal" } }));
    let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
    let runtime = TestRuntime { store: store.clone(), config: AppConfig { workspaces, ..Default::default() }, ..Default::default() }.build();
    let system = SecurityContext::system();

    let owner = user("owner");
    store.entity_stores().unwrap().users.create(&system, &owner).await.unwrap();
    let owner = owner.id;
    let as_owner = SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
    let workspace = runtime.workspace_manager().create(&as_owner, "docs", Scope::UserScope { user_id: owner }, "dev", Some("Drafts"), internal.clone()).await.unwrap();

    let personality = Personality {
        description: None, id: PersonalityId::new(), name: ScopedName::new("plain", Default::default()).unwrap(),
        kind: Default::default(), roles: None, introduction: None, components: Default::default(),
        security_attributes: Default::default(), reasoning_visibility: Visibility::Channel, hears_during_turn: Default::default(), deleted: None,
    };
    store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
    let agent = Agent::new(ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(), personality.id, ActorId::User(owner));
    store.entity_stores().unwrap().agents.create(&system, &agent).await.unwrap();
    let session_id = SessionId::new();
    let mut channel = Channel::build(session_id, ActorId::User(owner), Name::new("main").unwrap(), Default::default(), internal, None);
    channel.participants = vec![Participant::User(owner), Participant::Agent(agent.id)];
    channel.internal_actors = vec![InternalActor::Approver(Approver::new(ApprovalStrategy::Any))];
    let now = Utc::now();
    let session = Session {
        summary: None, briefing: None, language: None, time_zone: None,
        approval_rules: Vec::new(),
        workspace: Some(workspace.id),
        // Replaced by its own, which `create` makes with it.
        project: ProjectId::new(),
        id: session_id, name: Name::new("s").unwrap(), participants: channel.participants.clone(), channels: vec![channel.id],
        commands: Default::default(), privacy: Default::default(), created_at: now, updated_at: now,
        security_attributes: Session::attributes_of(session_id, owner, &[]),
        deleted: None,
    };
    let session = SessionManager::new(Arc::clone(&runtime)).create(&as_owner, session, vec![channel]).await.unwrap();
    let context = SecurityContext::new(ActorId::User(owner), session_id, session.channels()[0].id()).next(ActorId::Agent(agent.id), ActorId::User(owner), ActorId::User(owner));
    WithWorkspace { files, session, agent: agent.id, context, workspace, runtime }
}
