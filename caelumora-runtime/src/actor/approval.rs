use crate::actor::{Actor, Answering};
use crate::controller::ChannelController;
use crate::runtime::ActorRuntime;
use crate::transport::{is_closed, EventReceiver, EventSender};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{
    ApprovalDecision, ApprovalId, ApprovalResponse, ApprovalStatus, ApprovalStrategy, Approver,
};
use caelumora_model::channel::{self, Channel, Participant};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{
    AccessPolicy, AccessQuestions, SecurityAttributes, SecurityContext,
};
use caelumora_model::value::name::Name;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
    /// Each user's answer, with their part in it as their actor said it
    /// (issue 419).
    responses: HashMap<ActorId, (ApprovalStatus, ApprovalDecision)>,
    timeout: DateTime<Utc>,
    /// The agent's request, whose contexts every answer to it continues
    /// (`ApproverActor::answering`).
    request: Envelope,
}

impl PendingApproval {
    /// The decision its strategy reaches from the answers of `approvers` -
    /// those who may approve -, with whose answers made it (issue 419):
    /// every counted answer that agrees with it, in the order they were
    /// given. `None` while it waits for more answers.
    fn decision(&self, approvers: &[ActorId]) -> Option<(ApprovalStatus, Vec<ApprovalDecision>)> {
        let statuses: HashMap<ActorId, ApprovalStatus> =
            self.responses.iter().map(|(by, (status, _))| (*by, *status)).collect();
        let status = self.strategy.decide(approvers, &statuses)?;
        let mut decided_by: Vec<ApprovalDecision> = self
            .responses
            .iter()
            .filter(|(by, (answered, _))| approvers.contains(by) && *answered == status)
            .map(|(_, (_, decision))| decision.clone())
            .collect();
        decided_by.sort_by_key(|decision| decision.at);
        Some((status, decided_by))
    }
}

#[derive(Debug)]
pub struct ApproverActor {
    id: ActorId,
    approver: Approver,
    actor_runtime: ActorRuntime,
    channel: Arc<ChannelController>,
    /// Who may approve (issue 14).
    access_policy: Arc<dyn AccessPolicy>,
    pending: Mutex<HashMap<ApprovalId, PendingApproval>>,
    /// Cancelled by `stop`: the channel is deleted, and the run ends.
    ended: CancellationToken,
}

impl ApproverActor {
    /// The actor of `approver` in the channel of `actor_runtime`.
    pub fn new(approver: Approver, actor_runtime: ActorRuntime) -> Self {
        Self {
            id: ActorId::Approver(approver.id),
            approver,
            access_policy: Arc::clone(actor_runtime.access_policy()),
            channel: Arc::clone(actor_runtime.channel_controller()),
            actor_runtime,
            pending: Mutex::new(HashMap::new()),
            ended: CancellationToken::new(),
        }
    }

    /// The contexts it answers `request`, an agent's request for approval,
    /// in (`Answering::to`): in its own name, about the call of whoever asked
    /// (issue 255), answered for by the owner of its channel as it is when
    /// it answers. What it sends passes `Answering::send` unchanged: an
    /// approval carries no message, and the approvers see the arguments of
    /// the call they decide.
    fn answering(&self, request: &Envelope) -> Answering {
        Answering::to(request, self.id, self.name(), &self.security_context())
    }

