use crate::actor::Actor;
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::ApprovalResponse;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::event::GlobalEvent;
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::security::SecurityContext;
use caelumora_model::session::SessionId;
use caelumora_model::user::{User, UserController, UserId};
use chrono::Utc;
use tracing::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

/// Capacity of the queue towards a subscriber (see `UserActor::subscribe`).
const SUBSCRIBER_CAPACITY: usize = 100;

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    user_id: UserId,
    session_id: SessionId,
    channel_id: ChannelId,
    security_context: SecurityContext,
    user: Arc<UserController>,
    actor_runtime: ActorRuntime,
    // Envelopes built by `send_message` / `send_approval`, forwarded into the
    // channel by `run`.
    user_rx: Mutex<Receiver<Envelope>>,
    user_tx: Sender<Envelope>,
    // Where envelopes routed to this user go, if anyone listens: see
    // `subscribe`. A plain lock, never held across an await.
    subscriber: std::sync::Mutex<Option<Sender<Envelope>>>,
}

impl UserActor {
    pub async fn new(channel: &Channel, mut user:User, actor_runtime: ActorRuntime) -> Result<Self> {
        user.security.session_id = channel.session_id;
        user.security.channel_id = channel.id;
        let user_id = user.id;
        let security_context = user.security.clone();
        let user_controller = actor_runtime.controller_manager().new_user_controller(user);

        let (utx,urx) = mpsc::channel(100);

        Ok(Self {
            id: ActorId::User(user_id),
            user_id,
            session_id: channel.session_id,
            channel_id: channel.id,
            security_context,
            user: user_controller,
            actor_runtime,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            subscriber: std::sync::Mutex::new(None),
        })
    }

    /// An envelope from this user. Envelopes are only ever built here, so the
    /// sender, session, channel and security context are always this actor's
    /// own, whatever the caller of `send_message` is.
    fn envelope(&self, event: ActorEvent, span_id: Uuid) -> Envelope {
        let mut context = EventContext::new(self.session_id, self.channel_id, self.id(), self.name());
        context.span_id = span_id;
        Envelope::new(self.security_context.clone(), context, event)
    }

    /// Sends a message from this user into the channel.
    ///
    /// Returns the `span_id` of the envelopes it starts, which every envelope
    /// caused by the message carries as well (see `crate::logging`).
    pub async fn send_message(&self, message: String) -> anyhow::Result<Uuid> {
        let msg = Message {
            role: MessageRole::User,
            content: Some(message),
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name(),
            timestamp: Utc::now(),
            ..Message::default()
        };
        // Both envelopes stem from one user action: one span, two events.
        let span_id = Uuid::new_v4();
        self.user_tx.send(self.envelope(ActorEvent::CompleteMessageGenerated(msg.clone()), span_id)).await?;
        self.user_tx.send(self.envelope(ActorEvent::FullResponseGenerated(vec![msg]), span_id)).await?;
        Ok(span_id)
    }

    /// Sends this user's decision on an approval. Returns the `span_id`, as
    /// `send_message` does.
    pub async fn send_approval(&self, response: ApprovalResponse, approved: bool) -> anyhow::Result<Uuid> {
        let event = if approved {
            ActorEvent::ApprovalGranted(response)
        } else {
            ActorEvent::ApprovalDenied(response)
        };
        let span_id = Uuid::new_v4();
        self.user_tx.send(self.envelope(event, span_id)).await?;
        Ok(span_id)
    }

    /// Receives the envelopes routed to this user from now on, e.g. for a
    /// terminal UI. There is one subscriber at a time; subscribing again
    /// replaces the previous one.
    ///
    /// Without a subscriber, routed envelopes are dropped - the REST server
    /// reads the channel's broadcast instead and never subscribes here. A
    /// subscriber that falls `SUBSCRIBER_CAPACITY` envelopes behind loses the
    /// newest ones rather than stalling this actor, which would also stop it
    /// from forwarding what the user sends.
    pub fn subscribe(&self) -> Receiver<Envelope> {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_CAPACITY);
        if let Ok(mut subscriber) = self.subscriber.lock() {
            *subscriber = Some(tx);
        }
        rx
    }

    fn handle_actor_envelope(&self, maybe_event: Option<Envelope>) -> bool {
        match maybe_event {
            Some(event) => {
                debug!("UserActor {} received {}", self.id(), event.describe());
                let Ok(mut subscriber) = self.subscriber.lock() else {
                    return false;
                };
                if let Some(tx) = subscriber.as_ref() {
                    match tx.try_send(event) {
                        Ok(()) => {}
                        Err(TrySendError::Full(event)) => {
                            warn!("UserActor {}: subscriber is not keeping up, dropping {}", self.id(), event.describe());
                        }
                        Err(TrySendError::Closed(_)) => {
                            *subscriber = None;
                        }
                    }
                }
                false
            }
            None => true,
        }
    }

    async fn handle_user_event(
        &self,
        maybe_envelope: Option<Envelope>,
        tx: &dyn EventSender,
    ) -> Result<bool> {
        match maybe_envelope {
            Some(envelope) => {
                debug!(span_id = %envelope.context.span_id, "UserActor {} sends {}", self.id(), envelope.event.kind());
                tx.send(envelope)
                    .await
                    .context(format!("UserActor {} failed to forward user message", self.id))?;
                Ok(false)
            }
            None => Ok(true),
        }
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) -> bool {
        let mut global_rx_closed = false;
        match maybe_global {
            Some(event) => {
                self.user.process_global_event(&event);
            }
            None => {
                global_rx_closed = true;
            }
        }
        global_rx_closed
    }
}

