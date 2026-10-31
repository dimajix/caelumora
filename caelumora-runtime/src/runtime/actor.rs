use super::{ChannelRuntime, Services};
use crate::actor::ActorManager;
use crate::agent::state::AgentStateHandle;
use crate::attachment::{AttachmentManager, AttachmentResult};
use crate::channel::{ChannelLoop, ResolvedParticipant};
use crate::command::CommandExecutor;
use crate::component::{ComponentContext, ComponentRegistry};
use crate::controller::{ChannelController, SessionController, WorkspaceController};
use crate::event::EventSubscription;
use crate::terminal::{Shell, Terminal};
use crate::view::{SessionReader, ViewResult, Viewer};
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
/// for one participant (`ChannelRuntime::participant_runtime`) and hands out nothing
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
    /// In whose name it acts, which the core builds into a context at every call (issue 516).
    pub(super) acting: Acting,
}

/// How the context an actor's runtime acts in is built, at every call that asks the core
/// (`ActorRuntime::acting_for`, issue 516).
#[derive(Debug, Clone)]
pub(crate) enum Acting {
    /// A participant of the channel, as the channel resolved it: its context as the controllers
    /// hold it now - a user's own, an agent's with its owner's rights and roles, limited by its
    /// personality, the approver's answered for by the channel's owner.
    Participant(ResolvedParticipant),
    /// In a test, an actor that takes part in nothing: in its own name, without roles.
    #[cfg(any(test, feature = "test-support"))]
    Itself,
    /// In a test, the context it gives (`ActorRuntime::acting_as`).
    #[cfg(any(test, feature = "test-support"))]
    Fixed(SecurityContext),
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

    /// The context the actor acts in now, for `initiator` - who began the chain it answers, for
    /// the record, never for rights (issue 516): built by the core at every call, from what it
    /// holds of the actor (`Acting`), never handed in by the actor.
    pub(crate) fn acting_for(&self, initiator: ActorId) -> SecurityContext {
        match &self.acting {
            Acting::Participant(participant) => participant.context_in(&self.channel(), initiator),
            #[cfg(any(test, feature = "test-support"))]
            Acting::Itself => SecurityContext {
                initiator_id: initiator,
                ..SecurityContext::new(self.actor, self.session_id, self.channel_id)
            },
            #[cfg(any(test, feature = "test-support"))]
            Acting::Fixed(context) => context.clone(),
        }
    }

