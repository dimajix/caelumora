use crate::channel::ChannelLoop;
use crate::actor::ActorManager;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::component::ComponentRegistry;
use crate::controller::{ChannelController, SessionController};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::approval::ApprovalRule;
use caelumora_model::chain::Node;
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::history::HistoryEntry;
use caelumora_model::message::{Message, MessageId};
use caelumora_model::security::SecurityAttributes;
use caelumora_model::session::SessionId;
use crate::agent::state::AgentStateHandle;
use crate::event::{EventSubscription, SubscriptionTopic};
use crate::view::{SessionView, Viewer};
use caelumora_model::secret::SecretManager;
use caelumora_model::security::{SecurityContext, AccessPolicy};
use caelumora_model::session::Session;
use caelumora_model::store::history::HistoryKey;
use caelumora_model::store::StoreResult;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, Place};
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::security::Grant;
use caelumora_model::workspace::Workspace;
use crate::terminal::{Shell, Terminal};
use crate::workspace::{WorkspaceError, WorkspaceResult};
use std::sync::Arc;
use tracing::debug;
use super::{ChannelRuntime, Services};

/// What an actor runs with (issue 69): the services every actor needs, and
/// access to what concerns this actor alone - its controller, its state, its
/// events, what it asks of its channel. It is the actor's interface: what an
/// actor needs and does not find here is added here (issue 370). It is made
/// for one actor (`ChannelRuntime::new_actor_runtime`) and hands out nothing
/// about others. It is the actor's alone, and its turns': its components get
/// only what they need of it (`ComponentContext`, issue 128) and the turn's
/// methods; what they read of the session they read from the root its work
/// is given (`session_view`, issue 163), as the confidentiality rules allow.
/// The full channel and session are `pub(crate)`, for the user's and the
/// approver's actors and the views.
#[derive(Debug, Clone)]
pub struct ActorRuntime {
    pub(super) runtime: ChannelRuntime,
    pub(super) actor: ActorId,
    pub(super) session_id: SessionId,
    pub(super) channel_id: ChannelId,
}

impl ActorRuntime {
    /// The actor this runtime is for.
    pub fn actor_id(&self) -> ActorId {
        self.actor
    }

    pub fn access_policy(&self) -> &Arc<dyn AccessPolicy> { self.runtime.runtime_manager.access_policy() }

    pub fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> { self.runtime.runtime_manager.confidentiality() }

    pub fn secret_manager(&self) -> &Arc<SecretManager> { self.runtime.runtime_manager.secret_manager() }

    pub fn services(&self) -> &Services { self.runtime.runtime_manager.services() }

    /// Where the actor's outbox is kept across its runs (issue 386).
    pub(crate) fn actor_manager(&self) -> &Arc<ActorManager> { self.runtime.runtime_manager.actor_manager() }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> {
        self.runtime.runtime_manager.component_registry()
    }

    /// What the system restricts of tool calls everywhere (issue 13).
    pub fn approval_rules(&self) -> &Arc<[ApprovalRule]> {
        self.runtime.runtime_manager.approval_rules()
    }

    /// The controller of the actor's session: what its agent's tool calls
    /// are guarded by, beside the channel's (issue 13). Held for as long as
    /// the actor runs, unlike its channel: the guard must not lapse.
    pub fn session_controller(&self) -> &Arc<SessionController> {
        self.runtime.session_controller()
    }

    /// The controller of the actor's channel (issue 374), held as the
    /// session's is: what the actor reads its channel by, and what its
    /// agent's tool calls are guarded by.
    pub fn channel_controller(&self) -> &Arc<ChannelController> {
        self.runtime.channel_controller()
    }

    /// The user commands of the actor's session. Only a `UserActor` runs
    /// them: commands are for people, agents act through tools.
    pub fn command_executor(&self) -> &Arc<CommandExecutor<dyn ChannelCommandProvider>> {
        &self.runtime.command_executor
    }

    /// The attributes of the agent `id`, as stored - what is asked of an
    /// agent named in an event (`ControllerManager::agent_attributes`).
    ///
    /// # Errors
    /// `StoreError::NotFound` if the agent no longer exists, or whatever
    /// else the store reports.
    pub(crate) async fn agent_attributes(&self, id: AgentId) -> StoreResult<SecurityAttributes> {
        self.runtime.runtime_manager.controller_manager().agent_attributes(id).await
    }

