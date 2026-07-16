use crate::actor::Actor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::agent::AgentId;
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApprovalStatus, ApprovalStrategy, Approver};
use caelumora_model::channel::{ChannelController, Participant};
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::SecurityContext;
use chrono::{DateTime, Duration, Utc};
use log::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct PendingApproval {
    agent_id: AgentId,
    original_approval_id: ApprovalId,
    strategy: ApprovalStrategy,
    responses: HashMap<ActorId, ApprovalStatus>,
    timeout: DateTime<Utc>,
    _request: ApprovalRequest,
    security_context: SecurityContext,
    event_context: EventContext,
}

#[derive(Debug)]
pub struct ApproverActor {
    id: ActorId,
    approver: Approver,
    actor_runtime: ActorRuntime,
    channel: Arc<ChannelController>,
    pending: Mutex<HashMap<ApprovalId, PendingApproval>>,
}

impl ApproverActor {
    pub fn new(channel: Arc<ChannelController>, approver: Approver, actor_runtime: ActorRuntime) -> Self {
        Self {
            id: ActorId::Approver(approver.id.0),
            approver,
            actor_runtime,
            channel,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn get_user_count(&self) -> usize {
        let channel = self.channel.load();
        channel.participants.iter().filter(|p| matches!(p, Participant::User(_))).count()
    }

    async fn handle_global_event(&self, event:&GlobalEvent, output: &dyn EventSender) {
        match event {
            GlobalEvent::AgentChannelJoined(_, cid, _) | GlobalEvent::AgentChannelLeft(_, cid, _) |
            GlobalEvent::UserChannelJoined(_, cid, _) | GlobalEvent::UserChannelLeft(_, cid, _) if *cid == self.channel.id() => {
                if let Err(e) = self.recheck_all_pending(output).await {
                    warn!("ApproverActor error rechecking approvals: {:?}", e);
                }
            }
            _ => {}
        }
    }

    async fn handle_envelope(&self, env: Envelope, output: &dyn EventSender) -> Result<()> {
        match env.event {
            ActorEvent::ApprovalRequested(req) => {
                self.handle_request(req, env.context, env.security, output).await
            }
            ActorEvent::ApprovalGranted(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Approved, output).await
            }
            ActorEvent::ApprovalDenied(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Denied, output).await
            }
            _ => Ok(()),
        }
    }

    async fn handle_request(&self, mut request: ApprovalRequest, context:EventContext, security:SecurityContext, output: &dyn EventSender) -> Result<()> {
        let new_id = ApprovalId(Uuid::new_v4());
        let timeout = Utc::now() + Duration::minutes(10); // Default timeout 10 mins

        let original_approval_id = request.id;
        let agent_id = request.agent_id;
        request.id = new_id;

        let pending = PendingApproval {
            agent_id,
            original_approval_id,
            strategy: self.approver.strategy,
            responses: HashMap::new(),
            timeout,
            _request: request.clone(),
            security_context: security.clone(),
            event_context: context.clone(),
        };

        self.pending.lock().await.insert(new_id, pending);

        let new_event = ActorEvent::ApprovalRequested(request);
        let new_context = context.next(self.id, "Approver".to_string());
        let new_security = security.next(self.id);
        let envelope = Envelope::new(new_security, new_context, new_event);

        output.send(envelope).await.context("Failed to send approval request")?;

        Ok(())
    }

