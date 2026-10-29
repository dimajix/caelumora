//! What tests that reach a workspace share: a running session whose channel "main" an agent takes
//! part in, with a workspace attached that its storage gives a shell; and the workspace's terminal
//! and shell in a channel, as a frontend reaches them.
#![expect(clippy::unwrap_used, reason = "test code: what fails here fails the test that runs it")]

use crate::runtime::RuntimeManager;
use crate::session::testing::{make_session, user};
use crate::session::{SessionLoop, SessionManager};
use crate::terminal::{Shell, Terminal};
use crate::testing::TestRuntime;
use crate::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{Agent, AgentId, Personality, PersonalityId};
use caelumora_model::approval::{ApprovalStrategy, Approver};
use caelumora_model::channel::{Channel, ChannelId, InternalActor, Participant};
use caelumora_model::confidentiality::{Classification, Confidentiality, Visibility};
use caelumora_model::config::AppConfig;
use caelumora_model::config::{BackendConfig, WorkspacesConfig};
use caelumora_model::scope::Scope;
use caelumora_model::security::{Grant, SecurityAttributes, SecurityContext};
use caelumora_model::session::Session;
use caelumora_model::store::Persistence;
use caelumora_model::value::name::{Name, ScopedName};
use caelumora_model::workspace::Workspace;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
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
    let storage = BackendConfig::new(
        "local",
        json!({ "root": files.path(), "shell": true, "destination": { "level": "internal" } }),
    );
    let workspaces = WorkspacesConfig {
        storage: [("dev".to_string(), storage)].into(),
        max_file_bytes: MAX_FILE_BYTES,
    };
    let runtime = TestRuntime {
        store: store.clone(),
        config: AppConfig { workspaces, ..Default::default() },
        ..Default::default()
    }
    .build();
    let system = SecurityContext::system();

    let owner = user("owner");
    store.entity_stores().unwrap().users.create(&system, &owner).await.unwrap();
    let owner = owner.id;
    let as_owner =
        SecurityContext::new(ActorId::User(owner), Default::default(), Default::default());
    let workspace = runtime
        .workspace_manager()
        .create(
            &as_owner,
            "docs",
            Scope::UserScope { user_id: owner },
            "dev",
            Some("Drafts"),
            internal.clone(),
        )
        .await
        .unwrap();

    let personality = Personality {
        description: None,
        id: PersonalityId::new(),
        name: ScopedName::new("plain", Default::default()).unwrap(),
        kind: Default::default(),
        roles: None,
        introduction: None,
        components: Default::default(),
        security_attributes: SecurityAttributes::systems(),
        reasoning_visibility: Visibility::Channel,
        hears_during_turn: Default::default(),
        deleted: None,
        revision: Default::default(),
    };
    store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
    let agent = Agent::new(
        ScopedName::new("Jonathan", Scope::GlobalScope).unwrap(),
        personality.id,
        ActorId::User(owner),
    );
    store.entity_stores().unwrap().agents.create(&system, &agent).await.unwrap();
    let session = make_session("s", owner);
    let mut channel = Channel::build(
        session.id,
        ActorId::User(owner),
        Name::new("main").unwrap(),
        Default::default(),
        internal,
        None,
    );
    channel.participants = vec![Participant::User(owner), Participant::Agent(agent.id)];
    channel.internal_actors = vec![InternalActor::Approver(Approver::new(ApprovalStrategy::Any))];
    let session = Session {
        workspace: Some(workspace.id),
        participants: channel.participants.clone(),
        channels: vec![channel.id],
        ..session
    };
    let session = SessionManager::new(Arc::clone(&runtime))
        .create(&as_owner, session, vec![channel])
        .await
        .unwrap();
    let context = SecurityContext::new(
        ActorId::User(owner),
        session.id(),
        session.channels()[0].id(),
    )
    .next(ActorId::Agent(agent.id), ActorId::User(owner), ActorId::User(owner));
    WithWorkspace { files, session, agent: agent.id, context, workspace, runtime }
}

/// The session's workspace and its terminal in channel `channel` of `session`, for what `need`
/// asks, as a frontend reaches them (`WorkspaceController::terminal_in`): of the session and the
/// channel as the running session holds them.
///
/// # Errors
/// `NotUsableInChannel` where no workspace is attached (`SessionLoop::workspace`) or the channel
/// does not run, else what `terminal_in` reports.
pub async fn terminal_in(
    session: &SessionLoop,
    channel: ChannelId,
    ctx: &SecurityContext,
    need: Grant,
) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
    let workspace = session.workspace()?;
    let (session, channel) = documents_of(session, channel)?;
    workspace.terminal_in(ctx, &session, &channel, need).await
}

/// The session's workspace and its shell as channel `channel` of `session` sees it, as a frontend
/// reaches them (`WorkspaceController::shell_in`).
///
/// # Errors
/// As `terminal_in`.
pub async fn shell_in(
    session: &SessionLoop,
    channel: ChannelId,
    ctx: &SecurityContext,
) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
    let workspace = session.workspace()?;
    let (session, channel) = documents_of(session, channel)?;
    workspace.shell_in(ctx, &session, &channel).await
}

/// The session and its channel `channel` as `session` holds them.
///
/// # Errors
/// `NotUsableInChannel` where the channel does not run.
fn documents_of(
    session: &SessionLoop,
    channel: ChannelId,
) -> WorkspaceResult<(Arc<Session>, Arc<Channel>)> {
    let channel = session.channel(channel).ok_or(WorkspaceError::NotUsableInChannel)?;
    Ok((session.session(), channel.channel()))
}