    /// This runtime, acting in `context` whatever its actor and its initiator are: for a test
    /// that gives the work of an actor the rights it needs without making it a participant
    /// (issue 516).
    #[cfg(any(test, feature = "test-support"))]
    pub fn acting_as(self, context: SecurityContext) -> Self {
        Self { acting: Acting::Fixed(context), ..self }
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

    /// The context the actor's components are built in (issue 321): in the actor's own name, as
    /// the core holds it now, with the runtime's services (issue 516). What a component reads
    /// while it is built it reads in it; what it does later, in its turn's.
    pub fn component_context(&self) -> ComponentContext {
        ComponentContext::new(self.acting_for(self.actor), self.services().clone())
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

    /// Keeps `media`, what a tool gave its model for the result of a call the actor makes in its
    /// channel for `initiator`, classified `classification` as the
    /// result (`AttachmentManager::create_media`, with the channel as it stands, issue 512).
    ///
    /// # Errors
    /// As `AttachmentManager::create_media`.
    pub async fn create_media(
        &self,
        initiator: ActorId,
        classification: Classification,
        media: Vec<ToolMedia>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        let context = &self.acting_for(initiator);
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

    /// The actor's own session as the actor reads it, for `initiator`, from its
    /// channel at `classification` (issue 163), if it may read it
    /// (`GetSession`, as `Views::session` asks, issue 304) - an agent by the
    /// group of the session's participants: the root of what its work
    /// navigates, asked in the context of each piece of work (issue 255).
    /// `None` while the channel is still being built or once it or its
    /// session is gone.
    ///
    /// # Errors
    /// As `Views::session`: the actor may not read it, or reading it failed.
    pub async fn session_view(
        &self,
        initiator: ActorId,
        classification: Classification,
    ) -> ViewResult<Option<Box<dyn SessionReader>>> {
        let context = &self.acting_for(initiator);
        let Some(session_loop) = self.channel_loop().and_then(|channel| channel.session_loop())
        else {
            return Ok(None);
        };
        let from =
            Place { channel: self.channel_id, classification, session: session_loop.privacy() };
        let views = session_loop.views(Viewer::from_place(context.clone(), from));
        let session = views.session(session_loop.id()).await?;
        Ok(Some(Box::new(session)))
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
    /// resolved as the actor's context for `initiator` - who caused them - names them (issue 165,
    /// `ChannelLoop::address_questions`); `None` while the channel is still
    /// being built or once it is gone, when there is nobody to ask.
    ///
    /// # Errors
    /// As `Questions::resolve`: an addressee the channel does not know.
    pub fn address_questions(
        &self,
        initiator: ActorId,
        questions: Questions,
    ) -> Option<Result<Questions, QuestionError>> {
        let context = &self.acting_for(initiator);
        Some(self.channel_loop()?.address_questions(questions, context))
    }

    /// How a call of `call`, made by the actor's agent for `initiator`, is admitted (issue 516):
    /// `Forbidden` unless the policy lets it be called (`CallTool`, the call's one question of
    /// permission, issue 304); else as the strongest rule of the system, the actor's session and
    /// its channel decides it now (`ApprovalRule::decide`, issue 13). Whether a person is asked
    /// beyond what a rule asks is its tool's, as the agent's owner configured it.
    pub async fn admit_tool_call(&self, initiator: ActorId, call: &ToolCall) -> CallPermit {
        let context = &self.acting_for(initiator);
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

    /// Whether what is classified `classification` may flow, in the actor's name, into what is
    /// classified `into` (`ConfidentialityPolicy::may_publish`, issue 516): a tool's result into
    /// the turn, the channel to a tool set's remote side, what an external agent brings into the
    /// channel, what a turn saw into the channel as it is now.
    pub fn may_publish(
        &self,
        initiator: ActorId,
        classification: &Classification,
        into: &Classification,
    ) -> bool {
        let context = &self.acting_for(initiator);
        self.confidentiality().may_publish(context, classification, into)
    }

    /// Whether what lies at `from` may be copied, in the actor's name, to `into`
    /// (`ConfidentialityQuestions::may_copy_information`, issue 516): what an agent may recall
    /// of what it keeps of its own, written elsewhere.
    pub fn may_copy_information(&self, initiator: ActorId, from: &Place, into: &Place) -> bool {
        let context = &self.acting_for(initiator);
        self.confidentiality().may_copy_information(context, from, into)
    }

    /// Whether `record` - what the actor's agent keeps of its own in its channel - may be
    /// reclassified, in the actor's name for `initiator`, as `classification`, within what is
    /// classified `within` (issues 133, 516): `Reclassify` of it, by the agent as its author, with
    /// the channel's attributes as it stands, then the flow (`require_publish`).
    pub async fn may_reclassify(
        &self,
        initiator: ActorId,
        record: OwnRecord,
        classification: &Classification,
        within: &Classification,
    ) -> bool {
        let context = &self.acting_for(initiator);
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

    /// The secrets that may be read in the actor's name, for `initiator` (issue 516), each read
    /// asked of the policy then (`ReadSecret`): what a component that talks to a remote system is
    /// handed for its credentials, shared, as what follows a remote task after a call reads them
    /// too.
    pub fn secrets(&self, initiator: ActorId) -> Arc<dyn Secrets> {
        let context = &self.acting_for(initiator);
        Arc::new(self.secret_manager().for_context(context.clone()))
    }

    /// `attachment`, one of the actor's channel, copied in the actor's name for a message it will
    /// say there (`AttachmentManager::copy`, issue 282).
    ///
    /// # Errors
    /// As `AttachmentManager::copy`.
    pub async fn copy_attachment(
        &self,
        initiator: ActorId,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        let context = &self.acting_for(initiator);
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

    /// The workspace of the actor's session and its terminal, for what `need` asks, as the actor
    /// may use it in its channel, for `initiator` (`WorkspaceController::terminal_in`, issue 344):
    /// asked by the workspace the session holds, of the session and the channel as they are now.
    ///
    /// # Errors
    /// `NotUsableInChannel` while the channel is still being built or once it or its session is
    /// gone, or no workspace is attached to the session; else what `terminal_in` reports.
    pub async fn workspace_terminal(
        &self,
        initiator: ActorId,
        need: Grant,
    ) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let context = &self.acting_for(initiator);
        let (session, workspace) = self.session_workspace()?;
        workspace.terminal_in(context, &session, &self.channel(), need).await
    }

    /// The workspace of the actor's session and its shell as the actor's channel sees it - the
    /// commands started in that channel alone -, for `initiator` (`WorkspaceController::shell_in`,
    /// issue 344).
    ///
    /// # Errors
    /// As [`Self::workspace_terminal`].
    pub async fn workspace_shell(
        &self,
        initiator: ActorId,
    ) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let context = &self.acting_for(initiator);
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
    use crate::testing::{actor_runtime_in, Script, ScriptedAgents, World, STAFF};
    use caelumora_model::channel::testing::bare_channel;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::user::{User, UserId, UserUpdateEvent};

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

    /// Decision 3 of issue 516: an agent's runtime acts with its owner's rights as they are at
    /// each call, not as they were when it was made - a role taken from her between two calls of
    /// one piece of work is gone for the second.
    #[tokio::test]
    async fn an_agent_s_runtime_acts_with_its_owner_s_roles_as_they_are_at_each_call() {
        let world = World::with(&[&ScriptedAgents(Script::Hangs)], |_| {}).await;
        let personality = world.personality("plain", None, &[]).await;
        let helper = world.agent("helper", world.alice, &personality).await;
        let session = world.shared_session(&[helper], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let runtime = actor_runtime_in(&main, ActorId::Agent(helper));
        let bob = ActorId::User(world.bob);

        let context = runtime.acting_for(bob);
        assert_eq!(
            (context.caller_id, context.principal_id, context.initiator_id),
            (ActorId::Agent(helper), ActorId::User(world.alice), bob),
            "as its owner's agent, for who asked"
        );
        assert_eq!(context.roles, [*STAFF], "with her roles");
        world
            .runtime
            .controller_manager()
            .user_controller(world.alice)
            .await
            .unwrap()
            .process_global_event(&GlobalEvent::updated::<User>(
                &world.alice,
                &UserUpdateEvent::RolesChanged(Vec::new()),
            ));
        assert!(runtime.acting_for(bob).roles.is_empty(), "not any more, at the next call");
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }
}