    async fn handle_response(&self, user_id: ActorId, response: ApprovalResponse, status: ApprovalStatus, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let mut to_remove = None;

        if let Some(p) = pending.get_mut(&response.id) {
            p.responses.insert(user_id, status);
            
            let user_count = self.get_user_count();
            if let Some(decision) = self.check_strategy(p, user_count) {
                to_remove = Some((response.id, decision));
            }
        }

        if let Some((id, decision)) = to_remove {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, decision, output).await?;
            }
        }

        Ok(())
    }

    async fn recheck_all_pending(&self, output: &dyn EventSender) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let mut to_remove = Vec::new();

        {
            let user_count = self.get_user_count();

            for (id, p) in pending.iter() {
                if let Some(decision) = self.check_strategy(p, user_count) {
                    to_remove.push((*id, decision));
                }
            }
        }

        for (id, decision) in to_remove {
            if let Some(p) = pending.remove(&id) {
                self.send_decision(p, decision, output).await?;
            }
        }

        Ok(())
    }

    fn check_strategy(&self, p: &PendingApproval, user_count: usize) -> Option<ApprovalStatus> {
        let approved_count = p.responses.values().filter(|&&s| s == ApprovalStatus::Approved).count();
        let denied_count = p.responses.values().filter(|&&s| s == ApprovalStatus::Denied).count();
        let total_responses = p.responses.len();

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
            id: p.original_approval_id,
            agent_id: p.agent_id,
        };

        let event = match status {
            ApprovalStatus::Approved => ActorEvent::ApprovalGranted(response),
            ApprovalStatus::Denied => ActorEvent::ApprovalDenied(response),
            ApprovalStatus::Timeout => ActorEvent::ApprovalTimedOut(response),
            ApprovalStatus::Pending => return Ok(()),
        };

        let context = p.event_context.next(self.id, "Approver".to_string());
        let envelope = Envelope::new(p.security_context.next(self.id), context, event);

        output.send(envelope).await.context("Failed to send decision")?;
        Ok(())
    }

    async fn handle_timeouts(&self, output: &dyn EventSender) -> Result<()> {
        let now = Utc::now();
        let mut timed_out = Vec::new();

        {
            let mut pending = self.pending.lock().await;
            pending.retain(|_id, p| {
                if p.timeout <= now {
                    timed_out.push(p.clone());
                    false
                } else {
                    true
                }
            });
        }

        for p in timed_out {
            let response = ApprovalResponse {
                id: p.original_approval_id,
                agent_id: p.agent_id,
            };
            let event = ActorEvent::ApprovalTimedOut(response);
            let context = p.event_context.next(self.id, "Approver".to_string());
            let envelope = Envelope::new(p.security_context.next(self.id), context, event);
            output.send(envelope).await.context("Failed to send timeout")?;
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
        "Approver".to_string()
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
            )
        }
    }

    async fn run(&self, mut rx: Box<dyn EventReceiver>, tx: Box<dyn EventSender>) -> Result<()> {
        info!("ApproverActor {:?} started", self.id);
        let mut global_rx = self.actor_runtime.event_bus().subscribe_channel(self.channel.id());

        loop {
            let next_timeout = {
                let pending = self.pending.lock().await;
                pending.values().map(|p| p.timeout).min()
            };

            let timeout_fut = async {
                if let Some(t) = next_timeout {
                    let now = Utc::now();
                    if t > now {
                        tokio::time::sleep((t - now).to_std().unwrap()).await;
                    }
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                maybe_env = rx.recv() => {
                    match maybe_env {
                        Some(env) => {
                            if let Err(e) = self.handle_envelope(env, &*tx).await {
                                warn!("ApproverActor error handling envelope: {:?}", e);
                            }
                        }
                        None => break,
                    }
                }
                maybe_global = global_rx.recv() => {
                    if let Some(event) = maybe_global {
                        self.channel.process_global_event(&event).await;
                        self.handle_global_event(&event, &*tx).await;
                    }
                }
                _ = timeout_fut => {
                    if let Err(e) = self.handle_timeouts(&*tx).await {
                        warn!("ApproverActor error handling timeouts: {:?}", e);
                    }
                }
            }
        }
        info!("ApproverActor {:?} finished", self.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeManager;
    use crate::transport_impl::{InMemoryEventReceiver, InMemoryEventSender};
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::{Approver, ApproverId};
    use caelumora_model::channel::{Channel, ChannelId, Participant};
    use caelumora_model::session::SessionId;
    use caelumora_model::store::EntityStore;
    use caelumora_model::tool::ToolCall;
    use caelumora_model::user::{User, UserId};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_approval_actor_any_strategy() {
        let store = Arc::new(caelumora_model::store_impl::memory::MemoryStore::new());
        let runtime_manager = Arc::new(RuntimeManager::new(store.clone(), store.clone()));

        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        let approval_id = ApprovalId::new();

        let user1_id = UserId(Uuid::new_v4());
        let user1 = User {
            id: user1_id,
            username: "user1".to_string(),
            email: "user1@example.com".to_string(),
            roles: vec![],
            security: caelumora_model::security::SecurityContext::default(),
        };

        let approver = Approver {
            id: ApproverId(Uuid::new_v4()),
            strategy: ApprovalStrategy::Any,
        };

        let channel = Channel {
            id: channel_id,
            session_id,
            name: "test".to_string(),
            moderator: "broadcast".to_string(),
            participants: vec![
                Participant::User(user1),
                Participant::Approver(approver.clone()),
            ],
            security_attributes: caelumora_model::security::SecurityAttributes::default(),
        };

        let channel_controller = Arc::new(ChannelController::new(channel, runtime_manager.event_bus().clone(), Some(store.clone())));
        let actor_runtime = runtime_manager.new_actor_runtime();
        let actor = ApproverActor::new(channel_controller, approver, actor_runtime);
        let (tx, rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let actor_arc = Arc::new(actor);
        let actor_clone = actor_arc.clone();
        tokio::spawn(async move {
            let a: &dyn Actor = actor_clone.as_ref();
            let receiver = Box::new(InMemoryEventReceiver::new(rx));
            let sender = Box::new(InMemoryEventSender::new(output_tx));
            a.run(receiver, sender).await.unwrap();
        });

        let request = ApprovalRequest {
            id: approval_id,
            session_id,
            channel_id,
            agent_id,
            tool_call: ToolCall {
                id: Some("call_1".to_string()),
                function_name: "test_tool".to_string(),
                arguments: json!("{}".to_string()),
            },
            description: "test".to_string(),
        };

        let context = EventContext::new(session_id, channel_id, ActorId::Agent(agent_id), "Agent".to_string());
        let envelope = Envelope::new(caelumora_model::security::SecurityContext::default(), context, ActorEvent::ApprovalRequested(request));

        tx.send(envelope).await.unwrap();

        // Should receive forward request
        let forward_env = output_rx.recv().await.unwrap();
        let new_approval_id = if let ActorEvent::ApprovalRequested(req) = forward_env.event {
            req.id
        } else {
            panic!("Expected ToolExecutionRequested");
        };

        // Send approval from user 1
        let user1_actor_id = ActorId::User(user1_id);
        let response = ApprovalResponse {
            id: new_approval_id,
            agent_id,
        };
        let context1 = EventContext::new(session_id, channel_id, user1_actor_id, "User 1".to_string());
        let env1 = Envelope::new(caelumora_model::security::SecurityContext::default(), context1, ActorEvent::ApprovalGranted(response));
        tx.send(env1).await.unwrap();

        // Decision reached (Any strategy)
        let decision_env: Envelope = output_rx.recv().await.unwrap();
        if let ActorEvent::ApprovalGranted(res) = decision_env.event {
            assert_eq!(res.id, approval_id);
            assert_eq!(res.agent_id, agent_id);
        } else {
            panic!("Expected ToolExecutionGranted, got {:?}", decision_env.event);
        }
    }

    #[tokio::test]
    async fn test_approval_actor_dynamic_membership() {
        let store = Arc::new(caelumora_model::store_impl::memory::MemoryStore::new());
        let runtime_manager = Arc::new(RuntimeManager::new(store.clone(), store.clone()));

        let session_id = SessionId::new();
        let channel_id = ChannelId::new();

        let approver = Approver {
            id: ApproverId(Uuid::new_v4()),
            strategy: ApprovalStrategy::All,
        };

        let channel = Channel {
            id: channel_id,
            session_id,
            name: "test".to_string(),
            moderator: "broadcast".to_string(),
            participants: vec![
                Participant::Approver(approver.clone()),
            ],
            security_attributes: caelumora_model::security::SecurityAttributes::default(),
        };

        let channel_controller = Arc::new(ChannelController::new(channel, runtime_manager.event_bus().clone(), Some(store.clone())));
        let actor_runtime = runtime_manager.new_actor_runtime();
        let actor = ApproverActor::new(channel_controller, approver, actor_runtime);
        let (tx, rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let actor_arc = Arc::new(actor);
        let actor_clone = actor_arc.clone();
        tokio::spawn(async move {
            let a: &dyn Actor = actor_clone.as_ref();
            let receiver = Box::new(InMemoryEventReceiver::new(rx));
            let sender = Box::new(InMemoryEventSender::new(output_tx));
            a.run(receiver, sender).await.unwrap();
        });

        // Give some time for the actor to start and subscribe
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 1. Join user1
        let user1_id = UserId(Uuid::new_v4());
        let user1 = User {
            id: user1_id,
            username: "user1".to_string(),
            email: "user1@example.com".to_string(),
            roles: vec![],
            security: caelumora_model::security::SecurityContext::default(),
        };
        store.create_user(&caelumora_model::security::SecurityContext::default(), &user1).await.unwrap();

        let join_event = GlobalEvent::UserChannelJoined(session_id, channel_id, user1_id);
        runtime_manager.event_bus().publish(join_event);

        // Give some time for the event to be processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 2. Request approval
        let agent_id = AgentId::new();
        let approval_id = ApprovalId::new();
        let request = ApprovalRequest {
            id: approval_id,
            session_id,
            channel_id,
            agent_id,
            tool_call: ToolCall {
                id: Some("call_1".to_string()),
                function_name: "test_tool".to_string(),
                arguments: json!("{}".to_string()),
            },
            description: "test".to_string(),
        };
        let req_env = Envelope::new(
            caelumora_model::security::SecurityContext::default(),
            EventContext::new(session_id, channel_id, ActorId::Agent(agent_id), "Agent".to_string()),
            ActorEvent::ApprovalRequested(request)
        );
        tx.send(req_env).await.unwrap();

        let forward_env = output_rx.recv().await.unwrap();
        let new_approval_id = if let ActorEvent::ApprovalRequested(req) = forward_env.event {
            req.id
        } else {
            panic!("Expected ToolExecutionRequested");
        };

        // 3. User1 approves
        let response = ApprovalResponse { id: new_approval_id, agent_id };
        let context1 = EventContext::new(session_id, channel_id, ActorId::User(user1_id), "User 1".to_string());
        let env1 = Envelope::new(caelumora_model::security::SecurityContext::default(), context1, ActorEvent::ApprovalGranted(response));
        tx.send(env1).await.unwrap();

        // 4. Decision reached (All strategy, user count is 1)
        let decision_env = output_rx.recv().await.unwrap();
        if let ActorEvent::ApprovalGranted(res) = decision_env.event {
            assert_eq!(res.id, approval_id);
        } else {
            panic!("Expected ToolExecutionGranted");
        }
    }

    #[tokio::test]
    async fn test_approval_actor_leave_decision() {
        let store = Arc::new(caelumora_model::store_impl::memory::MemoryStore::new());
        let runtime_manager = Arc::new(RuntimeManager::new(store.clone(), store.clone()));

        let session_id = SessionId::new();
        let channel_id = ChannelId::new();

        let approver = Approver {
            id: ApproverId(Uuid::new_v4()),
            strategy: ApprovalStrategy::All,
        };

        let user1_id = UserId(Uuid::new_v4());
        let user2_id = UserId(Uuid::new_v4());

        let user1 = User { id: user1_id, username: "u1".to_string(), email: "".to_string(), roles: vec![], security: Default::default() };
        let user2 = User { id: user2_id, username: "u2".to_string(), email: "".to_string(), roles: vec![], security: Default::default() };

        let channel = Channel {
            id: channel_id,
            session_id,
            name: "test".to_string(),
            moderator: "broadcast".to_string(),
            participants: vec![
                Participant::Approver(approver.clone()),
                Participant::User(user1.clone()),
                Participant::User(user2.clone()),
            ],
            security_attributes: caelumora_model::security::SecurityAttributes::default(),
        };

        let channel_controller = Arc::new(ChannelController::new(channel, runtime_manager.event_bus().clone(), Some(store.clone())));
        let actor_runtime = runtime_manager.new_actor_runtime();
        let actor = ApproverActor::new(channel_controller, approver, actor_runtime);
        let (tx, rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let actor_arc = Arc::new(actor);
        let actor_clone = actor_arc.clone();
        tokio::spawn(async move {
            let a: &dyn Actor = actor_clone.as_ref();
            let receiver = Box::new(InMemoryEventReceiver::new(rx));
            let sender = Box::new(InMemoryEventSender::new(output_tx));
            a.run(receiver, sender).await.unwrap();
        });

        // 1. Request approval
        let agent_id = AgentId::new();
        let approval_id = ApprovalId::new();
        let request = ApprovalRequest {
            id: approval_id,
            session_id,
            channel_id,
            agent_id,
            tool_call: ToolCall { id: None, function_name: "f".to_string(), arguments: json!("".to_string()) },
            description: "test".to_string(),
        };
        tx.send(Envelope::new(Default::default(), EventContext::new(session_id, channel_id, ActorId::Agent(agent_id), "Agent".to_string()), ActorEvent::ApprovalRequested(request))).await.unwrap();

        let forward_env = output_rx.recv().await.unwrap();
        let new_approval_id = if let ActorEvent::ApprovalRequested(req) = forward_env.event { req.id } else { panic!() };

        // 2. User 1 approves
        tx.send(Envelope::new(Default::default(), EventContext::new(session_id, channel_id, ActorId::User(user1_id), "u1".to_string()), ActorEvent::ApprovalGranted(ApprovalResponse { id: new_approval_id, agent_id }))).await.unwrap();

        // 3. User 2 leaves
        let leave_event = GlobalEvent::UserChannelLeft(session_id, channel_id, user2_id);
        runtime_manager.event_bus().publish(leave_event);

        // Give some time for the event to be processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 4. Decision should be reached now
        let decision_env = output_rx.recv().await.unwrap();
        if let ActorEvent::ApprovalGranted(res) = decision_env.event {
            assert_eq!(res.id, approval_id);
        } else {
            panic!("Expected ToolExecutionGranted, got {:?}", decision_env.event);
        }
    }
}
