//! What the tests of the components that work on a workspace share: a
//! running session whose channel "main" an agent takes part in, with a
//! workspace attached that its storage gives a shell.

use crate::agent::state::AgentStateHandle;
use crate::agent::turn::Turn;
use crate::runtime::RuntimeManager;
use crate::session::SessionLoop;
use crate::session::testing::actor_runtime_in;
use caelumora_model::actor::{ActorId, EventContext};
use caelumora_model::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
use caelumora_model::channel::{Channel, Participant};
use caelumora_model::config::{BackendConfig, WorkspacesConfig};
use serde_json::json;
use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
use caelumora_model::scope::Scope;
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{AllowAllPolicy, SecurityContext};
use caelumora_model::session::{Session, SessionId};
use caelumora_model::store::memory::MemoryStore;
use caelumora_model::user::UserId;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::workspace::Workspace;
use std::sync::Arc;
use caelumora_model::confidentiality::ConfidentialityRules;
use caelumora_model::config::AppConfig;
use crate::testing::test_registry;

/// The largest file the workspace takes.
pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;

pub(crate) struct WithWorkspace {
    /// Where the storage keeps the workspace's files.
    pub(crate) files: tempfile::TempDir,
    pub(crate) session: Arc<SessionLoop>,
    pub(crate) agent: AgentId,
    /// What the agent acts in: for the workspace's owner.
    pub(crate) context: SecurityContext,
    pub(crate) workspace: Workspace,
    /// What it runs on: where skills are made (issue 22).
    pub(crate) runtime: Arc<RuntimeManager>,
}

impl WithWorkspace {
    /// A turn of the agent in the channel, begun in `state`.
    pub(crate) fn turn<'a>(&self, state: &'a mut AgentChannelState) -> Turn<'a> {
        let channel = &self.session.channels()[0];
        state.begin_turn(EventContext::new(self.session.id(), channel.id(), ActorId::None, String::new()), self.context.clone(), channel.classification(), Vec::new(), Default::default());
        Turn::of(self.agent, "Jonathan".to_string(), state, AgentStateHandle::detached(), actor_runtime_in(&self.session, channel, ActorId::Agent(self.agent))).expect("a turn was begun")
    }

    /// Where the workspace's files lie on the host.
    pub(crate) fn dir(&self) -> std::path::PathBuf {
        self.files.path().join(self.workspace.id.to_string())
    }
}

pub(crate) async fn session_with_workspace() -> WithWorkspace {
    let store = Arc::new(MemoryStore::new());
    let access_policy = Arc::new(AllowAllPolicy::new());
    let secrets = Arc::new(SecretManager::new(vec![], Arc::<AllowAllPolicy>::clone(&access_policy)));
    let files = tempfile::tempdir().unwrap();
    let internal = Classification::general(Confidentiality::Internal);
    let storage = BackendConfig::new("local", json!({ "root": files.path(), "shell": true, "destination": { "level": "internal" } }));
    let workspaces = WorkspacesConfig { storage: [("dev".to_string(), storage)].into(), max_file_bytes: MAX_FILE_BYTES };
    let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, access_policy, Arc::new(ConfidentialityRules::default()), &AppConfig { workspaces, ..Default::default() }, test_registry(Arc::<MemoryStore>::clone(&store))).unwrap());
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
    let now = chrono::Utc::now();
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
