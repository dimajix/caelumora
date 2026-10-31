use super::{ChannelRuntime, Services};
use crate::actor::ActorManager;
use crate::agent::state::AgentStateHandle;
use crate::attachment::{AttachmentManager, AttachmentResult};
use crate::channel::ChannelLoop;
use crate::command::CommandExecutor;
use crate::component::ComponentRegistry;
use crate::controller::{ChannelController, SessionController, WorkspaceController};
use crate::event::EventSubscription;
use crate::terminal::{Shell, Terminal};
use crate::view::{SessionView, ViewResult, Viewer};
use crate::workspace::{WorkspaceError, WorkspaceResult};
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::{AgentChannelState, AgentId};
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId, RuleDecision};
use caelumora_model::channel::{self, Channel, ChannelId};
use caelumora_model::confidentiality::{
    Classification, ConfidentialityPolicy, ConfidentialityQuestions, Place,
};
use caelumora_model::content::ContentRef;
use caelumora_model::context::ModelProfile;
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::message::{Message, MessageId};
use caelumora_model::prompt_catalogue::PromptCatalogue;
use caelumora_model::question::{QuestionError, Questions};
use caelumora_model::secret::{SecretManager, Secrets};
use caelumora_model::security::{
    AccessPolicy, ActionType, Enforcement, Grant, Reclassified, SecurityAttributes, SecurityContext,
};
use caelumora_model::session::Session;
use caelumora_model::session::SessionId;
use caelumora_model::store::StoreResult;
use caelumora_model::tool::{ToolCall, ToolMedia};
use caelumora_model::value::time_zone::TimeZone;
use caelumora_model::workspace::Workspace;
use chrono::Utc;
use std::sync::Arc;

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
/// approver's actors and the views. What the core decides of the actor's
/// work - a tool call, a flow, a reclassification, the secrets it may read -
/// it decides here, and it lends no policy - but through `services`, which
/// the components are built with, until that is closed (issue 516, step 5).
#[derive(Debug, Clone)]
pub struct ActorRuntime {
    pub(super) runtime: ChannelRuntime,
    pub(super) actor: ActorId,
    pub(super) session_id: SessionId,
    pub(super) channel_id: ChannelId,
}

/// What the core decides of a tool call as it is admitted (`ActorRuntime::admit_tool_call`,
/// issue 516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPermit {
    /// The policy does not let it be called (`CallTool`).
    Forbidden,
    /// A rule of a place denies it.
    Denied(ApprovalRuleId),
    /// A rule of a place asks a person to decide it, whatever its tool says (issue 13).
    Ask,
    /// Nothing of the core stands against it: whether a person is asked is its tool's.
    Open,
}

/// What an agent keeps of its own in its channel, which it may reclassify
/// (`ActorRuntime::may_reclassify`, issue 516).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnRecord {
    /// One of its notes, public or private.
    Note,
    /// An entry of its memory, recalled in the channel.
    Memory,
}

impl ActorRuntime {
    /// The actor this runtime is for.
    pub fn actor_id(&self) -> ActorId {
        self.actor
    }

    /// The access policy of the process - the core's alone, as its other policy and its secrets
    /// are: what an actor of an application needs decided it asks below (issue 516).
    pub(crate) fn access_policy(&self) -> &Arc<dyn AccessPolicy> {
        self.runtime.runtime_manager.access_policy()
    }

    /// The confidentiality policy of the process, by which the flow questions below are decided.
    fn confidentiality(&self) -> &Arc<dyn ConfidentialityPolicy> {
        self.runtime.runtime_manager.confidentiality()
    }

    /// The secrets of the process, which `secrets` binds to a name.
    fn secret_manager(&self) -> &Arc<SecretManager> {
        self.runtime.runtime_manager.secret_manager()
    }

    pub fn services(&self) -> &Services {
        self.runtime.runtime_manager.services()
    }

    /// The texts given to models (issue 57): what the actor's work words
    /// for its model itself - the results of tool calls, a plan's steps.
    pub fn prompts(&self) -> &Arc<PromptCatalogue> {
        self.runtime.runtime_manager.prompts()
    }

    /// Where the actor's outbox is kept across its runs (issue 386).
    pub(crate) fn actor_manager(&self) -> &Arc<ActorManager> {
        self.runtime.runtime_manager.actor_manager()
    }

