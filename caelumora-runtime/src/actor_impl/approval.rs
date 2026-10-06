use crate::actor::Actor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApprovalStatus, ApprovalStrategy, Approver};
use caelumora_model::channel::Participant;
use crate::controller::{ChannelController, ControllerManager};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{AccessQuestions, ActionType, Enforcement, SecurityAttributes, SecurityContext, SecurityManager};
use chrono::{DateTime, Duration, Utc};
use tracing::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// An approval waiting for its decision, kept under the id the agent gave
/// it: the same id from the agent's request through the users' answers to
/// the decision (issue 81). Users' answers reach only the approver - the
/// moderator routes them so - which is what keeps them from deciding for
/// themselves, not a second id.
#[derive(Debug)]
struct PendingApproval {
    id: ApprovalId,
    agent_id: AgentId,
    strategy: ApprovalStrategy,
    responses: HashMap<ActorId, ApprovalStatus>,
    timeout: DateTime<Utc>,
    security_context: SecurityContext,
    event_context: EventContext,
}

#[derive(Debug)]
pub struct ApproverActor {
    id: ActorId,
    approver: Approver,
    actor_runtime: ActorRuntime,
    channel: Arc<ChannelController>,
    /// Who may approve (issue 14).
    security: Arc<dyn SecurityManager>,
    /// Where the attributes of the agents that ask are read.
    controllers: Arc<ControllerManager>,
    pending: Mutex<HashMap<ApprovalId, PendingApproval>>,
    /// Cancelled by `stop`: the channel is deleted, and the run ends.
    ended: CancellationToken,
}

impl ApproverActor {
    pub fn new(channel: Arc<ChannelController>, approver: Approver, actor_runtime: ActorRuntime) -> Self {
        Self {
            id: ActorId::Approver(approver.id),
            approver,
            security: Arc::clone(actor_runtime.security_manager()),
            controllers: Arc::clone(actor_runtime.runtime_manager().controller_manager()),
            actor_runtime,
            channel,
            pending: Mutex::new(HashMap::new()),
            ended: CancellationToken::new(),
        }
    }

    /// The attributes of `agent`, as stored - never as its request says,
    /// since the agent makes that itself. `None` if it cannot be read.
    async fn attributes_of(&self, agent: AgentId) -> Option<SecurityAttributes> {
        match self.controllers.agent_controller(agent).await {
            Ok(controller) => Some(controller.load().security_attributes.clone()),
            Err(e) => {
                warn!("ApproverActor cannot read agent {}: {}", agent, e);
                None
            }
        }
    }

    /// The users taking part who may approve the calls of `agent`
    /// (`ApproveToolCall`, issue 14), each asked in the context of their own
    /// actor - the one their answers come with. Nobody while the channel
    /// does not run or the agent cannot be read.
    async fn approvers_of(&self, agent: AgentId) -> Vec<ActorId> {
        let (Some(running), Some(attributes)) = (self.actor_runtime.channel_loop(), self.attributes_of(agent).await) else {
            return Vec::new();
        };
        let channel = self.channel.load();
        let mut approvers = Vec::new();
        for participant in &channel.participants {
            let Participant::User(user) = participant else { continue };
            let Some(actor) = running.user_actor(*user) else { continue };
            if self.security.may_approve(actor.security_context(), &attributes, &channel.security_attributes).await {
                approvers.push(ActorId::User(*user));
            }
        }
        approvers
    }

    /// Follows the channel's participants (issue 46), which its
    /// `ChannelController` has already changed when the event arrives: an
    /// agent that left takes its pending approvals with it - they are
    /// answered as denied, so that nobody keeps waiting for them - and every
    /// join or leave may decide a pending approval, since who may approve
    /// changed.
    async fn handle_global_event(&self, event:&GlobalEvent, output: &dyn EventSender) {
        match event {
            GlobalEvent::AgentChannelLeft(_, cid, agent) if *cid == self.channel.id() => {
                if let Err(e) = self.discard_pending_of(*agent, output).await {
                    warn!("ApproverActor error discarding the approvals of agent {}: {:?}", agent, e);
                }
                if let Err(e) = self.recheck_all_pending(output).await {
                    warn!("ApproverActor error rechecking approvals: {:?}", e);
                }
            }
            GlobalEvent::AgentChannelJoined(_, cid, _) |
            GlobalEvent::UserChannelJoined(_, cid, _) | GlobalEvent::UserChannelLeft(_, cid, _) if *cid == self.channel.id() => {
                if let Err(e) = self.recheck_all_pending(output).await {
                    warn!("ApproverActor error rechecking approvals: {:?}", e);
                }
            }
            _ => {}
        }
    }

