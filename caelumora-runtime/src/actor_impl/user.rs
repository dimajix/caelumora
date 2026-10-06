use crate::actor::Actor;
use crate::command::{CommandContext, CommandReply};
use crate::runtime::ActorRuntime;
use crate::transport::{EventReceiver, EventSender, TransportError};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope, EventContext};
use caelumora_model::approval::ApprovalResponse;
use caelumora_model::channel::{Channel, ChannelId};
use caelumora_model::command::{Command, CommandVisibility};
use caelumora_model::message::{Message, MessageId, MessageRole};
use caelumora_model::confidentiality::Classification;
use caelumora_model::security::{ActionType, Enforcement, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use crate::controller::UserController;
use chrono::Utc;
use tracing::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How many envelopes a watcher of the actor may fall behind before it
/// loses the oldest (see `UserActor::watch`). A power of two, since a
/// broadcast rounds up to one (as the `EventBus`, issue 38).
const SUBSCRIBER_CAPACITY: usize = 128;

#[derive(Debug)]
pub struct UserActor {
    id: ActorId,
    session_id: SessionId,
    channel_id: ChannelId,
    security_context: SecurityContext,
    user: Arc<UserController>,
    actor_runtime: ActorRuntime,
    // Envelopes built by `send_message` / `send_approval`, forwarded into the
    // channel by `run`.
    user_rx: Mutex<Receiver<Envelope>>,
    user_tx: Sender<Envelope>,
    // Where envelopes routed to this user go, to everyone who listens:
    // see `watch` (issue 220).
    received: broadcast::Sender<Envelope>,
    // Cancelled when the actor ends - it left the channel, or the channel
    // stopped: what ends every `watch` (issue 104).
    ended: CancellationToken,
}

/// The channel as a user follows it (issues 104, 220): what is routed to
/// their actor and what the actor says for them, until `ended` - when the
/// actor ends, because they left the channel or it stopped. Only a
/// participant follows a channel live, and only through their actor.
pub struct ChannelWatch {
    pub events: broadcast::Receiver<Envelope>,
    pub ended: CancellationToken,
}

impl UserActor {
    /// The actor of the user `user` - the session's controller of them,
    /// shared with their actors in its other channels (issue 120).
    pub async fn new(channel: &Channel, user: Arc<UserController>, actor_runtime: ActorRuntime) -> Result<Self> {
        let user_id = user.id();
        // The user acts on their own behalf, in this channel (issue 87).
        let security_context = SecurityContext::new(ActorId::User(user_id), channel.session_id, channel.id);

        let (utx,urx) = mpsc::channel(100);

        Ok(Self {
            id: ActorId::User(user_id),
            session_id: channel.session_id,
            channel_id: channel.id,
            security_context,
            user,
            actor_runtime,
            user_rx: Mutex::new(urx),
            user_tx: utx,
            received: broadcast::channel(SUBSCRIBER_CAPACITY).0,
            ended: CancellationToken::new(),
        })
    }

    /// The context this user acts in, here - what their answers carry, so
    /// that whoever decides on them asks about them in the same one (the
    /// approver, issue 14).
    pub(crate) fn security_context(&self) -> &SecurityContext {
        &self.security_context
    }

    /// An envelope from this user. Envelopes are only ever built here, so the
    /// sender, session, channel and security context are always this actor's
    /// own, whatever the caller of `send_message` is.
    fn envelope(&self, event: ActorEvent, span_id: Uuid) -> Envelope {
        let mut context = EventContext::new(self.session_id, self.channel_id, self.id(), self.name());
        context.span_id = span_id;
        Envelope::new(self.security_context.clone(), context, event)
    }

    /// Sends a message from this user into the channel, classified
    /// `classification` - the channel's if `None` (issue 133). A user may
    /// release what they write: classify it for a wider audience or a
    /// lower level than the channel, as long as it may flow into the
    /// channel. Asked here, so that the user learns of a refusal; the
    /// channel asks again as it admits the message.
    ///
    /// Returns the `span_id` of the envelopes it starts, which every envelope
    /// caused by the message carries as well (see `crate::logging`).
    ///
    /// # Errors
    /// `ChannelError::ClassificationRefused` if the message may not enter
    /// the channel as classified; an error if the channel is gone.
    pub async fn send_message(&self, message: String, classification: Option<Classification>) -> anyhow::Result<Uuid> {
        let channel_loop = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))?;
        let channel = channel_loop.classification();
        let classification = classification.unwrap_or_else(|| channel.clone());
        let action = ActionType::Publish { classification: &classification, actor: self.id(), channel: &channel };
        self.actor_runtime.security_manager()
            .require(&self.security_context, action, || crate::channel::ChannelError::ClassificationRefused { classification: classification.clone(), channel: self.channel_id })
            .await?;
        let msg = Message {
            role: MessageRole::User,
            mentions: channel_loop.mentions_in(&message),
            content: Some(message),
            id: MessageId(Uuid::new_v4()),
            sender_id: self.id(),
            sender_name: self.name(),
            timestamp: Utc::now(),
            ..Message::new(classification)
        };
        // All stem from one user action: one span. First the request to wake
        // the channel's agents for it - the channel does not wake by itself
        // (issue 90).
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::WakeupRequested, span_id)).await?;
        self.send(self.envelope(ActorEvent::CompleteMessageGenerated(msg.clone()), span_id)).await?;
        self.send(self.envelope(ActorEvent::FullResponseGenerated(vec![msg]), span_id)).await?;
        Ok(span_id)
    }

    /// Modifies this user's message `message` to read `content` from now on
    /// (issue 234), classified `classification` - as it is classified now
    /// if `None`. Asked here, so that the user learns of a refusal - whether
    /// they may modify it (`ModifyMessage`), then whether it may enter the
    /// channel so classified (`Publish`) - and asked again by the channel as
    /// it admits the modification. The message keeps its place; nobody is
    /// asked to respond to it.
    ///
    /// Returns the `span_id` of the envelope, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message,
    /// `Forbidden` if it is not this user's to modify, `MessageRevoked` if
    /// it was taken back, `ClassificationRefused` as for `send_message`; an
    /// error if the channel is gone.
    pub async fn modify_message(&self, message: MessageId, content: String, classification: Option<Classification>) -> anyhow::Result<Uuid> {
        let channel_loop = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))?;
        let original = self.message_to_change(&channel_loop, message, |author, channel| ActionType::ModifyMessage { author, channel }, "modifying a message of").await?;
        let channel = channel_loop.classification();
        let classification = classification.unwrap_or_else(|| original.classification.clone());
        let action = ActionType::Publish { classification: &classification, actor: self.id(), channel: &channel };
        self.actor_runtime.security_manager()
            .require(&self.security_context, action, || crate::channel::ChannelError::ClassificationRefused { classification: classification.clone(), channel: self.channel_id })
            .await?;
        let modified = Message {
            mentions: channel_loop.mentions_in(&content),
            content: Some(content),
            classification,
            timestamp: Utc::now(),
            ..original
        };
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::MessageModified(modified), span_id)).await?;
        Ok(span_id)
    }

    /// Takes this user's message `message` back (issue 234): it keeps its
    /// place, without what it said, and the agents are no longer given it.
    /// Asked here (`RevokeMessage`) and again by the channel, as
    /// `modify_message`. Not a deletion: what was said stays in the record
    /// of every participant that heard it.
    ///
    /// # Errors
    /// As `modify_message`, but for the classification.
    pub async fn revoke_message(&self, message: MessageId) -> anyhow::Result<Uuid> {
        let channel_loop = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))?;
        self.message_to_change(&channel_loop, message, |author, channel| ActionType::RevokeMessage { author, channel }, "taking back a message of").await?;
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::MessageRevoked { message, at: Utc::now() }, span_id)).await?;
        Ok(span_id)
    }

    /// Asks the agent that said `message` to answer again instead of it
    /// (issue 234). Asked here (`RetryMessage`, whoever may read the
    /// channel), and again by the channel as it admits the request, which
    /// goes to that agent alone: whether it can answer again is the agent's
    /// to decide, by its own transcript, and it says so with `RetryAccepted`
    /// or `RetryRejected` - this returns once the request is on its way. A
    /// `WakeupRequested` goes ahead of it, as of a message, so that a quiet
    /// channel does not swallow it.
    ///
    /// Returns the `span_id` of the envelopes, as `send_message` does.
    ///
    /// # Errors
    /// `ChannelError::MessageNotFound` if the channel has no such message,
    /// `Forbidden` if this user may not ask it, `MessageRevoked` if it was
    /// taken back, `NotAnAgentsMessage` if no agent of the channel said it;
    /// an error if the channel is gone.
    pub async fn request_retry(&self, message: MessageId) -> anyhow::Result<Uuid> {
        use crate::channel::ChannelError;
        let channel_loop = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("Channel {} is gone", self.channel_id))?;
        let said = channel_loop.message(message).await
            .ok_or(ChannelError::MessageNotFound { message, channel: self.channel_id })?;
        let attributes = channel_loop.security_attributes();
        self.actor_runtime.security_manager()
            .require(&self.security_context, ActionType::RetryMessage(&attributes), || ChannelError::Forbidden { action: "asking for another answer in", channel: self.channel_id })
            .await?;
        if said.revoked.is_some() {
            return Err(ChannelError::MessageRevoked { message, channel: self.channel_id }.into());
        }
        let author = said.sender_id;
        if author.as_agent_id().is_none() || !channel_loop.actor_ids().contains(&author) {
            return Err(ChannelError::NotAnAgentsMessage { message, channel: self.channel_id }.into());
        }
        let span_id = Uuid::new_v4();
        self.send(self.envelope(ActorEvent::WakeupRequested, span_id)).await?;
        self.send(self.envelope(ActorEvent::RetryRequested { message, author }, span_id)).await?;
        Ok(span_id)
    }

    /// The message `id` of `channel` as it stands, if this user may change
    /// it - `action`, asked as `what` - and it can still be changed: not
    /// taken back.
    async fn message_to_change(
        &self,
        channel: &crate::channel::ChannelLoop,
        id: MessageId,
        action: for<'a> fn(caelumora_model::actor::ActorId, &'a SecurityAttributes) -> ActionType<'a>,
        what: &'static str,
    ) -> anyhow::Result<Message> {
        use crate::channel::ChannelError;
        let message = channel.message(id).await
            .ok_or(ChannelError::MessageNotFound { message: id, channel: self.channel_id })?;
        let attributes = channel.security_attributes();
        self.actor_runtime.security_manager()
            .require(&self.security_context, action(message.sender_id, &attributes), || ChannelError::Forbidden { action: what, channel: self.channel_id })
            .await?;
        if message.revoked.is_some() {
            return Err(ChannelError::MessageRevoked { message: id, channel: self.channel_id }.into());
        }
        Ok(message)
    }

    /// Hands `envelope` to `run`, which forwards it into the channel, and
    /// shows it to those who follow this actor: the channel sends nothing
    /// back to its sender, so as an agent takes its own messages in itself,
    /// this actor shows what its user says itself (issue 220) - all but the
    /// request to wake the channel, which is not said to anyone.
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()> {
        let shown = (!matches!(envelope.event, ActorEvent::WakeupRequested)).then(|| envelope.clone());
        self.user_tx.send(envelope).await?;
        if let Some(envelope) = shown {
            // An error only says that nobody listens.
            let _ = self.received.send(envelope);
        }
        Ok(())
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
        self.send(self.envelope(event, span_id)).await?;
        Ok(span_id)
    }

    /// Runs the user command `name` with `args` on behalf of this user
    /// (issue 28). Commands are for people, so only a `UserActor` runs them.
    ///
    /// A command answers either this user alone or the whole channel; in the
    /// latter case this actor announces it as a `CommandExecuted`, in its own
    /// name. The answer is returned in both cases. Commands never enter the
    /// history.
    ///
    /// # Errors
    /// A `CommandError` if the command is unknown, refused or fails; an error
    /// if the channel no longer exists.
    pub async fn execute_command(&self, name: &str, args: &str) -> anyhow::Result<CommandReply> {
        let channel = self.actor_runtime.channel_loop()
            .ok_or_else(|| anyhow!("channel {} no longer exists", self.channel_id))?;
        let executor = self.actor_runtime.command_executor();
        let context = CommandContext { issuer: &self.security_context, channel: &channel, executor };
        debug!("UserActor {} runs /{}", self.id, name);
        let output = executor.execute(name, args, &context).await?;
        if output.visibility == CommandVisibility::Channel {
            let event = ActorEvent::CommandExecuted { command: name.to_string(), output: output.text.clone() };
            self.send(self.envelope(event, Uuid::new_v4())).await?;
        }
        Ok(CommandReply { command: name.to_string(), output: output.text, visibility: output.visibility })
    }

    /// The commands this user may see, e.g. for completion.
    pub async fn visible_commands(&self) -> Vec<Command> {
        self.actor_runtime.command_executor().visible(&self.security_context).await
    }

    /// Follows the channel as this user, from now on: see `ChannelWatch`.
    /// Any number may follow - two tabs, two devices (issue 220). One that
    /// falls `SUBSCRIBER_CAPACITY` envelopes behind loses the oldest
    /// (`RecvError::Lagged`) rather than stalling this actor, which would
    /// also stop it from forwarding what the user sends; without anyone
    /// following, envelopes are dropped.
    pub fn watch(&self) -> ChannelWatch {
        // A child: whoever watches can end their own watch, not the actor.
        ChannelWatch { events: self.received.subscribe(), ended: self.ended.child_token() }
    }

    fn handle_actor_envelope(&self, maybe_event: Option<Envelope>) -> bool {
        match maybe_event {
            Some(event) => {
                debug!("UserActor {} received {}", self.id(), event.describe());
                // An error only says that nobody listens.
                let _ = self.received.send(event);
                false
            }
            None => true,
        }
    }

    /// Forwards what the user sent into the channel. `false` once the
    /// channel's inbox is gone - the only way the in-memory transport fails
    /// - and with it the channel: the actor then ends (issue 79).
    async fn forward(&self, envelope: Envelope, tx: &dyn EventSender) -> bool {
        debug!(span_id = %envelope.context.span_id, "UserActor {} sends {}", self.id(), envelope.event.kind());
        match tx.send(envelope).await {
            Ok(()) => true,
            Err(TransportError::Closed) => {
                info!("UserActor {} ends: its channel is gone", self.id());
                false
            }
            Err(e) => {
                warn!("UserActor {} could not forward a message: {}", self.id(), e);
                true
            }
        }
    }
}

