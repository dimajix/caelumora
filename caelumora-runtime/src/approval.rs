//! Approval management for Caelumora.

use async_trait::async_trait;
use caelumora_model::approval::{ApprovalId, ApprovalRequest, ApprovalResponse};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Trait for managing tool execution approvals.
#[async_trait]
pub trait ApprovalManager: Send + Sync + std::fmt::Debug {
    /// Request approval for a tool execution.
    fn request_approval(&self, request: ApprovalRequest) -> oneshot::Receiver<ApprovalResponse>;

    /// Respond to a pending approval request.
    fn respond(&self, id: ApprovalId, response: ApprovalResponse) -> anyhow::Result<()>;
}

/// Default implementation of the `ApprovalManager`.
#[derive(Debug)]
pub struct DefaultApprovalManager {
    pending_approvals: DashMap<ApprovalId, oneshot::Sender<ApprovalResponse>>,
    event_publisher: Arc<dyn EventPublisher>,
}

impl DefaultApprovalManager {
    /// Create a new `DefaultApprovalManager`.
    pub fn new(event_publisher: Arc<dyn EventPublisher>) -> Self {
        Self {
            pending_approvals: DashMap::new(),
            event_publisher,
        }
    }
}

#[async_trait]
impl ApprovalManager for DefaultApprovalManager {
    fn request_approval(&self, request: ApprovalRequest) -> oneshot::Receiver<ApprovalResponse> {
        let (tx, rx) = oneshot::channel();
        let id = request.id;
        self.pending_approvals.insert(id, tx);
        self.event_publisher.publish(GlobalEvent::ApprovalRequested(request));
        rx
    }

    fn respond(&self, id: ApprovalId, response: ApprovalResponse) -> anyhow::Result<()> {
        if let Some((_, tx)) = self.pending_approvals.remove(&id) {
            tx.send(response).map_err(|_| anyhow::anyhow!("Failed to send approval response"))?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Approval request not found: {}", id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::agent::AgentId;
    use caelumora_model::approval::ApprovalStatus;
    use caelumora_model::channel::ChannelId;
    use caelumora_model::message::ToolCall;
    use caelumora_model::session::SessionId;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct MockPublisher {
        events: Mutex<Vec<GlobalEvent>>,
    }
    impl EventPublisher for MockPublisher {
        fn publish(&self, event: GlobalEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn test_approval_flow() {
        let publisher = Arc::new(MockPublisher::default());
        let manager = DefaultApprovalManager::new(publisher.clone());

        let request = ApprovalRequest {
            id: ApprovalId::new(),
            session_id: SessionId::new(),
            channel_id: ChannelId::new(),
            agent_id: AgentId::new(),
            tool_call: ToolCall {
                id: Some("call_1".to_string()),
                function_name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let id = request.id;

        let rx = manager.request_approval(request.clone());
        
        // Check if event was published
        {
            let events = publisher.events.lock().unwrap();
            assert_eq!(events.len(), 1);
            if let GlobalEvent::ApprovalRequested(ref req) = events[0] {
                assert_eq!(req.id, id);
            } else {
                panic!("Expected ApprovalRequested event");
            }
        }

        let response = ApprovalResponse {
            id,
            status: ApprovalStatus::Approved,
        };

        manager.respond(id, response.clone()).unwrap();

        let result = rx.await.unwrap();
        assert_eq!(result.id, id);
        assert_eq!(result.status, ApprovalStatus::Approved);
    }
}