    /// The owner of attachments (issue 282): what a user's message names of what they uploaded,
    /// what a model is given along.
    pub(crate) fn attachment_manager(&self) -> &Arc<AttachmentManager> {
        self.runtime.runtime_manager.attachment_manager()
    }

    /// Puts their content on the attachments of `messages` - what the actor's model is given of
    /// its transcript - that the model of `profile` is given along (issue 282), and answers how
    /// each is classified. Asking nothing: what lies in the actor's transcript, it heard.
    ///
    /// # Errors
    /// `AttachmentError` if one cannot be read, or is not what its message says.
    pub async fn load_attachments(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>> {
        self.attachment_manager().load(messages, profile).await
    }

    /// Keeps `media`, what a tool gave its model for the result of a call `context` makes in the
    /// actor's channel, classified `classification` as the
    /// result (`AttachmentManager::create_media`, with the channel as it stands, issue 512).
    ///
    /// # Errors
    /// As `AttachmentManager::create_media`.
    pub async fn create_media(
        &self,
        context: &SecurityContext,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        let channel = self.channel();
        self.attachment_manager().create_media(context, &channel, classification, media).await
    }

    pub fn component_registry(&self) -> &Arc<ComponentRegistry> {
        self.runtime.runtime_manager.component_registry()
    }

    /// What the system restricts of tool calls everywhere (issue 13).
    fn approval_rules(&self) -> &Arc<[ApprovalRule]> {
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
    pub fn command_executor(&self) -> &Arc<CommandExecutor> {
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
    pub async fn load_session(
        &self,
        security: &SecurityContext,
        id: SessionId,
    ) -> StoreResult<Session> {
        self.runtime.runtime_manager.entity_stores().sessions.get(security, id).await
    }

    /// The agent's own state, shared with its actors in other channels and
    /// sessions (issue 62); `attributes` are the agent's.
    ///
    /// # Errors
    /// If this actor is no agent, or the state cannot be loaded.
    pub async fn agent_state(
        &self,
        attributes: SecurityAttributes,
    ) -> Result<Arc<AgentStateHandle>> {
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
        Ok(self
            .runtime
            .runtime_manager
            .state_store()
            .get_agent_channel_state(agent, self.channel_id)
            .await?)
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
        self.runtime
            .runtime_manager
            .state_store()
            .put_agent_channel_state(agent, self.channel_id, state)
            .await?;
        state.keep_window();
        Ok(())
    }

    /// The global events about the actor's channel (issue 69): nothing about
    /// other channels can be subscribed to. Changes to the actor's own user
    /// or agent are the session's to take in, into the controller the actor
    /// shares (issue 120).
    pub fn subscribe_channel(&self) -> EventSubscription {
        self.runtime.runtime_manager.event_bus().subscribe_channel(self.channel_id)
    }

    /// Announces that this agent's state in its channel changed and is
    /// stored (issue 65), with the channel's attributes and without content.
    /// Nothing once the channel is gone.
    pub fn announce_channel_state_changed(&self) {
        let (Ok(agent), Some(channel)) = (self.agent_id(), self.channel_loop()) else { return };
        self.runtime.runtime_manager.event_bus().announce(
            GlobalEvent::custom::<Channel>(
                &(self.session_id, self.channel_id),
                channel::AGENT_STATE_CHANGED,
                &agent,
            ),
            &*channel.channel(),
        );
    }

    /// The actor's own session as `context` reads it from the actor's
    /// channel at `classification` (issue 163), if it may read it
    /// (`GetSession`, as `Views::session` asks, issue 304) - an agent by the
    /// group of the session's participants: the root of what its work
    /// navigates, asked in the context of each piece of work (issue 255).
    /// `None` while the channel is still being built or once it or its
    /// session is gone.
    ///
    /// # Errors
    /// As `Views::session`: `context` may not read it, or reading it failed.
    pub async fn session_view(
        &self,
        context: &SecurityContext,
        classification: Classification,
    ) -> ViewResult<Option<SessionView>> {
        let Some(session_loop) = self.channel_loop().and_then(|channel| channel.session_loop())
        else {
            return Ok(None);
        };
        let from =
            Place { channel: self.channel_id, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(context.clone(), from));
        views.session(session_loop.id()).await.map(Some)
    }

    /// The classification of the actor's channel as it is now, which what
    /// the actor says there is bound by (issues 141, 212): read from its
    /// controller, as it stood last once the channel is gone (issue 374).
    pub fn channel_classification(&self) -> Classification {
        self.channel().classification.clone()
    }

    /// The time zone of the actor's session as it is now (issue 424) - UTC
    /// where it names none -, which an agent's model is given the times of
    /// messages in: read from its controller, as `channel_classification`.
    pub fn session_time_zone(&self) -> TimeZone {
        self.session_controller().load().effective_time_zone()
    }

    /// `questions` put to the users of the actor's channel, their addressees
    /// resolved as `context` - who caused them - names them (issue 165,
    /// `ChannelLoop::address_questions`); `None` while the channel is still
    /// being built or once it is gone, when there is nobody to ask.
    ///
    /// # Errors
    /// As `Questions::resolve`: an addressee the channel does not know.
    pub fn address_questions(
        &self,
        questions: Questions,
        context: &SecurityContext,
    ) -> Option<Result<Questions, QuestionError>> {
        Some(self.channel_loop()?.address_questions(questions, context))
    }

    /// How a call of `call`, made in `context` by the actor's agent, is admitted (issue 516):
    /// `Forbidden` unless the policy lets it be called (`CallTool`, the call's one question of
    /// permission, issue 304); else as the strongest rule of the system, the actor's session and
    /// its channel decides it now (`ApprovalRule::decide`, issue 13). Whether a person is asked
    /// beyond what a rule asks is its tool's, as the agent's owner configured it.
    pub async fn admit_tool_call(&self, context: &SecurityContext, call: &ToolCall) -> CallPermit {
        let call_tool = ActionType::CallTool(call);
        if self.access_policy().require(context, call_tool, |_| ()).await.is_err() {
            return CallPermit::Forbidden;
        }
        match self.place_rule(call) {
            Some((rule, RuleDecision::Deny)) => CallPermit::Denied(rule),
            Some((_, RuleDecision::Ask)) => CallPermit::Ask,
            Some((_, RuleDecision::Allow)) | None => CallPermit::Open,
        }
    }

    /// The rule of a place that denies `call` now, if one does (issue 13): asked again as an
    /// admitted call is run, since one may have been set while it waited. The policy is not asked
    /// again (issue 304).
    pub fn rule_denying(&self, call: &ToolCall) -> Option<ApprovalRuleId> {
        match self.place_rule(call)? {
            (rule, RuleDecision::Deny) => Some(rule),
            (_, RuleDecision::Ask | RuleDecision::Allow) => None,
        }
    }

    /// The rule of a place that decides `call` now, if one does, with what it decides of it: the
    /// strongest of the system's, the session's and the channel's.
    fn place_rule(&self, call: &ToolCall) -> Option<(ApprovalRuleId, RuleDecision)> {
        let session = self.session_controller().load();
        let channel = self.channel_controller().load();
        let system = self.approval_rules().iter();
        let rules = system.chain(&session.approval_rules).chain(&channel.approval_rules);
        ApprovalRule::decide(rules, call, session.id, channel.id, Utc::now())
            .map(|(rule, decision)| (rule.id, decision))
    }

    /// Whether what is classified `classification` may flow, in `context`, into what is
    /// classified `into` (`ConfidentialityPolicy::may_publish`, issue 516): a tool's result into
    /// the turn, the channel to a tool set's remote side, what an external agent brings into the
    /// channel, what a turn saw into the channel as it is now.
    pub fn may_publish(
        &self,
        context: &SecurityContext,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        self.confidentiality().may_publish(context, classification, into)
    }

    /// Whether what lies at `from` may be copied, in `context`, to `into`
    /// (`ConfidentialityQuestions::may_copy_information`, issue 516): what an agent may recall
    /// of what it keeps of its own, written elsewhere.
    pub fn may_copy_information(
        &self,
        context: &SecurityContext,
        from: &Place,
        into: &Place,
    ) -> bool {
        self.confidentiality().may_copy_information(context, from, into)
    }

    /// Whether `record` - what the actor's agent keeps of its own in its channel - may be
    /// reclassified, in `context`, as `classification`, within what is classified `within`
    /// (issues 133, 516): `Reclassify` of it, by the agent as its author, with the channel's
    /// attributes as it stands, then the flow (`require_publish`).
    pub async fn may_reclassify(
        &self,
        context: &SecurityContext,
        record: OwnRecord,
        classification: &Classification,
        within: &Classification,
    ) -> bool {
        let (author, attributes) = (self.actor, self.channel().security_attributes.clone());
        let target = match record {
            OwnRecord::Note => Reclassified::Note { author, attributes: &attributes },
            OwnRecord::Memory => Reclassified::Memory { author, attributes: &attributes },
        };
        let reclassify = ActionType::Reclassify(target);
        self.access_policy().require(context, reclassify, |_| ()).await.is_ok()
            && self
                .confidentiality()
                .require_publish(context, classification, within, || ())
                .is_ok()
    }

    /// The secrets that may be read in the name of `context` (issue 516), each read asked of the
    /// policy then (`ReadSecret`): what a component that talks to a remote system is handed for
    /// its credentials, shared, as what follows a remote task after a call reads them too.
    pub fn secrets(&self, context: &SecurityContext) -> Arc<dyn Secrets> {
        Arc::new(self.secret_manager().for_context(context.clone()))
    }

    /// `attachment`, one of the actor's channel, copied in `context` for a message the actor will
    /// say there (`AttachmentManager::copy`, issue 282).
    ///
    /// # Errors
    /// As `AttachmentManager::copy`.
    pub async fn copy_attachment(
        &self,
        context: &SecurityContext,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        self.attachment_manager().copy(context, &self.channel(), attachment).await
    }

    /// The largest file that may be put into a workspace, as the process is configured
    /// (`WorkspaceManager::max_file_bytes`).
    pub fn max_file_bytes(&self) -> u64 {
        self.runtime.runtime_manager.workspace_manager().max_file_bytes()
    }

    /// The actor's channel as it stands, read from its controller - as it stood last once the
    /// channel is gone (issue 374): what `channel_classification` is read
    /// from, and what an operation in the channel is given as the channel's properties
    /// (`AttachmentManager::copy`).
    pub fn channel(&self) -> Arc<Channel> {
        Arc::clone(&self.channel_controller().load())
    }

    /// The workspace of the actor's session and its terminal, for what `need` asks, as `context`
    /// may use it in the actor's channel (`WorkspaceController::terminal_in`, issue 344): asked by
    /// the workspace the session holds, of the session and the channel as they are now.
    ///
    /// # Errors
    /// `NotUsableInChannel` while the channel is still being built or once it or its session is
    /// gone, or no workspace is attached to the session; else what `terminal_in` reports.
    pub async fn workspace_terminal(
        &self,
        context: &SecurityContext,
        need: Grant,
    ) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let (session, workspace) = self.session_workspace()?;
        workspace.terminal_in(context, &session, &self.channel(), need).await
    }

    /// The workspace of the actor's session and its shell as the actor's channel sees it - the
    /// commands started in that channel alone -, for `context` (`WorkspaceController::shell_in`,
    /// issue 344).
    ///
    /// # Errors
    /// As [`Self::workspace_terminal`].
    pub async fn workspace_shell(
        &self,
        context: &SecurityContext,
    ) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let (session, workspace) = self.session_workspace()?;
        workspace.shell_in(context, &session, &self.channel()).await
    }

    /// The actor's session as it is now and the workspace it holds.
    ///
    /// # Errors
    /// `NotUsableInChannel` while the channel is still being built or once it or its session is
    /// gone, or no workspace is attached to the session (`SessionLoop::workspace`).
    fn session_workspace(&self) -> WorkspaceResult<(Arc<Session>, Arc<WorkspaceController>)> {
        let session_loop = self
            .channel_loop()
            .and_then(|channel| channel.session_loop())
            .ok_or(WorkspaceError::NotUsableInChannel)?;
        let workspace = session_loop.workspace()?;
        Ok((session_loop.session(), workspace))
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
    use crate::session::testing::test_runtime;
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::session::testing::bare_session;
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
        assert!(hers
            .agent_state(SecurityAttributes::owned_by(ActorId::User(UserId::new())))
            .await
            .is_err());

        let its = runtime.new_actor_runtime(ActorId::Agent(AgentId::new()));
        its.store_channel_state(&mut AgentChannelState::default())
            .await
            .expect("its own state in its channel");
        its.load_channel_state().await.expect("and back");
    }
}