    /// Answers the pending approvals of `agent` as denied and forgets them.
    async fn discard_pending_of(&self, agent: AgentId, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let ids: Vec<ApprovalId> = pending.iter().filter(|(_, p)| p.agent_id == agent).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, ApprovalStatus::Denied, output).await?;
            }
        }
        Ok(())
    }

    async fn handle_envelope(&self, env: Envelope, output: &dyn EventSender) -> Result<()> {
        match env.event {
            ActorEvent::ApprovalRequested(req) => {
                self.handle_request(req, env.context, env.security, output).await
            }
            ActorEvent::ApprovalGranted(res) => {
                self.handle_response(env.context.sender_id, &env.security, res, ApprovalStatus::Approved, output).await
            }
            ActorEvent::ApprovalDenied(res) => {
                self.handle_response(env.context.sender_id, &env.security, res, ApprovalStatus::Denied, output).await
            }
            ActorEvent::ApprovalRequestWithdrawn { id } => {
                self.handle_withdrawal(env.context.sender_id, id, output).await
            }
            _ => Ok(()),
        }
    }

    /// Takes `request` up and asks the users. A request it already holds -
    /// asked again by an agent that resumed its turn (issue 172) - is only
    /// announced again: the answers given so far and the timeout stay. One
    /// that nobody taking part may approve is denied at once, not put to
    /// anyone (issue 14).
    async fn handle_request(&self, request: ApprovalRequest, context:EventContext, security:SecurityContext, output: &dyn EventSender) -> Result<()> {
        {
            let mut pending = self.pending.lock().await;
            let approval = pending.entry(request.id).or_insert_with(|| PendingApproval {
                id: request.id,
                agent_id: request.agent_id,
                strategy: self.approver.strategy,
                responses: HashMap::new(),
                timeout: Utc::now() + Duration::minutes(10), // Default timeout 10 mins
                security_context: security.clone(),
                event_context: context.clone(),
            });
            let approvers = self.approvers_of(approval.agent_id).await;
            if approvers.is_empty() {
                info!("ApproverActor: nobody in channel {} may approve the calls of agent {}; denied", self.channel.id(), approval.agent_id);
                if let Some(p) = pending.remove(&request.id) {
                    self.send_decision(p, ApprovalStatus::Denied, output).await?;
                }
                return Ok(());
            }
        }

        let new_event = ActorEvent::ApprovalRequested(request);
        let new_context = context.next(self.id, self.name());
        let new_security = security.next(self.id);
        let envelope = Envelope::new(new_security, new_context, new_event);

        output.send(envelope).await.context("Failed to send approval request")?;

        Ok(())
    }

    /// Counts the answer of `user_id`, given in `security`, if they may
    /// approve the calls of the agent that asked (`ApproveToolCall`, issue
    /// 14); an answer of anyone else is left out. Only who may approve is
    /// asked, so such an answer comes from a client going round what it was
    /// shown, and is not told.
    async fn handle_response(&self, user_id: ActorId, security: &SecurityContext, response: ApprovalResponse, status: ApprovalStatus, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let Some(p) = pending.get_mut(&response.id) else { return Ok(()) };
        let Some(attributes) = self.attributes_of(p.agent_id).await else { return Ok(()) };
        let channel = self.channel.load();
        let action = ActionType::ApproveToolCall { agent: &attributes, channel: &channel.security_attributes };
        if self.security.require(security, action, || ()).await.is_err() {
            warn!("ApproverActor: {} may not approve the calls of agent {}; its answer is left out", user_id, p.agent_id);
            return Ok(());
        }
        p.responses.insert(user_id, status);

        let approvers = self.approvers_of(p.agent_id).await;
        if let Some(decision) = Self::check_strategy(p, &approvers) {
            if let Some(p) = pending.remove(&response.id) {
                self.send_decision(p, decision, output).await?;
            }
        }
        Ok(())
    }

    /// Forgets the request `id` that `sender` withdrew (issue 148) - only the
    /// agent that asked may - and tells the users, so that nobody is asked
    /// any longer. Nothing is decided: the agent no longer waits.
    async fn handle_withdrawal(&self, sender: ActorId, id: ApprovalId, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let Some(p) = pending.get(&id) else { return Ok(()) };
        if sender != ActorId::Agent(p.agent_id) {
            warn!("ApproverActor: {} tried to withdraw the approval request {} of agent {}", sender, id, p.agent_id);
            return Ok(());
        }
        let Some(p) = pending.remove(&id) else { return Ok(()) };
        let context = p.event_context.next(self.id, self.name());
        let envelope = Envelope::new(p.security_context.next(self.id), context, ActorEvent::ApprovalRequestWithdrawn { id });
        output.send(envelope).await.context("Failed to announce a withdrawn approval request")?;
        Ok(())
    }

    async fn recheck_all_pending(&self, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let mut to_remove = Vec::new();

        for (id, p) in pending.iter() {
            let approvers = self.approvers_of(p.agent_id).await;
            if let Some(decision) = Self::check_strategy(p, &approvers) {
                to_remove.push((*id, decision));
            }
        }

        for (id, decision) in to_remove {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, decision, output).await?;
            }
        }

        Ok(())
    }

    /// The decision on `p` its strategy reaches, counting the answers of
    /// `approvers` - those taking part who may approve, now - and only
    /// theirs: who left or may no longer approve does not count, either
    /// way. Without anyone who may approve, it is denied.
    fn check_strategy(p: &PendingApproval, approvers: &[ActorId]) -> Option<ApprovalStatus> {
        let user_count = approvers.len();
        if user_count == 0 {
            return Some(ApprovalStatus::Denied);
        }
        let counted = || p.responses.iter().filter(|(id, _)| approvers.contains(id)).map(|(_, status)| *status);
        let approved_count = counted().filter(|&s| s == ApprovalStatus::Approved).count();
        let denied_count = counted().filter(|&s| s == ApprovalStatus::Denied).count();
        let total_responses = counted().count();

        match p.strategy {
            ApprovalStrategy::Any => {
                if approved_count > 0 {
                    Some(ApprovalStatus::Approved)
                } else if total_responses >= user_count && denied_count == user_count {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::Majority => {
                if approved_count > user_count / 2 {
                    Some(ApprovalStatus::Approved)
                } else if denied_count >= (user_count + 1) / 2 {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::All => {
                if approved_count >= user_count && user_count > 0 {
                    Some(ApprovalStatus::Approved)
                } else if denied_count > 0 {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::DenyOnFirst => {
                if denied_count > 0 {
                    Some(ApprovalStatus::Denied)
                } else if approved_count >= user_count && user_count > 0 {
                    Some(ApprovalStatus::Approved)
                } else {
                    None
                }
            }
        }
    }

    async fn send_decision(&self, p: PendingApproval, status: ApprovalStatus, output: &dyn EventSender) -> Result<()> {
        let response = ApprovalResponse {
            id: p.id,
            agent_id: p.agent_id,
        };

        let event = match status {
            ApprovalStatus::Approved => ActorEvent::ApprovalGranted(response),
            ApprovalStatus::Denied => ActorEvent::ApprovalDenied(response),
            ApprovalStatus::Timeout => ActorEvent::ApprovalTimedOut(response),
            ApprovalStatus::Pending => return Ok(()),
        };

        let context = p.event_context.next(self.id, self.name());
        let envelope = Envelope::new(p.security_context.next(self.id), context, event);

        output.send(envelope).await.context("Failed to send decision")?;
        Ok(())
    }

    async fn handle_timeouts(&self, output: &dyn EventSender) -> Result<()> {
        let now = Utc::now();
        let timed_out: Vec<PendingApproval> = {
            let mut pending = self.pending.lock().await;
            let ids: Vec<ApprovalId> = pending.iter().filter(|(_, p)| p.timeout <= now).map(|(id, _)| *id).collect();
            ids.iter().filter_map(|id| pending.remove(id)).collect()
        };
        for p in timed_out {
            self.send_decision(p, ApprovalStatus::Timeout, output).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Actor for ApproverActor {
    fn id(&self) -> ActorId {
        self.id
    }

    fn kind(&self) -> String {
        "approver".to_string()
    }

    fn name(&self) -> String {
        ActorId::APPROVER_NAME.to_string()
    }

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |event| {
            matches!(
                event.event,
                ActorEvent::ApprovalGranted(_)
                    | ActorEvent::ApprovalDenied(_)
                    | ActorEvent::ApprovalRequested(_)
                    | ActorEvent::ApprovalRequestWithdrawn { .. }
            )
        }
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        info!("ApproverActor {:?} started", self.id);
        let mut global_rx = self.actor_runtime.subscribe_channel();

        loop {
            let next_timeout = {
                let pending = self.pending.lock().await;
                pending.values().map(|p| p.timeout).min()
            };

            let timeout_fut = async {
                if let Some(t) = next_timeout {
                    // A timeout already past does not convert: nothing to wait for.
                    if let Ok(wait) = (t - Utc::now()).to_std() {
                        tokio::time::sleep(wait).await;
                    }
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                _ = self.ended.cancelled() => break,
                maybe_env = rx.recv() => {
                    match maybe_env {
                        Some(env) => {
                            if let Err(e) = self.handle_envelope(env, &*tx).await {
                                // Its channel is gone: nothing left to decide for (issue 79).
                                if crate::transport::is_closed(&e) {
                                    info!("ApproverActor {:?} ends: its channel is gone", self.id);
                                    break;
                                }
                                warn!("ApproverActor error handling envelope: {:?}", e);
                            }
                        }
                        None => break,
                    }
                }
                maybe_global = global_rx.recv() => {
                    if let Some(event) = maybe_global {
                        self.handle_global_event(&event, &*tx).await;
                    }
                }
                _ = timeout_fut => {
                    if let Err(e) = self.handle_timeouts(&*tx).await {
                        if crate::transport::is_closed(&e) {
                            info!("ApproverActor {:?} ends: its channel is gone", self.id);
                            break;
                        }
                        warn!("ApproverActor error handling timeouts: {:?}", e);
                    }
                }
            }
        }
        info!("ApproverActor {:?} finished", self.id);
        Ok(())
    }

    /// Ends the run: the channel is deleted - an approver never leaves one
    /// otherwise. What is pending is left undecided; nobody is left to ask.
    fn stop(&self) {
        self.ended.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelLoop;
    use crate::command::CommandExecutor;
    use crate::runtime::{ChannelRuntime, RuntimeManager};
    use crate::transport_impl::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::approval::ApproverId;
    use caelumora_model::channel::{Channel, ChannelId, Participant};
    use caelumora_model::command::default_commands;
    use caelumora_model::name::{Name, ScopedName};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
    use caelumora_model::session::SessionId;
    use caelumora_model::store::EntityStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::{User, UserId};
    use serde_json::json;
    use std::any::Any;
    use tokio::sync::mpsc;

    /// The approver of a channel of users, under the ownership policy, run
    /// on its own: the channel is built, so that its users have their
    /// actors, but not run. The agent whose calls are approved is stored,
    /// not taking part - the approver only reads its attributes.
    struct Approving {
        channel: Arc<ChannelLoop>,
        approver: Arc<ApproverActor>,
        runtime: Arc<RuntimeManager>,
        agent: AgentId,
        users: Vec<UserId>,
        input: mpsc::Sender<Envelope>,
        output: mpsc::Receiver<Envelope>,
    }

    /// `Approving` with `strategy`, `users` users - the first owning the
    /// channel - and an agent of `agent`, given the users.
    async fn approving(strategy: ApprovalStrategy, users: usize, agent: impl FnOnce(&[UserId]) -> SecurityAttributes) -> Approving {
        let store = Arc::new(MemoryStore::new());
        let system = SecurityContext::system();
        let mut ids = Vec::new();
        for n in 0..users {
            let user = User { id: UserId::new(), username: format!("user{n}"), email: String::new(), roles: vec![], deleted: None };
            store.create_user(&system, &user).await.unwrap();
            ids.push(user.id);
        }
        let personality = Personality {
            description: String::new(),
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components: Default::default(),
            security_attributes: Default::default(),
            reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let mut stored = Agent::new(ScopedName::new("helper", Default::default()).unwrap(), &personality, ActorId::System);
        stored.security_attributes = agent(&ids);
        store.create_agent(&system, &stored).await.unwrap();

        let security: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&security)));
        let registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, security, default_commands(), Default::default(), Vec::new(), registry).unwrap());
        let approver = Approver { id: ApproverId::new(), strategy };
        let mut participants: Vec<Participant> = ids.iter().map(|id| Participant::User(*id)).collect();
        participants.push(Participant::Approver(approver.clone()));
        let channel = Channel {
            approval_rules: Vec::new(),
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("test").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            participants,
            security_attributes: SecurityAttributes::owned_by(ActorId::User(ids[0])),
            deleted: None,
            former: Vec::new(),
        };
        store.create_channel(&system, &channel).await.unwrap();
        let executor = CommandExecutor::empty(Arc::clone(runtime.security_manager()));
        let session_id = channel.session_id;
        let channel = ChannelLoop::new(channel, ChannelRuntime::outside_a_session(Arc::clone(&runtime), Arc::new(executor), session_id)).await.unwrap();
        let actor: Arc<dyn Any + Send + Sync> = channel.actor_by_id(ActorId::Approver(approver.id)).unwrap();
        let approver = Arc::downcast::<ApproverActor>(actor).unwrap();

        let (input, rx) = mpsc::channel(100);
        let (tx, output) = mpsc::channel(100);
        let running = Arc::clone(&approver);
        tokio::spawn(async move {
            running.run(Box::new(InMemoryEventReceiver::new(rx)), Box::new(InMemoryEventSender::new(tx))).await.unwrap();
        });
        Approving { channel, approver, runtime, agent: stored.id, users: ids, input, output }
    }

    impl Approving {
        fn request(&self) -> ApprovalRequest {
            ApprovalRequest {
                explicit: false,
                id: ApprovalId::new(),
                session_id: self.channel.session_id(),
                channel_id: self.channel.id(),
                agent_id: self.agent,
                tool_call: ToolCall { id: Some("call_1".to_string()), function_name: "test_tool".to_string(), arguments: json!({}), visibility: Default::default() },
                description: "test".to_string(),
            }
        }

        /// `event` from `actor`, in its own context, as its actor sends it.
        fn from(&self, actor: ActorId, event: ActorEvent) -> Envelope {
            let (session, channel) = (self.channel.session_id(), self.channel.id());
            Envelope::new(SecurityContext::new(actor, session, channel), EventContext::new(session, channel, actor, actor.to_string()), event)
        }

        /// The agent asks for `request`, and it is put to the users.
        async fn ask(&mut self, request: &ApprovalRequest) {
            self.input.send(self.from(ActorId::Agent(self.agent), ActorEvent::ApprovalRequested(request.clone()))).await.unwrap();
            let asked = self.next().await;
            assert!(matches!(asked.event, ActorEvent::ApprovalRequested(ref r) if r.id == request.id), "put to the users: {:?}", asked.event);
            assert_eq!(asked.context.sender_id, ActorId::Approver(self.approver.approver.id));
        }

        /// The `n`th user answers `request`.
        async fn answer(&self, n: usize, request: &ApprovalRequest, approved: bool) {
            let response = ApprovalResponse { id: request.id, agent_id: self.agent };
            let event = if approved { ActorEvent::ApprovalGranted(response) } else { ActorEvent::ApprovalDenied(response) };
            self.input.send(self.from(ActorId::User(self.users[n]), event)).await.unwrap();
        }

        async fn next(&mut self) -> Envelope {
            tokio::time::timeout(std::time::Duration::from_secs(2), self.output.recv()).await.expect("sent in time").expect("still running")
        }

        async fn nothing(&mut self) {
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100), self.output.recv()).await.is_err(), "nothing is decided");
        }
    }

    /// The agent's attributes: owned by the first user, the others contributors.
    fn shared(users: &[UserId]) -> SecurityAttributes {
        SecurityAttributes { owner_id: ActorId::User(users[0]), contributors: users[1..].iter().map(|u| ActorId::User(*u)).collect() }
    }

    #[tokio::test]
    async fn one_approval_of_who_may_decides_any() {
        let mut approving = approving(ApprovalStrategy::Any, 1, shared).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(0, &request, true).await;
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id && r.agent_id == approving.agent), "one id from the agent's request to the decision: {:?}", decided.event);

        // A request withdrawn (issue 148) - only by the agent that asked -
        // is forgotten, and the users are told.
        let withdrawn = approving.request();
        approving.ask(&withdrawn).await;
        let user = ActorId::User(approving.users[0]);
        approving.input.send(approving.from(user, ActorEvent::ApprovalRequestWithdrawn { id: withdrawn.id })).await.unwrap();
        approving.input.send(approving.from(ActorId::Agent(approving.agent), ActorEvent::ApprovalRequestWithdrawn { id: withdrawn.id })).await.unwrap();
        let told = approving.next().await;
        assert!(matches!(told.event, ActorEvent::ApprovalRequestWithdrawn { id } if id == withdrawn.id), "not the user's withdrawal: {:?}", told.event);
        approving.answer(0, &withdrawn, true).await;
        approving.nothing().await;
    }

    #[tokio::test]
    async fn the_answer_of_who_may_not_approve_is_left_out_and_not_waited_for() {
        // Two users, only the first - the agent's owner - may approve.
        let mut approving = approving(ApprovalStrategy::All, 2, |users| SecurityAttributes::owned_by(ActorId::User(users[0]))).await;
        let request = approving.request();
        approving.ask(&request).await;

        approving.answer(1, &request, false).await;
        approving.nothing().await;
        approving.answer(0, &request, true).await;
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalGranted(_)), "all who may approve did: {:?}", decided.event);
    }

    #[tokio::test]
    async fn a_request_nobody_here_may_approve_is_denied_at_once() {
        // The agent belongs to someone not taking part.
        let mut approving = approving(ApprovalStrategy::All, 1, |_| SecurityAttributes::owned_by(ActorId::User(UserId::new()))).await;
        let request = approving.request();
        approving.input.send(approving.from(ActorId::Agent(approving.agent), ActorEvent::ApprovalRequested(request.clone()))).await.unwrap();
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalDenied(ref r) if r.id == request.id), "denied, not put to anyone: {:?}", decided.event);
        assert!(approving.approver.pending.lock().await.is_empty(), "nothing pending");
    }

    #[tokio::test]
    async fn the_calls_of_an_agent_of_the_system_are_approved_by_whose_channel_it_is() {
        let mut approving = approving(ApprovalStrategy::Any, 2, |_| SecurityAttributes::owned_by(ActorId::System)).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(1, &request, true).await;
        approving.nothing().await;
        approving.answer(0, &request, true).await;
        assert!(matches!(approving.next().await.event, ActorEvent::ApprovalGranted(_)), "the channel's owner approved");
    }

    #[tokio::test]
    async fn a_request_asked_again_keeps_the_answers_given_so_far() {
        // Both have to approve.
        let mut approving = approving(ApprovalStrategy::All, 2, shared).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(0, &request, true).await;

        // The agent resumed its turn and asks again (issue 172): the users
        // are asked again, the first answer stays.
        approving.ask(&request).await;
        approving.answer(1, &request, true).await;
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id), "both approved: {:?}", decided.event);
    }

    #[tokio::test]
    async fn a_user_who_leaves_is_no_longer_waited_for() {
        let mut approving = approving(ApprovalStrategy::All, 2, shared).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(0, &request, true).await;
        approving.nothing().await;

        approving.channel.remove_participant(&SecurityContext::system(), ActorId::User(approving.users[1])).await.unwrap();
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id), "all who are left approved: {:?}", decided.event);
    }

    #[tokio::test]
    async fn an_agent_that_leaves_takes_its_pending_approvals_with_it() {
        let mut approving = approving(ApprovalStrategy::All, 1, shared).await;
        let request = approving.request();
        approving.ask(&request).await;

        // The agent leaves before anyone decided (issue 46).
        let (session, channel) = (approving.channel.session_id(), approving.channel.id());
        approving.runtime.event_bus().publish(GlobalEvent::AgentChannelLeft(session, channel, approving.agent), Default::default());

        // Nobody keeps waiting: the request is answered as denied.
        let decided = approving.next().await;
        assert!(matches!(decided.event, ActorEvent::ApprovalDenied(ref r) if r.id == request.id), "denied: {:?}", decided.event);
        assert!(approving.approver.pending.lock().await.is_empty(), "nothing pending any more");
    }
}