    /// The attributes of `agent`, as stored (`agent_attributes`). `None` if
    /// they cannot be read.
    async fn attributes_of(&self, agent: AgentId) -> Option<SecurityAttributes> {
        match self.actor_runtime.agent_attributes(agent).await {
            Ok(attributes) => Some(attributes),
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
        let (Some(running), Some(attributes)) =
            (self.actor_runtime.channel_loop(), self.attributes_of(agent).await)
        else {
            return Vec::new();
        };
        let channel = self.channel.load();
        let mut approvers = Vec::new();
        for participant in &channel.participants {
            let Participant::User(user) = participant else { continue };
            let Some(actor) = running.user_actor(*user) else { continue };
            if self.access_policy.may_approve(&actor.security_context(), &attributes).await {
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
    async fn handle_global_event(&self, event: &GlobalEvent, output: &dyn EventSender) {
        let Some(((_, channel_id), change)) = event.about::<Channel>() else { return };
        if channel_id != self.channel.id() {
            return;
        }
        let left = change.data::<ActorId>(channel::LEFT);
        if let Some(ActorId::Agent(agent)) = left {
            if let Err(e) = self.discard_pending_of(agent, output).await {
                warn!("ApproverActor error discarding the approvals of agent {}: {:?}", agent, e);
            }
        }
        if left.is_some() || change.data::<ActorId>(channel::JOINED).is_some() {
            if let Err(e) = self.recheck_all_pending(output).await {
                warn!("ApproverActor error rechecking approvals: {:?}", e);
            }
        }
    }

    /// Answers the pending approvals of `agent` as denied and forgets them.
    async fn discard_pending_of(&self, agent: AgentId, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let ids: Vec<ApprovalId> =
            pending.iter().filter(|(_, p)| p.agent_id == agent).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, ApprovalStatus::Denied, Vec::new(), output).await?;
            }
        }
        Ok(())
    }

    async fn handle_envelope(&self, env: Envelope, output: &dyn EventSender) -> Result<()> {
        match env.event {
            ActorEvent::ApprovalRequested(_) => self.handle_request(env, output).await,
            ActorEvent::ApprovalGranted(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Approved, output)
                    .await
            }
            ActorEvent::ApprovalDenied(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Denied, output)
                    .await
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
    async fn handle_request(&self, envelope: Envelope, output: &dyn EventSender) -> Result<()> {
        let ActorEvent::ApprovalRequested(request) = &envelope.event else { return Ok(()) };
        {
            let mut pending = self.pending.lock().await;
            let approval = pending.entry(request.id).or_insert_with(|| PendingApproval {
                id: request.id,
                agent_id: request.agent_id,
                strategy: self.approver.strategy,
                responses: HashMap::new(),
                timeout: Utc::now() + Duration::minutes(10), // Default timeout 10 mins
                request: envelope.clone(),
            });
            let approvers = self.approvers_of(approval.agent_id).await;
            if approvers.is_empty() {
                info!(
                    "ApproverActor: nobody in channel {} may approve the calls of agent {}; denied",
                    self.channel.id(),
                    approval.agent_id
                );
                if let Some(p) = pending.remove(&request.id) {
                    self.send_decision(p, ApprovalStatus::Denied, Vec::new(), output).await?;
                }
                return Ok(());
            }
        }

        self.answering(&envelope).send(ActorEvent::ApprovalRequested(request.clone()), output).await
    }

    /// Counts the answer of `user_id` to the request it names, with their
    /// part in it (issue 419). Whether they may approve the calls of the
    /// agent it names was asked as the channel admitted it
    /// (`ApproveToolCall`, issue 312), and a refusal told them
    /// (`MessageRejected`); so was that it holds their part alone. An answer
    /// naming another agent than the one that asked is left out.
    async fn handle_response(
        &self,
        user_id: ActorId,
        response: ApprovalResponse,
        status: ApprovalStatus,
        output: &dyn EventSender,
    ) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let Some(p) = pending.get_mut(&response.id) else { return Ok(()) };
        if response.agent_id != p.agent_id {
            warn!("ApproverActor: {} answered approval {} for agent {}, not for agent {} that asked; its answer is left out", user_id, response.id, response.agent_id, p.agent_id);
            return Ok(());
        }
        let Some(decision) = response.decisions.into_iter().find(|decision| decision.by == user_id)
        else {
            warn!("ApproverActor: the answer of {} to approval {} does not say it is theirs; it is left out", user_id, response.id);
            return Ok(());
        };
        p.responses.insert(user_id, (status, decision));

        let approvers = self.approvers_of(p.agent_id).await;
        if let Some((status, decided_by)) = p.decision(&approvers) {
            if let Some(p) = pending.remove(&response.id) {
                self.send_decision(p, status, decided_by, output).await?;
            }
        }
        Ok(())
    }

    /// Forgets the request `id` that `sender` withdrew (issue 148) - only the
    /// agent that asked may - and tells the users, so that nobody is asked
    /// any longer. Nothing is decided: the agent no longer waits.
    async fn handle_withdrawal(
        &self,
        sender: ActorId,
        id: ApprovalId,
        output: &dyn EventSender,
    ) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let Some(p) = pending.get(&id) else { return Ok(()) };
        if sender != ActorId::Agent(p.agent_id) {
            warn!(
                "ApproverActor: {} tried to withdraw the approval request {} of agent {}",
                sender, id, p.agent_id
            );
            return Ok(());
        }
        let Some(p) = pending.remove(&id) else { return Ok(()) };
        self.answering(&p.request).send(ActorEvent::ApprovalRequestWithdrawn { id }, output).await
    }

