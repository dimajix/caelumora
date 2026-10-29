//! The `CommandReaper` (issues 254, 344): ends the commands of the storages
//! that nobody may run any longer, and those of a stopped session's
//! channels.
//!
//! A command in a workspace is its channel's - the scope of its shell - and
//! runs on only as long as some current participant of the channel could start
//! it there, as `WorkspaceController::shell_in` asks: a task ends every
//! other command its storage lists, at each event that can change the
//! answer and besides every `COMMANDS_CHECKED_EVERY`.

use crate::backend::NamedBackends;
use crate::component::{ComponentError, ComponentResult};
use crate::controller::ControllerManager;
use crate::event::EventBus;
use crate::terminal::{CommandKey, TerminalProvider};
use crate::workspace::{WorkspaceError, WorkspaceResult};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::Agent;
use caelumora_model::channel::{self, Participant};
use caelumora_model::event::{EntityChange, GlobalEvent};
use caelumora_model::security::{Grant, SecurityContext};
use caelumora_model::session::{Channel, ChannelId, Session};
use caelumora_model::store::{EntityStore, EntityStores, StoreResultExt, Stored};
use caelumora_model::user::User;
use caelumora_model::workspace::Workspace;
use futures::future::join_all;
use std::fmt;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time;
use tokio_util::task::AbortOnDropHandle;
use tracing::{info, warn};
use uuid::Uuid;

/// How often the commands of the storages are checked besides the events
/// that can change who may run them (issue 254).
const COMMANDS_CHECKED_EVERY: Duration = Duration::from_secs(60);

/// The scope of the shell a workspace has in channel `id`: a command is its
/// channel's (issues 251, 296).
pub(crate) fn scope_of(id: ChannelId) -> String {
    id.to_string()
}

/// The channel scope `scope` is; `None` for one that is no channel's.
fn channel_of(scope: &str) -> Option<ChannelId> {
    scope.parse::<Uuid>().ok().map(ChannelId::from)
}

pub(crate) struct CommandReaper {
    sessions: Arc<dyn EntityStore<Session>>,
    channels: Arc<dyn EntityStore<Channel>>,
    /// Where the contexts of a channel's participants are built, with their
    /// roles as they are now (issue 304), and the workspaces asked.
    controllers: Arc<ControllerManager>,
    /// The storages, by name, whose commands it ends.
    storages: Arc<NamedBackends<dyn TerminalProvider>>,
    /// The task that ends the commands nobody may run any longer; ends with
    /// the reaper.
    checker: OnceLock<AbortOnDropHandle<()>>,
}

impl fmt::Debug for CommandReaper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandReaper").field("storages", &self.storages).finish_non_exhaustive()
    }
}

impl CommandReaper {
    /// The reaper of the commands of `storages`, asking what `stores` hold
    /// and `controllers` give. Where one of the storages has a shell, it
    /// starts the task that ends the commands nobody may run any longer,
    /// woken by `events`.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` for a storage with a shell outside a
    /// Tokio runtime.
    pub(crate) fn new(
        stores: &EntityStores,
        events: &EventBus,
        controllers: Arc<ControllerManager>,
        storages: Arc<NamedBackends<dyn TerminalProvider>>,
    ) -> ComponentResult<Arc<Self>> {
        let with_shell = storages.iter().any(|(_, provider)| provider.has_shell());
        let reaper = Arc::new(Self {
            sessions: Arc::clone(&stores.sessions),
            channels: Arc::clone(&stores.channels),
            controllers,
            storages,
            checker: OnceLock::new(),
        });
        if with_shell {
            let runtime = Handle::try_current().map_err(|_| {
                ComponentError::InvalidConfig(
                    "a storage of workspaces with a shell needs a running runtime".to_string(),
                )
            })?;
            let subscription = events.subscribe_all().into_receiver();
            let _ = reaper.checker.set(AbortOnDropHandle::new(
                runtime.spawn(check_commands(Arc::downgrade(&reaper), subscription)),
            ));
        }
        Ok(reaper)
    }

    /// Ends every command started in one of `channels`, with what it
    /// started, without asking anything: what stops a session ends what
    /// runs in its channels (issue 254).
    pub(crate) async fn end_commands_in(&self, channels: &[ChannelId]) {
        let scopes: Vec<String> = channels.iter().copied().map(scope_of).collect();
        for (_, provider) in self.storages.iter() {
            let ending = provider
                .commands()
                .into_iter()
                .filter(|(_, scope, _)| scopes.contains(scope))
                .collect();
            end_commands(provider.as_ref(), ending, "its session stopped").await;
        }
    }

