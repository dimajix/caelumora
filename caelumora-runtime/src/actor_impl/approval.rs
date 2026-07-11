use crate::actor::{Actor, ActorSender, StandardActorSender};
use anyhow::{Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, Envelope, EventContext};
use caelumora_model::agent::ProcessingState;
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalResponse, ApprovalStatus, ApprovalStrategy};
use chrono::{DateTime, Duration, Utc};
use log::{info, warn};
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct PendingApproval {
    agent_id: ActorId,
    original_approval_id: ApprovalId,
    strategy: ApprovalStrategy,
    responses: HashMap<ActorId, ApprovalStatus>,
    timeout: DateTime<Utc>,
    _request: ApprovalRequest,
    session_id: caelumora_model::session::SessionId,
    channel_id: caelumora_model::channel::ChannelId,
}

#[derive(Debug)]
pub struct ApprovalActor {
    id: ActorId,
    user_count: usize,
    pending: Mutex<HashMap<ApprovalId, PendingApproval>>,
}

impl ApprovalActor {
    pub fn new(id: Uuid, user_count: usize) -> Self {
        Self {
            id: ActorId::Approval(id),
            user_count,
            pending: Mutex::new(HashMap::new()),
        }
    }

    async fn handle_envelope(&self, env: Envelope, output: &Sender<Envelope>) -> Result<()> {
        match env.event {
            ActorEvent::ToolExecutionRequested(req) => {
                self.handle_request(env.context, req, output).await
            }
            ActorEvent::ToolExecutionGranted(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Approved, output).await
            }
            ActorEvent::ToolExecutionDenied(res) => {
                self.handle_response(env.context.sender_id, res, ApprovalStatus::Denied, output).await
            }
            _ => Ok(()),
        }
    }

    async fn handle_request(&self, context: EventContext, request: ApprovalRequest, output: &Sender<Envelope>) -> Result<()> {
        let new_id = ApprovalId(Uuid::new_v4());
        let timeout = Utc::now() + Duration::minutes(10); // Default timeout 10 mins

        let mut pending_req = request.clone();
        pending_req.id = new_id;

        let pending = PendingApproval {
            agent_id: context.sender_id,
            original_approval_id: request.id,
            strategy: ApprovalStrategy::Any, // TODO: Get strategy from somewhere?
            responses: HashMap::new(),
            timeout,
            _request: pending_req.clone(),
            session_id: request.session_id,
            channel_id: request.channel_id,
        };

        self.pending.lock().await.insert(new_id, pending);

        let new_event = ActorEvent::ToolExecutionRequested(pending_req);
        let new_context = context.next(self.id, "Approval".to_string());
        let envelope = Envelope::new(caelumora_model::security::SecurityContext::default(), new_context, new_event);

        output.send(envelope).await.context("Failed to send approval request")?;

        Ok(())
    }

    async fn handle_response(&self, user_id: ActorId, response: ApprovalResponse, status: ApprovalStatus, output: &Sender<Envelope>) -> Result<()> {
        let mut pending = self.pending.lock().await;
        let mut to_remove = None;

        if let Some(p) = pending.get_mut(&response.id) {
            p.responses.insert(user_id, status);
            
            if let Some(decision) = self.check_strategy(p) {
                to_remove = Some((response.id, decision));
            }
        }

        if let Some((id, decision)) = to_remove {
            let p = pending.remove(&id).unwrap();
            self.send_decision(p, decision, output).await?;
        }

        Ok(())
    }

    fn check_strategy(&self, p: &PendingApproval) -> Option<ApprovalStatus> {
        let approved_count = p.responses.values().filter(|&&s| s == ApprovalStatus::Approved).count();
        let denied_count = p.responses.values().filter(|&&s| s == ApprovalStatus::Denied).count();
        let total_responses = p.responses.len();

        match p.strategy {
            ApprovalStrategy::Any => {
                if approved_count > 0 {
                    Some(ApprovalStatus::Approved)
                } else if total_responses >= self.user_count && denied_count == self.user_count {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::Majority => {
                if approved_count > self.user_count / 2 {
                    Some(ApprovalStatus::Approved)
                } else if denied_count >= (self.user_count + 1) / 2 {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::All => {
                if approved_count == self.user_count {
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
                } else if approved_count == self.user_count {
                    Some(ApprovalStatus::Approved)
                } else {
                    None
                }
            }
        }
    }

    async fn send_decision(&self, p: PendingApproval, status: ApprovalStatus, output: &Sender<Envelope>) -> Result<()> {
        let response = ApprovalResponse {
            id: p.original_approval_id,
            agent_id: p.agent_id.as_agent_id().unwrap(),
        };

        let event = match status {
            ApprovalStatus::Approved => ActorEvent::ToolExecutionGranted(response),
            ApprovalStatus::Denied => ActorEvent::ToolExecutionDenied(response),
            ApprovalStatus::Unknown => return Ok(()),
        };

        let context = EventContext::new(p.session_id, p.channel_id, self.id, "Approval".to_string());
        let envelope = Envelope::new(caelumora_model::security::SecurityContext::default(), context, event);

        output.send(envelope).await.context("Failed to send decision")?;
        Ok(())
    }

    async fn handle_timeouts(&self, output: &Sender<Envelope>) -> Result<()> {
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
                agent_id: p.agent_id.as_agent_id().unwrap(),
            };
            let event = ActorEvent::ToolExecutionRequestTimedOut(response);
            let context = EventContext::new(p.session_id, p.channel_id, self.id, "Approval".to_string());
            let envelope = Envelope::new(caelumora_model::security::SecurityContext::default(), context, event);
            output.send(envelope).await.context("Failed to send timeout")?;
        }

        Ok(())
    }
}

#[async_trait]
impl Actor for ApprovalActor {
    fn id(&self) -> ActorId {
        self.id
    }

    fn kind(&self) -> String {
        "approval".to_string()
    }

    fn name(&self) -> String {
        "Approval".to_string()
    }

    fn state(&self) -> ProcessingState {
        ProcessingState::Idle
    }

    fn new_sender(&self, tx: Sender<Envelope>) -> Box<dyn ActorSender> {
        Box::new(StandardActorSender {
            tx,
            filter: Box::new(|_| true),
        })
    }

    async fn run(&self, mut rx: Receiver<Envelope>, tx: Sender<Envelope>) -> Result<()> {
        info!("ApprovalActor {:?} started", self.id);
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
                            if let Err(e) = self.handle_envelope(env, &tx).await {
                                warn!("ApprovalActor error handling envelope: {:?}", e);
                            }
                        }
                        None => break,
                    }
                }
                _ = timeout_fut => {
                    if let Err(e) = self.handle_timeouts(&tx).await {
                        warn!("ApprovalActor error handling timeouts: {:?}", e);
                    }
                }
            }
        }
        info!("ApprovalActor {:?} finished", self.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::channel::ChannelId;
    use caelumora_model::session::SessionId;
    use caelumora_model::tool::ToolCall;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_approval_actor_any_strategy() {
        let actor_id = Uuid::new_v4();
        let actor = ApprovalActor::new(actor_id, 2);
        let (tx, rx) = mpsc::channel(100);
        let (output_tx, mut output_rx) = mpsc::channel(100);

        let actor_arc = Arc::new(actor);
        let actor_clone = actor_arc.clone();
        tokio::spawn(async move {
            let a: &dyn Actor = actor_clone.as_ref();
            a.run(rx, output_tx).await.unwrap();
        });

        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let channel_id = ChannelId::new();
        let approval_id = ApprovalId::new();

        let request = ApprovalRequest {
            id: approval_id,
            session_id,
            channel_id,
            agent_id,
            tool_call: ToolCall {
                id: Some("call_1".to_string()),
                function_name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            },
            description: "test".to_string(),
        };

        let context = EventContext::new(session_id, channel_id, ActorId::Agent(agent_id), "Agent".to_string());
        let envelope = Envelope::new(caelumora_model::security::SecurityContext::default(), context, ActorEvent::ToolExecutionRequested(request));

        tx.send(envelope).await.unwrap();

        // Should receive forward request
        let forward_env = output_rx.recv().await.unwrap();
        let new_approval_id = if let ActorEvent::ToolExecutionRequested(req) = forward_env.event {
            req.id
        } else {
            panic!("Expected ToolExecutionRequested");
        };

        // Send approval from user 1
        let user1_id = ActorId::User(caelumora_model::user::UserId(Uuid::new_v4()));
        let response = ApprovalResponse {
            id: new_approval_id,
            agent_id,
        };
        let context1 = EventContext::new(session_id, channel_id, user1_id, "User 1".to_string());
        let env1 = Envelope::new(caelumora_model::security::SecurityContext::default(), context1, ActorEvent::ToolExecutionGranted(response));
        tx.send(env1).await.unwrap();

        // Decision reached (Any strategy)
        let decision_env: Envelope = output_rx.recv().await.unwrap();
        if let ActorEvent::ToolExecutionGranted(res) = decision_env.event {
            assert_eq!(res.id, approval_id);
            assert_eq!(res.agent_id, agent_id);
        } else {
            panic!("Expected ToolExecutionGranted");
        }
    }
}