    async fn recheck_all_pending(&self, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let mut to_remove = Vec::new();

        for (id, p) in pending.iter() {
            let approvers = self.approvers_of(p.agent_id).await;
            if let Some(decision) = p.decision(&approvers) {
                to_remove.push((*id, decision));
            }
        }

        for (id, (status, decided_by)) in to_remove {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, status, decided_by, output).await?;
            }
        }

        Ok(())
    }

    /// Tells the agent and the users the decision `status` on `p`, with
    /// `decided_by`, whose answers made it (issue 419).
    async fn send_decision(
        &self,
        p: PendingApproval,
        status: ApprovalStatus,
        decided_by: Vec<ApprovalDecision>,
        output: &dyn EventSender,
    ) -> Result<()> {
        let response =
            ApprovalResponse { decisions: decided_by, ..ApprovalResponse::new(p.id, p.agent_id) };

        let event = match status {
            ApprovalStatus::Approved => ActorEvent::ApprovalGranted(response),
            ApprovalStatus::Denied => ActorEvent::ApprovalDenied(response),
            ApprovalStatus::Timeout => ActorEvent::ApprovalTimedOut(response),
        };

        self.answering(&p.request).send(event, output).await
    }

    async fn handle_timeouts(&self, output: &dyn EventSender) -> Result<()> {
        let now = Utc::now();
        let timed_out: Vec<PendingApproval> = {
            let mut pending = self.pending.lock().await;
            let ids: Vec<ApprovalId> =
                pending.iter().filter(|(_, p)| p.timeout <= now).map(|(id, _)| *id).collect();
            ids.iter().filter_map(|id| pending.remove(id)).collect()
        };
        for p in timed_out {
            self.send_decision(p, ApprovalStatus::Timeout, Vec::new(), output).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Actor for ApproverActor {
    /// The context it acts in, here: its own, answered for by the owner of
    /// its channel (issue 341).
    fn security_context(&self) -> SecurityContext {
        let channel = self.channel.load();
        SecurityContext {
            owner_id: channel.security_attributes.owner_id,
            ..SecurityContext::new(self.id, channel.session_id, channel.id)
        }
    }

    fn id(&self) -> ActorId {
        self.id
    }

    fn name(&self) -> Name {
        ActorId::approver_name()
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
                        time::sleep(wait).await;
                    }
                } else {
                    future::pending::<()>().await;
                }
            };

            tokio::select! {
                _ = self.ended.cancelled() => break,
                maybe_env = rx.recv() => {
                    match maybe_env {
                        Some(env) => {
                            if let Err(e) = self.handle_envelope(env, &*tx).await {
                                // Its channel is gone: nothing left to decide for (issue 79).
                                if is_closed(&e) {
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
                        if is_closed(&e) {
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

    /// Ends the run: its channel stops (issue 377). What is pending is
    /// left undecided; the agents that wait for it ask again as their
    /// channel runs again.
    fn stop(&self) {
        self.ended.cancel();
    }

    /// Ends the run: the channel is deleted - an approver never leaves one
    /// otherwise. What is pending is left undecided; nobody is left to ask.
    fn leave(&self) {
        self.ended.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelLoop;
    use crate::runtime::{ChannelRuntime, RuntimeManager};
    use crate::session::testing::bare_session;
    use crate::testing::TestRuntime;
    use crate::transport::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_model::actor::EventContext;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::approval::ApprovalRequest;
    use caelumora_model::channel::{
        moderation_components, Channel, ChannelId, InternalActor, Participant,
    };
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::security::{Grant, OwnershipPolicy, Subject};
    use caelumora_model::session::SessionId;
    use caelumora_model::store::Persistence;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::{User, UserId};
    use caelumora_model::value::name::{Name, ScopedName};
    use serde_json::json;
    use std::any::Any;
    use std::time::Duration;
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
    async fn approving(
        strategy: ApprovalStrategy,
        users: usize,
        agent: impl FnOnce(&[UserId]) -> SecurityAttributes,
    ) -> Approving {
        let store = Persistence::in_memory();
        let system = SecurityContext::system();
        let mut ids = Vec::new();
        for n in 0..users {
            let user = User {
                id: UserId::new(),
                username: Name::new(format!("user{n}")).unwrap(),
                email: String::new(),
                roles: vec![],
                deleted: None,
            };
            store.entity_stores().unwrap().users.create(&system, &user).await.unwrap();
            ids.push(user.id);
        }
        let personality = Personality {
            description: None,
            introduction: None,
            id: PersonalityId::new(),
            name: ScopedName::new("plain", Default::default()).unwrap(),
            kind: Default::default(),
            roles: None,
            components: Default::default(),
            security_attributes: SecurityAttributes::systems(),
            reasoning_visibility: Default::default(),
            hears_during_turn: Default::default(),
            deleted: None,
        };
        store.entity_stores().unwrap().personalities.create(&system, &personality).await.unwrap();
        let attributes = agent(&ids);
        let mut stored = Agent::new(
            ScopedName::new("helper", Default::default()).unwrap(),
            personality.id,
            attributes.owner_id,
        );
        stored.security_attributes = attributes;
        store.entity_stores().unwrap().agents.create(&system, &stored).await.unwrap();

        let runtime = TestRuntime {
            store: store.clone(),
            access_policy: Arc::new(OwnershipPolicy::new()),
            ..Default::default()
        }
        .build();
        let approver = Approver::new(strategy);
        let participants: Vec<Participant> = ids.iter().map(|id| Participant::User(*id)).collect();
        let channel = Channel {
            summary: None,
            briefing: None,
            approval_rules: Vec::new(),
            uses_workspace: true,
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("test").unwrap(),
            components: moderation_components(None, Some("broadcast_policy")),
            classification: Classification::CHANNEL_DEFAULT,
            language: None,
            participants,
            internal_actors: vec![InternalActor::Approver(approver.clone())],
            security_attributes: SecurityAttributes::owned_by(ActorId::User(ids[0])),
            deleted: None,
            former: Vec::new(),
        };
        store.entity_stores().unwrap().channels.create(&system, &channel).await.unwrap();
        let session_id = channel.session_id;
        let channel = ChannelLoop::new(ChannelRuntime::outside_a_session(
            Arc::clone(&runtime),
            bare_session(session_id),
            channel,
        ))
        .await
        .unwrap();
        let actor: Arc<dyn Any + Send + Sync> =
            channel.actor(ActorId::Approver(approver.id)).unwrap();
        let approver = Arc::downcast::<ApproverActor>(actor).unwrap();

        let (input, rx) = mpsc::channel(100);
        let (tx, output) = mpsc::channel(100);
        let running = Arc::clone(&approver);
        tokio::spawn(async move {
            running
                .run(
                    Box::new(InMemoryEventReceiver::new(rx)),
                    Box::new(InMemoryEventSender::new(tx)),
                )
                .await
                .unwrap();
        });
        Approving { channel, approver, runtime, agent: stored.id, users: ids, input, output }
    }

    impl Approving {
        fn request(&self) -> ApprovalRequest {
            ApprovalRequest {
                suggested: None,
                explicit: false,
                id: ApprovalId::new(),
                session_id: self.channel.session_id(),
                channel_id: self.channel.id(),
                agent_id: self.agent,
                tool_call: ToolCall {
                    id: Some("call_1".to_string()),
                    function_name: "test_tool".to_string(),
                    arguments: json!({}),
                    visibility: Default::default(),
                    presentation: Default::default(),
                },
                description: "test".to_string(),
            }
        }

        /// `event` from `actor`, in its own context, as its actor sends it.
        fn from(&self, actor: ActorId, event: ActorEvent) -> Envelope {
            let (session, channel) = (self.channel.session_id(), self.channel.id());
            Envelope::new(
                SecurityContext::new(actor, session, channel),
                EventContext::new(session, channel, actor, actor.to_string()),
                event,
            )
        }

        /// The agent asks for `request`, and it is put to the users.
        async fn ask(&mut self, request: &ApprovalRequest) {
            self.input
                .send(self.from(
                    ActorId::Agent(self.agent),
                    ActorEvent::ApprovalRequested(request.clone()),
                ))
                .await
                .unwrap();
            let asked = self.next().await;
            assert!(
                matches!(asked.event, ActorEvent::ApprovalRequested(ref r) if r.id == request.id),
                "put to the users: {:?}",
                asked.event
            );
            assert_eq!(asked.context.sender_id, ActorId::Approver(self.approver.approver.id));
            assert_eq!(
                self.speaker(&asked),
                (asked.context.sender_id, ActorId::User(self.users[0])),
                "in its own name, answered for by whose channel it is"
            );
        }

        /// Who `said` it, and who answers for them: the approver, the
        /// owner of its channel (issue 341).
        fn speaker(&self, said: &Envelope) -> (ActorId, ActorId) {
            assert_eq!(
                said.security.principal_id, said.security.caller_id,
                "the approver speaks in its own name"
            );
            (said.security.caller_id, said.security.owner_id)
        }

        /// The `n`th user answers `request`, saying it is theirs (issue 419).
        async fn answer(&self, n: usize, request: &ApprovalRequest, approved: bool) {
            let user = ActorId::User(self.users[n]);
            let part = ApprovalDecision {
                by: user,
                name: format!("user {n}"),
                by_rule: None,
                at: Utc::now(),
            };
            let response = ApprovalResponse {
                decisions: vec![part],
                ..ApprovalResponse::new(request.id, self.agent)
            };
            let event = if approved {
                ActorEvent::ApprovalGranted(response)
            } else {
                ActorEvent::ApprovalDenied(response)
            };
            self.input.send(self.from(ActorId::User(self.users[n]), event)).await.unwrap();
        }

        async fn next(&mut self) -> Envelope {
            time::timeout(Duration::from_secs(2), self.output.recv())
                .await
                .expect("sent in time")
                .expect("still running")
        }

        async fn nothing(&mut self) {
            assert!(
                time::timeout(Duration::from_millis(100), self.output.recv()).await.is_err(),
                "nothing is decided"
            );
        }
    }

    /// Whose answers made the decision `decided` tells (issue 419).
    fn decided_by(decided: &Envelope) -> Vec<ActorId> {
        match &decided.event {
            ActorEvent::ApprovalGranted(r)
            | ActorEvent::ApprovalDenied(r)
            | ActorEvent::ApprovalTimedOut(r) => {
                r.decisions.iter().map(|decision| decision.by).collect()
            }
            other => panic!("no decision: {other:?}"),
        }
    }

    /// The agent's attributes: owned by the first user, read by the others.
    fn shared(users: &[UserId]) -> SecurityAttributes {
        users[1..].iter().fold(SecurityAttributes::owned_by(ActorId::User(users[0])), |a, u| {
            a.granting(Subject::Actor(ActorId::User(*u)), Grant::Read)
        })
    }

    #[tokio::test]
    async fn one_approval_of_who_may_decides_any() {
        let mut approving = approving(ApprovalStrategy::Any, 1, shared).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(0, &request, true).await;
        let decided = approving.next().await;
        assert!(
            matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id && r.agent_id == approving.agent),
            "one id from the agent's request to the decision: {:?}",
            decided.event
        );
        assert_eq!(
            approving.speaker(&decided).1,
            ActorId::User(approving.users[0]),
            "decided, answered for by whose channel it is"
        );
        assert_eq!(
            decided_by(&decided),
            [ActorId::User(approving.users[0])],
            "naming who approved (issue 419)"
        );

        // A request withdrawn (issue 148) - only by the agent that asked -
        // is forgotten, and the users are told.
        let withdrawn = approving.request();
        approving.ask(&withdrawn).await;
        let user = ActorId::User(approving.users[0]);
        approving
            .input
            .send(approving.from(user, ActorEvent::ApprovalRequestWithdrawn { id: withdrawn.id }))
            .await
            .unwrap();
        approving
            .input
            .send(approving.from(
                ActorId::Agent(approving.agent),
                ActorEvent::ApprovalRequestWithdrawn { id: withdrawn.id },
            ))
            .await
            .unwrap();
        let told = approving.next().await;
        assert!(
            matches!(told.event, ActorEvent::ApprovalRequestWithdrawn { id } if id == withdrawn.id),
            "not the user's withdrawal: {:?}",
            told.event
        );
        assert_eq!(
            approving.speaker(&told).1,
            ActorId::User(approving.users[0]),
            "told, answered for by whose channel it is"
        );
        approving.answer(0, &withdrawn, true).await;
        approving.nothing().await;
    }

    #[tokio::test]
    async fn the_answer_of_who_may_not_approve_is_left_out_and_not_waited_for() {
        // Two users, only the first - the agent's owner - may approve.
        let mut approving = approving(ApprovalStrategy::All, 2, |users| {
            SecurityAttributes::owned_by(ActorId::User(users[0]))
        })
        .await;
        let request = approving.request();
        approving.ask(&request).await;

        approving.answer(1, &request, false).await;
        approving.nothing().await;
        // An answer that does not say it is theirs is left out (issue 419).
        let unsaid =
            ActorEvent::ApprovalGranted(ApprovalResponse::new(request.id, approving.agent));
        approving
            .input
            .send(approving.from(ActorId::User(approving.users[0]), unsaid))
            .await
            .unwrap();
        approving.nothing().await;
        approving.answer(0, &request, true).await;
        let decided = approving.next().await;
        assert!(
            matches!(decided.event, ActorEvent::ApprovalGranted(_)),
            "all who may approve did: {:?}",
            decided.event
        );
        assert_eq!(
            decided_by(&decided),
            [ActorId::User(approving.users[0])],
            "naming who approved, not who may not and denied"
        );
    }

    #[tokio::test]
    async fn a_request_nobody_here_may_approve_is_denied_at_once() {
        // The agent belongs to someone not taking part.
        let mut approving = approving(ApprovalStrategy::All, 1, |_| {
            SecurityAttributes::owned_by(ActorId::User(UserId::new()))
        })
        .await;
        let request = approving.request();
        approving
            .input
            .send(approving.from(
                ActorId::Agent(approving.agent),
                ActorEvent::ApprovalRequested(request.clone()),
            ))
            .await
            .unwrap();
        let decided = approving.next().await;
        assert!(
            matches!(decided.event, ActorEvent::ApprovalDenied(ref r) if r.id == request.id),
            "denied, not put to anyone: {:?}",
            decided.event
        );
        assert!(decided_by(&decided).is_empty(), "by nobody's answer");
        assert!(approving.approver.pending.lock().await.is_empty(), "nothing pending");
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
        assert!(
            matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id),
            "both approved: {:?}",
            decided.event
        );
    }

    #[tokio::test]
    async fn a_user_who_leaves_is_no_longer_waited_for() {
        let mut approving = approving(ApprovalStrategy::All, 2, shared).await;
        let request = approving.request();
        approving.ask(&request).await;
        approving.answer(0, &request, true).await;
        approving.nothing().await;

        approving
            .channel
            .remove_participant(&SecurityContext::system(), ActorId::User(approving.users[1]))
            .await
            .unwrap();
        let decided = approving.next().await;
        assert!(
            matches!(decided.event, ActorEvent::ApprovalGranted(ref r) if r.id == request.id),
            "all who are left approved: {:?}",
            decided.event
        );
    }

    #[tokio::test]
    async fn an_agent_that_leaves_takes_its_pending_approvals_with_it() {
        let mut approving = approving(ApprovalStrategy::All, 1, shared).await;
        let request = approving.request();
        approving.ask(&request).await;

        // The agent leaves before anyone decided (issue 46).
        let (session, channel) = (approving.channel.session_id(), approving.channel.id());
        approving.runtime.event_bus().publish(
            GlobalEvent::custom::<Channel>(
                &(session, channel),
                channel::LEFT,
                &ActorId::Agent(approving.agent),
            ),
            SecurityAttributes::systems(),
        );

        // Nobody keeps waiting: the request is answered as denied.
        let decided = approving.next().await;
        assert!(
            matches!(decided.event, ActorEvent::ApprovalDenied(ref r) if r.id == request.id),
            "denied: {:?}",
            decided.event
        );
        assert!(approving.approver.pending.lock().await.is_empty(), "nothing pending any more");
    }
}