    /// Ends, as the process ends, what the storages ran that does not
    /// outlive it (issue 297): each storage decides what that is
    /// (`TerminalProvider::shutdown`).
    pub(crate) async fn shutdown(&self) {
        join_all(self.storages.iter().map(|(_, provider)| provider.shutdown())).await;
    }

    /// Ends every command of the storages that nobody may run any longer
    /// where it was started (issue 254). One whose question cannot be
    /// answered now runs on until it can.
    async fn end_unusable_commands(&self) {
        for (_, provider) in self.storages.iter() {
            let mut ending = Vec::new();
            for key in provider.commands() {
                match self.may_run(&key).await {
                    Ok(true) => {}
                    Ok(false) => ending.push(key),
                    Err(e) => {
                        warn!(workspace = %key.0, command = %key.2, "Whether a command may still run could not be asked: {e}")
                    }
                }
            }
            end_commands(
                provider.as_ref(),
                ending,
                "nobody may run it any longer where it was started",
            )
            .await;
        }
    }

    /// Whether some current participant of the channel command `key` was started
    /// in - its scope - could start it there, as `shell_in` asks: a command
    /// is its channel's, not its starter's (issue 254). The session and the
    /// channel are read as they are stored now, in the runtime's name; asked
    /// in each participant's name, with the runtime as initiator: it asks on its
    /// own.
    ///
    /// # Errors
    /// What the store reports.
    async fn may_run(&self, (workspace, scope, _): &CommandKey) -> WorkspaceResult<bool> {
        let system = SecurityContext::system();
        let Some(channel_id) = channel_of(scope) else { return Ok(false) };
        let Some(channel) = self.channels.get(&system, channel_id).await.optional()? else {
            return Ok(false);
        };
        let Some(session) = self.sessions.get(&system, channel.session_id).await.optional()? else {
            return Ok(false);
        };
        let Some(workspace) = self.controllers.workspace_controller(*workspace).await.optional()?
        else {
            return Ok(false);
        };
        for participant in &channel.participants {
            let member = match participant {
                // One whose record is gone asks with no roles, as before.
                Participant::User(user) => match self
                    .controllers
                    .user_controller(*user)
                    .await
                    .optional()?
                {
                    Some(controller) => controller.security_context(channel.session_id, channel.id),
                    None => {
                        SecurityContext::new(ActorId::User(*user), channel.session_id, channel.id)
                    }
                },
                Participant::Agent(agent) => {
                    match self.controllers.agent_controller(*agent).await.optional()? {
                        Some(agent) => {
                            let personality = self
                                .controllers
                                .personality_controller(agent.load().personality_id)
                                .await
                                .optional()?;
                            let personality = personality.as_ref().map(|p| p.load());
                            agent.security_context(
                                channel.session_id,
                                channel.id,
                                personality.as_ref().and_then(|p| p.limiting_roles()),
                            )
                        }
                        None => continue,
                    }
                }
            };
            let ctx = SecurityContext { initiator_id: ActorId::System, ..member };
            match workspace.usable_in_channel(&ctx, &session, &channel, Grant::Write).await {
                Ok(_) => return Ok(true),
                Err(WorkspaceError::NotUsableInChannel | WorkspaceError::ReadOnly { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }
}

/// Ends the commands `keys` of `provider`, for the reason `why`; one that
/// cannot be ended runs on, logged.
async fn end_commands(provider: &dyn TerminalProvider, keys: Vec<CommandKey>, why: &str) {
    for key in keys {
        info!(workspace = %key.0, command = %key.2, "Ending a command: {why}");
        if let Err(e) = provider.end_command(&key).await {
            warn!(workspace = %key.0, command = %key.2, "A command to end could not be ended: {e}");
        }
    }
}

/// Ends, at every event that can change who may use a workspace in a
/// channel and besides every `COMMANDS_CHECKED_EVERY`, the commands nobody
/// may run any longer (issue 254), until `reaper` or the bus is gone.
/// Events it fell behind on are a reason to check, as any is.
async fn check_commands(reaper: Weak<CommandReaper>, mut events: broadcast::Receiver<GlobalEvent>) {
    let mut every = time::interval(COMMANDS_CHECKED_EVERY);
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) if !changes_who_may_run(&event) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            _ = every.tick() => {}
        }
        let Some(reaper) = reaper.upgrade() else { return };
        reaper.end_unusable_commands().await;
    }
}

/// Whether `event` can change who may run a command in a channel: the
/// session, the channel, its participants, the workspace or who someone is.
fn changes_who_may_run(event: &GlobalEvent) -> bool {
    match event.change {
        EntityChange::Updated(_) | EntityChange::Deleted => {
            [Session::KIND, Channel::KIND, Agent::KIND, User::KIND, Workspace::KIND]
                .contains(&event.kind)
        }
        EntityChange::Custom { name: channel::LEFT, .. } => event.kind == Channel::KIND,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Shell;
    use crate::workspace::testing::{session_with_workspace, shell_in, WithWorkspace};
    use caelumora_model::channel::ChannelUpdateEvent;
    use caelumora_model::workspace::CommandStatus;

    /// A command started by the agent in the session's channel, still
    /// running, with its shell.
    async fn running(w: &WithWorkspace) -> (Arc<dyn Shell>, String) {
        let main = w.session.channels()[0].id();
        let (_, shell) = shell_in(&w.session, main, &w.context).await.unwrap();
        let shell = shell.expect("its storage gives a shell");
        let id = shell.start("sleep 30").await.unwrap();
        (shell, id)
    }

    /// Whether the channel uses the workspace.
    async fn use_workspace(w: &WithWorkspace, uses: bool) -> Channel {
        let mut channel = w
            .runtime
            .entity_stores()
            .channels
            .get(&SecurityContext::system(), w.session.channels()[0].id())
            .await
            .unwrap();
        channel.uses_workspace = uses;
        w.runtime
            .entity_stores()
            .channels
            .update(&SecurityContext::system(), &channel)
            .await
            .unwrap();
        channel
    }

    #[tokio::test]
    async fn a_command_runs_on_while_its_channel_may_use_the_workspace_and_ends_once_it_may_not() {
        let w = session_with_workspace().await;
        let reaper = w.runtime.command_reaper();
        let (shell, id) = running(&w).await;

        reaper.end_unusable_commands().await;
        assert_eq!(
            shell.output(&id, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Running,
            "the channel may still use it"
        );

        use_workspace(&w, false).await;
        reaper.end_unusable_commands().await;
        assert_eq!(
            shell.output(&id, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Killed,
            "the channel no longer uses the workspace"
        );
    }

    #[tokio::test]
    async fn a_command_runs_on_as_long_as_some_participant_could_start_it_there() {
        let w = session_with_workspace().await;
        let reaper = w.runtime.command_reaper();
        let (shell, id) = running(&w).await;
        let mut channel = w
            .runtime
            .entity_stores()
            .channels
            .get(&SecurityContext::system(), w.session.channels()[0].id())
            .await
            .unwrap();

        channel.participants.retain(|participant| !matches!(participant, Participant::Agent(_)));
        w.runtime
            .entity_stores()
            .channels
            .update(&SecurityContext::system(), &channel)
            .await
            .unwrap();
        reaper.end_unusable_commands().await;
        assert_eq!(
            shell.output(&id, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Running,
            "the agent that started it left, the owner may still start it there"
        );

        channel.participants.clear();
        w.runtime
            .entity_stores()
            .channels
            .update(&SecurityContext::system(), &channel)
            .await
            .unwrap();
        reaper.end_unusable_commands().await;
        assert_eq!(
            shell.output(&id, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Killed,
            "nobody left who could"
        );
    }

    #[tokio::test]
    async fn a_shutdown_ends_the_commands_that_would_outlive_the_process() {
        let w = session_with_workspace().await;
        let (shell, id) = running(&w).await;
        w.runtime.command_reaper().shutdown().await;
        assert_eq!(
            shell.output(&id, 0, Duration::ZERO).await.unwrap().status,
            CommandStatus::Killed,
            "a `local` shell's, whose process nothing would know of after a restart (issue 297)"
        );
    }

    #[tokio::test]
    async fn a_change_announced_ends_a_command_at_once() {
        let w = session_with_workspace().await;
        let (shell, id) = running(&w).await;
        let channel = use_workspace(&w, false).await;
        w.runtime.event_bus().publish(
            GlobalEvent::updated::<Channel>(
                &(channel.session_id, channel.id),
                &ChannelUpdateEvent::UsesWorkspaceChanged(false),
            ),
            channel.security_attributes.clone(),
        );
        let ended = shell.output(&id, 0, Duration::from_secs(10)).await.unwrap();
        assert_eq!(ended.status, CommandStatus::Killed, "not only at the next round");
    }
}