/// The end of the actor's task, however it ends - also when it is aborted
/// with its channel: the queue closes, so that what the user sends to a
/// stopped channel fails at once instead of filling a queue nobody reads
/// (issue 90), and every watch ends (issue 104).
struct EndOfRun<'a> {
    queue: tokio::sync::MutexGuard<'a, Receiver<Envelope>>,
    ended: &'a CancellationToken,
}

impl Drop for EndOfRun<'_> {
    fn drop(&mut self) {
        self.queue.close();
        self.ended.cancel();
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

    /// Ends the actor: it left the channel (issue 46). Its task ends, and
    /// with it every watch.
    fn stop(&self) {
        self.ended.cancel();
    }

    fn event_filter(&self) -> fn(&Envelope) -> bool {
        |_| true
    }

    async fn run(
        &self,
        mut rx: Box<dyn EventReceiver>,
        tx: Box<dyn EventSender>,
    ) -> anyhow::Result<()> {
        let queue = self.user_rx.try_lock()
            .map_err(|_| anyhow!("UserActor {} is already running", self.id()))?;
        let mut run = EndOfRun { queue, ended: &self.ended };

        info!("UserActor {} is starting", self.id());
        // Until the channel closes this actor's inbox - the user left, or
        // the channel stopped - or the actor is told to stop.
        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    if self.handle_actor_envelope(maybe_event) { break }
                }
                Some(envelope) = run.queue.recv() => {
                    if !self.forward(envelope, &*tx).await { break }
                }
                _ = self.ended.cancelled() => break,
            }
        }
        info!("UserActor {} has finished", self.id());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::name::Name;
    use caelumora_model::command::default_commands;
    use crate::command::CommandExecutor;
    use crate::runtime::{ChannelRuntime, RuntimeManager};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes};
    use caelumora_model::store_impl::memory::MemoryStore;

    /// A user's actor, with the channel it is in - to be kept while the
    /// actor is used: the actor's runtime holds it weakly.
    async fn user_actor() -> (UserActor, Arc<crate::channel::ChannelLoop>) {
        let store = Arc::new(MemoryStore::new());
        let secret_manager = Arc::new(SecretManager::new(vec![], Arc::new(DummySecurityManager::new())));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secret_manager, Arc::new(DummySecurityManager::new()), default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let channel_runtime = ChannelRuntime::new(Arc::clone(&runtime), Arc::new(CommandExecutor::empty(Arc::clone(runtime.security_manager()))));
        let channel = Channel {
            id: ChannelId::new(),
            session_id: SessionId::new(),
            name: Name::new("main").unwrap(),
            components: caelumora_model::channel::moderation_components(None, Some("broadcast_policy")),
            participants: Vec::new(),
            security_attributes: SecurityAttributes::default(),
            classification: caelumora_model::confidentiality::Classification::CHANNEL_DEFAULT,
            language: None,
            history_limits: Vec::new(),
            deleted: None,
            former: Vec::new(),
        };
        let user = caelumora_model::user::User {
            id: caelumora_model::user::UserId::new(),
            username: "alice".to_string(),
            email: String::new(),
            roles: Vec::new(),
            deleted: None,
        };
        let channel_loop = crate::channel::ChannelLoop::new(channel.clone(), channel_runtime.clone()).await.unwrap();
        let actor_runtime = channel_runtime.new_actor_runtime(ActorId::User(user.id), channel.session_id, channel.id);
        runtime.entity_store().create_user(&SecurityContext::system(), &user).await.unwrap();
        let actor = UserActor::new(&channel, runtime.controller_manager().user_controller(user.id).await.unwrap(), actor_runtime).await.unwrap();
        (actor, channel_loop)
    }

    fn envelope_from(sender: ActorId) -> Envelope {
        let context = EventContext::new(Default::default(), Default::default(), sender, "agent".to_string());
        Envelope::new(SecurityContext::default(), context, ActorEvent::ResponseStarted)
    }

    #[tokio::test]
    async fn send_message_asks_for_a_wakeup_and_returns_the_span_of_all_its_envelopes() {
        let (actor, _channel) = user_actor().await;

        let span_id = actor.send_message("hello".to_string(), None).await.unwrap();

        let mut rx = actor.user_rx.lock().await;
        let wakeup = rx.recv().await.expect("WakeupRequested");
        let complete = rx.recv().await.expect("CompleteMessageGenerated");
        let full = rx.recv().await.expect("FullResponseGenerated");
        assert!(matches!(wakeup.event, ActorEvent::WakeupRequested), "the wakeup first, so that the agents are awake when the message comes");
        assert!(matches!(complete.event, ActorEvent::CompleteMessageGenerated(_)));
        assert!(matches!(full.event, ActorEvent::FullResponseGenerated(_)));
        for envelope in [&wakeup, &complete, &full] {
            assert_eq!(envelope.context.span_id, span_id, "one user action, one span");
            assert_eq!(envelope.context.sender_id, actor.id());
            assert_eq!(envelope.context.channel_id, actor.channel_id);
        }
        assert_ne!(complete.context.event_id, full.context.event_id, "two events, two event ids");
    }

    #[tokio::test]
    async fn the_actor_shows_its_own_messages_to_those_who_follow_it() {
        let (actor, _channel) = user_actor().await;
        let mut following = actor.watch().events;

        actor.send_message("hello".to_string(), None).await.unwrap();

        let shown: Vec<&str> = std::iter::from_fn(|| following.try_recv().ok()).map(|e| e.event.kind()).collect();
        assert_eq!(shown, ["CompleteMessageGenerated", "FullResponseGenerated"], "what it said, not the request to wake the channel (issue 220)");

        actor.send_approval(ApprovalResponse { id: Default::default(), agent_id: Default::default() }, true).await.unwrap();
        assert_eq!(following.try_recv().map(|e| e.event.kind()).ok(), Some("ApprovalGranted"), "everything it sends");
    }

    #[tokio::test]
    async fn a_user_actor_whose_channel_is_gone_ends_without_an_error() {
        use crate::transport_impl::{InMemoryEventReceiver, InMemoryEventSender};
        let (actor, _channel) = user_actor().await;
        let (_inbox, rx) = mpsc::channel(1);
        let (output, channel_inbox) = mpsc::channel(1);
        drop(channel_inbox);

        actor.send_message("hello".to_string(), None).await.unwrap();
        let ended = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            actor.run(Box::new(InMemoryEventReceiver::new(rx)), Box::new(InMemoryEventSender::new(output))),
        ).await.expect("the actor ends");
        assert!(ended.is_ok(), "a channel that is gone is an end, not an error: {ended:?}");
    }

    #[tokio::test]
    async fn envelopes_without_a_subscriber_are_dropped_without_blocking() {
        let (actor, _channel) = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        // More than any queue would hold: must neither block nor fail.
        for _ in 0..(SUBSCRIBER_CAPACITY * 2) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent))));
        }
        assert!(actor.handle_actor_envelope(None), "a closed input ends the actor");
    }

    #[tokio::test]
    async fn every_subscriber_receives_what_is_routed_and_a_lagging_one_loses_the_oldest() {
        let (actor, _channel) = user_actor().await;
        let agent = ActorId::Agent(Default::default());
        let (mut first, mut second) = (actor.watch().events, actor.watch().events);

        assert!(!actor.handle_actor_envelope(Some(envelope_from(agent))));
        assert!(first.try_recv().is_ok() && second.try_recv().is_ok(), "two tabs, both are told");

        for _ in 0..(SUBSCRIBER_CAPACITY + 10) {
            assert!(!actor.handle_actor_envelope(Some(envelope_from(agent))));
        }
        assert!(matches!(first.try_recv(), Err(broadcast::error::TryRecvError::Lagged(10))), "the oldest are lost, and it learns how many");
        let mut received = 0;
        while first.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, SUBSCRIBER_CAPACITY, "the newest are kept");
    }


}