#[async_trait]
impl Actor for UserActor {
    fn id(&self) -> ActorId {
        self.id
    }

    fn kind(&self) -> String{
        "user".to_string()
    }

    fn name(&self) -> String {
        self.user.username()
    }

    fn state(&self) -> ActorState {
        ActorState::Idle
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> anyhow::Result<()> {
        let mut user_rx = self.user_rx.try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;

        let user_id = self.user_id;
        let mut global_rx = self.actor_runtime.event_bus().subscribe(vec![
            crate::event::SubscriptionTopic::User(user_id),
            crate::event::SubscriptionTopic::Channel(self.channel_id),
        ]);
        let mut rx_closed = false;
        let mut user_rx_closed = false;
        let mut global_rx_closed = false;

        info!("UserActor {} is starting", self.id());
        while !rx_closed || !user_rx_closed || !global_rx_closed {
            tokio::select! {
                maybe_event = rx.recv(), if !rx_closed => {
                    rx_closed = self.handle_actor_envelope(maybe_event);
                }
                maybe_message = user_rx.recv(), if !user_rx_closed => {
                    user_rx_closed = self.handle_user_event(maybe_message, &*tx).await?;
                }
                maybe_global = global_rx.recv(), if !global_rx_closed => {
                    global_rx_closed = self.handle_global_event(maybe_global).await;
                }
                else => break,
            }
        }
        info!("UserActor {} has finished", self.id());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeManager;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;

    async fn user_actor() -> UserActor {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = RuntimeManager::new(store.clone(), store, secret_manager).unwrap();
        let channel = Channel::new(SessionId::new(), "main".to_string(), "broadcast".to_string(), Vec::new(), SecurityAttributes::default());
        let user = User {
            id: UserId::new(),
            username: "alice".to_string(),
            email: String::new(),
            roles: Vec::new(),
            security: SecurityContext::default(),
        };
        UserActor::new(&channel, user, runtime.new_actor_runtime()).await.unwrap()
    }

    fn envelope_from(sender: ActorId) -> Envelope {
        let context = EventContext::new(Default::default(), Default::default(), sender, "agent".to_string());
        Envelope::new(SecurityContext::default(), context, ActorEvent::ResponseStarted)
    }

    #[tokio::test]
    async fn send_message_returns_the_span_id_of_both_envelopes() {
        let actor = user_actor().await;

        let span_id = actor.send_message("hello".to_string()).await.unwrap();

        let mut rx = actor.user_rx.lock().await;
        let complete = rx.recv().await.expect("CompleteMessageGenerated");
        let full = rx.recv().await.expect("FullResponseGenerated");
        assert!(matches!(complete.event, ActorEvent::CompleteMessageGenerated(_)));
        assert!(matches!(full.event, ActorEvent::FullResponseGenerated(_)));
        for envelope in [&complete, &full] {
            assert_eq!(envelope.context.span_id, span_id, "one user action, one span");
            assert_eq!(envelope.context.sender_id, actor.id());
            assert_eq!(envelope.context.channel_id, actor.channel_id);
        }
        assert_ne!(complete.context.event_id, full.context.event_id, "two events, two event ids");
    }

    #[tokio::test]
    async fn envelopes_without_a_subscriber_are_dropped_without_blocking() {
        let actor = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        // More than any queue would hold: must neither block nor fail.
        for _ in 0..(SUBSCRIBER_CAPACITY * 2) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent))));
        }
        assert!(actor.handle_actor_envelope(None), "a closed input ends the actor");
    }

    #[tokio::test]
    async fn a_lagging_subscriber_loses_envelopes_but_does_not_stall_the_actor() {
        let actor = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        let mut rx = actor.subscribe();

        for _ in 0..(SUBSCRIBER_CAPACITY + 10) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent))));
        }

        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, SUBSCRIBER_CAPACITY, "the queue holds what it can, the rest is dropped");

        drop(rx);
        actor.handle_actor_envelope(Some(envelope_from(agent)));
        assert!(actor.subscriber.lock().unwrap().is_none(), "a closed subscriber is removed");
    }
}