    /// The session `id` as stored, as `security` may read it.
    pub async fn load_session(&self, security: &SecurityContext, id: SessionId) -> StoreResult<Session> {
        self.runtime.runtime_manager.entity_stores().sessions.get(security, id).await
    }

    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62); `attributes` are the agent's.
    ///
    /// # Errors
    /// If this actor is no agent, or the state cannot be loaded.
    pub async fn agent_state(&self, attributes: SecurityAttributes) -> Result<Arc<AgentStateHandle>> {
        let agent = self.agent_id()?;
        self.runtime.runtime_manager.agent_states().get(agent, attributes).await
    }

    /// The state of this agent in its channel, as stored - of either kind
    /// (issue 206).
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports.
    pub async fn load_channel_state(&self) -> Result<AgentChannelState> {
        let agent = self.agent_id()?;
        Ok(self.runtime.runtime_manager.state_store().get_agent_channel_state(agent, self.channel_id).await?)
    }

    /// The position in its channel's history of the entry that said the
    /// message `id`; `None` if the channel never heard it (issue 379).
    ///
    /// # Errors
    /// Whatever the `HistoryStore` reports.
    pub async fn channel_position_of(&self, id: MessageId) -> Result<Option<u64>> {
        Ok(self.runtime.runtime_manager.history_store().said(HistoryKey::Channel(self.channel_id), id).await?.map(|(at, _)| at))
    }

    /// The entries of its channel's history from the position `from` on, as
    /// they were added (issue 379): what the actor missed of it.
    ///
    /// # Errors
    /// Whatever the `HistoryStore` reports.
    pub async fn channel_entries(&self, from: u64) -> Result<Vec<Node<HistoryEntry>>> {
        Ok(self.runtime.runtime_manager.history_store().entries(HistoryKey::Channel(self.channel_id), from).await?)
    }

    /// The message `id` as this agent's transcript in its channel holds it,
    /// on whichever branch, with every change of it recorded since (issue
    /// 380): what the remote side of an external agent was given of it, read
    /// from the store, since its actor holds only the active branch. `None`
    /// if the transcript holds it nowhere.
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `TranscriptStore` reports.
    pub async fn transcript_message(&self, id: MessageId) -> Result<Option<Message>> {
        let agent = self.agent_id()?;
        Ok(self.runtime.transcript_store().message(agent, self.channel_id, id).await?)
    }

    /// Stores the state of this agent in its channel, marks it stored
    /// (`StateStore`), and then lets go of what its history holds before
    /// its window (`AgentChannelState::keep_window`, issue 379): the store
    /// has it.
    ///
    /// # Errors
    /// If this actor is no agent, or whatever the `StateStore` reports;
    /// nothing is let go then.
    pub async fn store_channel_state(&self, state: &mut AgentChannelState) -> Result<()> {
        let agent = self.agent_id()?;
        self.runtime.runtime_manager.state_store().put_agent_channel_state(agent, self.channel_id, state).await?;
        state.keep_window();
        Ok(())
    }

    /// The global events about the actor's channel (issue 69): nothing about
    /// other channels can be subscribed to. Changes to the actor's own user
    /// or agent are the session's to take in, into the controller the actor
    /// shares (issue 120).
    pub fn subscribe_channel(&self) -> EventSubscription {
        self.runtime.runtime_manager.event_bus().subscribe(vec![SubscriptionTopic::Channel(self.channel_id)])
    }

    /// Announces that this agent's state in its channel changed and is
    /// stored (issue 65), with the channel's attributes and without content.
    /// Nothing once the channel is gone.
    pub fn announce_channel_state_changed(&self) {
        let (Ok(agent), Some(channel)) = (self.agent_id(), self.channel_loop()) else { return };
        self.runtime.runtime_manager.event_bus()
            .announce(GlobalEvent::custom::<Channel>(&(self.session_id, self.channel_id), channel::AGENT_STATE_CHANGED, &agent), &**channel.channel());
    }

    /// The actor's own session as `context` reads it from the actor's
    /// channel at `classification` (issue 163), if it may read it
    /// (`GetSession`, as `Views::session` asks, issue 304) - an agent by the
    /// group of the session's participants: the root of what its work
    /// navigates, asked in the context of each piece of work (issue 255).
    /// `None` while the channel is still being built or once it or its
    /// session is gone, or if it may not.
    pub async fn session_view(&self, context: &SecurityContext, classification: Classification) -> Option<SessionView> {
        let session_loop = self.channel_loop()?.session_loop()?;
        let from = Place { channel: self.channel_id, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(context.clone(), from));
        match views.session(session_loop.id()).await {
            Ok(session) => Some(session),
            Err(e) => {
                debug!(session = %session_loop.id(), caller = %context.caller_id, "An agent may not read its own session: {e}");
                None
            }
        }
    }

    /// The classification of the actor's channel as it is now, which what
    /// the actor says there is bound by (issues 141, 212): read from its
    /// controller, as it stood last once the channel is gone (issue 374).
    pub fn channel_classification(&self) -> Classification {
        self.channel_controller().load().classification.clone()
    }

    /// The attributes of the actor's channel as it stands, by which an act
    /// in it is asked - which of an agent's commands someone may see or
    /// carry out (issue 269), an act on what an agent keeps there (issue
    /// 312): read from its controller, as `channel_classification`.
    pub fn channel_attributes(&self) -> SecurityAttributes {
        self.channel_controller().load().security_attributes.clone()
    }

    /// `questions` put to the users of the actor's channel, their addressees
    /// resolved as `context` - who caused them - names them (issue 165,
    /// `ChannelLoop::address_questions`); `None` while the channel is still
    /// being built or once it is gone, when there is nobody to ask.
    ///
    /// # Errors
    /// As `Questions::resolve`: an addressee the channel does not know.
    pub fn address_questions(&self, questions: Questions, context: &SecurityContext) -> Option<Result<Questions, QuestionError>> {
        Some(self.channel_loop()?.address_questions(questions, context))
    }

    /// The workspace of the actor's session and its terminal, for what
    /// `need` asks, as `context` may use it in the actor's channel
    /// (`ChannelLoop::workspace_terminal`, issue 344).
    ///
    /// # Errors
    /// `NotUsableInChannel` while the channel is still being built or once
    /// it is gone, or what `ChannelLoop::workspace_terminal` reports.
    pub async fn workspace_terminal(&self, context: &SecurityContext, need: Grant) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let channel = self.channel_loop().ok_or(WorkspaceError::NotUsableInChannel)?;
        channel.workspace_terminal(context, need).await
    }

    /// The workspace of the actor's session and its shell as the actor's
    /// channel sees it, for `context` (`ChannelLoop::workspace_shell`, issue
    /// 344).
    ///
    /// # Errors
    /// As [`Self::workspace_terminal`].
    pub async fn workspace_shell(&self, context: &SecurityContext) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let channel = self.channel_loop().ok_or(WorkspaceError::NotUsableInChannel)?;
        channel.workspace_shell(context).await
    }

    /// The channel the actor is in, or `None` while the channel is still
    /// being built or once it is gone - for the user's and the approver's
    /// actors, which act for a person as the frontends do, and for the
    /// views. What an agent needs of its channel is asked above (issue
    /// 370): the loop's public API acts in a user's name too.
    pub(crate) fn channel_loop(&self) -> Option<Arc<ChannelLoop>> {
        self.runtime.channel_loop.get()
    }

    fn agent_id(&self) -> Result<AgentId> {
        self.actor.as_agent_id().ok_or_else(|| anyhow!("{} is no agent", self.actor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::testing::{bare_channel, bare_session, test_runtime};
    use caelumora_model::user::UserId;

    fn channel_runtime() -> ChannelRuntime {
        let session = bare_session(SessionId::new());
        let channel = bare_channel(session.id);
        ChannelRuntime::outside_a_session(test_runtime(), session, channel)
    }

    #[tokio::test]
    async fn an_actor_runtime_acts_for_its_own_actor_only() {
        let runtime = channel_runtime();
        let hers = runtime.new_actor_runtime(ActorId::User(UserId::new()));

        assert!(hers.load_channel_state().await.is_err(), "a user has no agent state");
        assert!(hers.agent_state(Default::default()).await.is_err());

        let its = runtime.new_actor_runtime(ActorId::Agent(AgentId::new()));
        its.store_channel_state(&mut AgentChannelState::default()).await.expect("its own state in its channel");
        its.load_channel_state().await.expect("and back");
    }
}
