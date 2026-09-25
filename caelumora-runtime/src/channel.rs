use crate::actor::Actor;
use crate::actor_impl::agent::AgentActor;
use crate::actor_impl::approval::ApproverActor;
use crate::actor_impl::user::UserActor;
use crate::moderator::Moderator;
use crate::recorder::Recorder;
use crate::runtime::ChannelRuntime;
use crate::transport::{EventSender, TransportError};
use crate::transport_impl::{FilteringEventSender, InMemoryEventReceiver, InMemoryEventSender};
use anyhow::{anyhow, Result};
use caelumora_model::actor::{ActorEvent, ActorId, ActorState, Envelope};
use caelumora_model::channel::{Channel, ChannelController, ChannelError, ChannelId, ChannelState, Participant};
use caelumora_model::event::GlobalEvent;
use caelumora_model::mention::{handle_of, MentionDirectory};
use caelumora_model::message::Message;
use caelumora_model::security::{ActionType, SecurityAttributes, SecurityContext};
use caelumora_model::session::SessionId;
use caelumora_model::store::{StateStore, StoreError};
use caelumora_model::user::UserId;
use tracing::{debug, info, trace, warn, Instrument};
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

/// A participant of a channel, with the handle it is `@`-mentioned by.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMember {
    pub id: ActorId,
    pub name: String,
    /// The handle the participant is mentioned by (see `caelumora_model::mention`),
    /// or `None` if it cannot be mentioned - approvers, and names that yield
    /// no handle.
    pub handle: Option<String>,
    /// The actor kind, e.g. `"user"` or the agent's kind.
    pub kind: String,
}

struct ActorHandle {
    id: ActorId,
    abort_handle: AbortHandle,
    sender: Box<dyn EventSender>,
}

#[derive(Debug)]
pub struct ChannelLoop {
    id: ChannelId,
    session_id: SessionId,
    channel: Arc<ChannelController>,
    runtime: ChannelRuntime,
    actors: HashMap<ActorId,Arc<dyn Actor>>,
    moderator: Mutex<Box<dyn Moderator>>,
    mentions: MentionDirectory,
    recorder: Mutex<Box<dyn Recorder>>,
    state: Arc<Mutex<ChannelState>>,
    state_store: Arc<dyn StateStore>,
    processing_state: Arc<AtomicU8>,
    event_tx: broadcast::Sender<Envelope>,
    max_route_decisions: i32,
    run_lock: Mutex<()>
}

impl ChannelLoop {
    /// Builds the channel and its actors, whose runtimes `runtime` creates and
    /// which reach the channel through it once it exists.
    pub async fn new(channel: Channel, runtime: ChannelRuntime) -> Result<Arc<Self>> {
        let id = channel.id;
        let session_id = channel.session_id;
        let state_store = runtime.state_store();
        let mut moderator = runtime.new_moderator(&channel.moderator)?;
        
        let state = match state_store.get_channel_state(id).await {
            Ok(s) => s,
            Err(StoreError::NotFound { entity: _, id: _ }) => {
                let state = ChannelState::new(id);
                state_store.put_channel_state(id, &state).await?;
                state
            }
            Err(e) => return Err(e.into()),
        };
        let state = Arc::new(Mutex::new(state));

        let recorder = runtime.new_recorder()?;
        let (event_tx, _) = broadcast::channel(100);

        let channel_controller = runtime.controller_manager().new_channel_controller(channel);

        let mut actors = HashMap::new();
        for participant in &channel_controller.load().participants {
            let actor = Self::create_actor(channel_controller.clone(), participant, &runtime).await?;
            let actor: Arc<dyn Actor> = Arc::from(actor);
            moderator.add_actor(actor.clone());
            actors.insert(actor.id(), actor);
        }
        let mentions = Self::mention_directory(&actors);
        for handle in mentions.ambiguous() {
            warn!("Channel {}: handle @{} is shared by several participants; mentioning it addresses all of them", id, handle);
        }

        let channel_loop = Arc::new(Self {
            id,
            session_id,
            channel: channel_controller,
            runtime,
            actors,
            moderator: Mutex::new(moderator),
            mentions,
            recorder: Mutex::new(recorder),
            state,
            state_store,
            processing_state: Arc::new(AtomicU8::new(ActorState::Idle.into())),
            event_tx,
            max_route_decisions: 100,
            run_lock: Mutex::new(())
        });
        channel_loop.runtime.attach(&channel_loop);
        Ok(channel_loop)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.event_tx.subscribe()
    }

    pub async fn history(&self) -> Vec<Message> {
        self.state.lock().await.history.messages()
    }


    pub fn id(&self) -> ChannelId { self.id }
    
    pub fn name(&self) -> String {
        self.channel.name()
    }

    /// Who the channel belongs to and who contributes to it (issue 11).
    pub fn security_attributes(&self) -> SecurityAttributes {
        self.channel.load().security_attributes.clone()
    }

    pub async fn update_name(&self, new_name: String) {
        self.channel.update_name(new_name).await;
    }

    /// Ends the current turn of every actor in the channel, on behalf of
    /// `context`'s initiator.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `CancelChannel`; the channel is then untouched. Deliberately one
    /// decision for the whole channel: a per-actor one could leave half of it
    /// cancelled (issue 30).
    pub async fn cancel(&self, context: &SecurityContext) -> Result<()> {
        let channel = self.channel.load();
        self.check(context, ActionType::CancelChannel(&channel.security_attributes), "cancelling").await?;
        self.processing_state.store(ActorState::Cancelling.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.cancel().await?;
        }
        Ok(())
    }

    /// Wakes the channel's sleeping actors because someone asked for it -
    /// a `/wakeup` and nothing else. Returns the names of the actors that
    /// were asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`.
    pub async fn wakeup(&self, context: &SecurityContext) -> Result<Vec<String>> {
        let channel = self.channel.load();
        self.check(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let asleep = self.actors.values()
            .filter(|a| a.state() == ActorState::Sleeping)
            .map(|a| a.name())
            .collect();
        self.wake_for_delivery().await?;
        Ok(asleep)
    }

    /// Wakes one actor deliberately, like `wakeup` and under the same
    /// permission. Returns whether it was asleep.
    ///
    /// # Errors
    /// `ChannelError::Forbidden` if the `SecurityManager` refuses
    /// `WakeChannel`; an error if `actor` is not in this channel.
    pub async fn wakeup_actor(&self, context: &SecurityContext, actor: ActorId) -> Result<bool> {
        let channel = self.channel.load();
        self.check(context, ActionType::WakeChannel(&channel.security_attributes), "waking the agents of").await?;
        let actor = self.actors.get(&actor)
            .ok_or_else(|| anyhow!("actor {} is not in channel {}", actor, self.id))?;
        let was_asleep = actor.state() == ActorState::Sleeping;
        self.processing_state.store(ActorState::Active.into(), Ordering::SeqCst);
        actor.wakeup().await?;
        Ok(was_asleep)
    }

    /// Wakes the channel's actors because something is on its way to them:
    /// a message, a mention, a `WakeupRequested`. That is the consequence of
    /// an action already permitted, so it carries no permission of its own -
    /// otherwise a user allowed to write but not to wake could not write.
    pub async fn wake_for_delivery(&self) -> Result<()> {
        self.processing_state.store(ActorState::Active.into(), Ordering::SeqCst);
        for actor in self.actors.values() {
            actor.wakeup().await?;
        }
        Ok(())
    }

    /// Asks the `SecurityManager` about `action`, turning a refusal into a
    /// `ChannelError::Forbidden` that names what was refused, not why.
    async fn check(&self, context: &SecurityContext, action: ActionType<'_>, what: &'static str) -> Result<()> {
        if self.runtime.security_manager().check(context, action).await {
            return Ok(());
        }
        info!("Refused {} channel {} for {}", what, self.id, context.initiator_id);
        Err(ChannelError::Forbidden { action: what, channel: self.id }.into())
    }

    pub fn actor_names(&self) -> Vec<String> {
        self.actors.values().map(|a| a.name()).collect()
    }
    pub fn actor_ids(&self) -> Vec<ActorId> {
        self.actors.values().map(|a| a.id()).collect()
    }

    /// All participants of the channel, in no particular order.
    pub fn members(&self) -> Vec<ChannelMember> {
        self.actors.values()
            .map(|a| {
                let name = a.name();
                let handle = Some(handle_of(&name))
                    .filter(|h| Self::is_mentionable(a.id()) && !h.is_empty());
                ChannelMember { id: a.id(), name, handle, kind: a.kind() }
            })
            .collect()
    }

    fn is_mentionable(id: ActorId) -> bool {
        matches!(id, ActorId::User(_) | ActorId::Agent(_))
    }

    fn mention_directory(actors: &HashMap<ActorId, Arc<dyn Actor>>) -> MentionDirectory {
        let names: Vec<(ActorId, String)> = actors.values()
            .filter(|a| Self::is_mentionable(a.id()))
            .map(|a| (a.id(), a.name()))
            .collect();
        MentionDirectory::new(names.iter().map(|(id, name)| (*id, name.as_str())))
    }

    pub fn actor_by_id(&self, actor_id: ActorId) -> Option<Arc<dyn Actor>> {
        self.actors.values().find(|c| c.id() == actor_id).map(|c| c.clone())
    }
    pub fn actor_by_name(&self, name: &str) -> Option<Arc<dyn Actor>> {
        self.actors.values().find(|c| c.name() == name).map(|c| c.clone())
    }

    /// The actor `user` acts through in this channel, or `None` if they are
    /// not a participant of it.
    ///
    /// Every envelope is built by the actor itself, so this is what decides
    /// whose name a message or an approval carries: there is deliberately no
    /// way to reach "some" user actor (issue 30).
    pub fn user_actor(&self, user: UserId) -> Option<Arc<UserActor>> {
        let actor = self.actor_by_id(ActorId::User(user))?;
        let any_actor: Arc<dyn Any + Send + Sync> = actor;
        Arc::downcast::<UserActor>(any_actor).ok()
    }

    async fn create_actor(channel: Arc<ChannelController>, participant: &Participant, runtime: &ChannelRuntime) -> Result<Box<dyn Actor>> {
        let actor_runtime = runtime.new_actor_runtime();
        match participant {
            Participant::User(user) => {
                let actor = UserActor::new(&channel.load(), user.clone(), actor_runtime).await?;
                Ok(Box::new(actor))
            },
            Participant::Agent(agent) => {
                let actor = AgentActor::new(&channel.load(), agent.clone(), actor_runtime).await?;
                Ok(Box::new(actor))
            },
            Participant::Approver(approver) => {
                Ok(Box::new(ApproverActor::new(channel, approver.clone(), actor_runtime)))
            }
        }
    }

    pub async fn run(&self) -> Result<()> {
        let _guard = self.run_lock.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        info!("Channel {}/{} is starting", self.channel.name(), self.id);
        let mut global_rx = self.runtime.event_bus().subscribe_channel(self.id);
        let (event_tx, mut event_rx) = mpsc::channel(10);
        let mut join_set = JoinSet::new();
        let actors = Arc::new(self.spawn_actors(event_tx, &mut join_set)?);

        loop {
            tokio::select! {
                maybe_event = event_rx.recv() => {
                    if self.handle_actor_event(maybe_event, &actors).await {
                        break;
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global, &actors).await;
                }
            }
        }

        info!("Channel {}/{} stopped", self.channel.name(), self.id);
        Ok(())
    }

    async fn handle_actor_event(&self, maybe_event: Option<Envelope>, actors: &Arc<HashMap<ActorId,ActorHandle>>) -> bool {
        if let Some(event) = maybe_event {
            let span = crate::logging::envelope_span(&event.context, event.event.kind());
            self.process_envelope(event, actors).instrument(span).await;
            false
        } else {
            true
        }
    }

    async fn process_envelope(&self, event: Envelope, actors: &Arc<HashMap<ActorId,ActorHandle>>) {
        match event.event {
            // One per streamed chunk: far finer than per message.
            ActorEvent::PartialMessageGenerated(_) | ActorEvent::IntermediatePartialGenerated(_) => {
                trace!("Received {}", event.describe())
            }
            _ => debug!("Received {}", event.describe()),
        }
        // The full envelope carries message content: trace level only.
        trace!("Envelope: {:?}", event);
        let event = self.resolve_mentions(event);
        self.record_receive_event(&event).await;
        self.append_history(&event).await;
        self.route_event(actors, event).await;
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>, _actors: &Arc<HashMap<ActorId, ActorHandle>>) {
        if let Some(global_event) = maybe_global {
            self.channel.process_global_event(&global_event).await;
        }
    }

    fn spawn_actors(&self, actor_output:Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> Result<HashMap<ActorId,ActorHandle>> {
        let mut actors = HashMap::new();
        for (id, actor) in &self.actors {
            let handle = self.spawn_actor(actor.clone(), actor_output.clone(), join_set);
            actors.insert(*id, handle);
        };

        Ok(actors)
    }

    fn spawn_actor(&self, actor: Arc<dyn Actor>, actor_output: Sender<Envelope>, join_set: &mut JoinSet<ActorId>) -> ActorHandle {
        let id = actor.id();
        let filter = actor.event_filter();
        let (tx, rx) = mpsc::channel(100);

        let actor_rx = Box::new(InMemoryEventReceiver::new(rx));
        let actor_tx = Box::new(InMemoryEventSender::new(actor_output));

        let abort_handle = join_set.spawn(async move {
            let result = actor.run(actor_rx, actor_tx).await;
            if let Err(e) = result {
                tracing::error!("Actor {} stopped with an error: {:#}", actor.id(), e)
            }
            id
        });

        // Wrap the actor's input sender with its declared filter predicate.
        let filtered_sender = Box::new(FilteringEventSender::new(
            Box::new(InMemoryEventSender::new(tx)),
            filter,
        ));

        ActorHandle { id, abort_handle, sender: filtered_sender }
    }

    /// Resolves the `@`-mentions in every complete message carried by `event`
    /// and stores them in `Message::mentions`. Done here, once, as the message
    /// enters the channel, so that every frontend, the history and the
    /// moderator all see the same resolution (issue 29).
    fn resolve_mentions(&self, mut event: Envelope) -> Envelope {
        let resolve = |m: &mut Message| {
            if let Some(content) = &m.content {
                m.mentions = self.mentions.resolve(content);
            }
        };
        match &mut event.event {
            ActorEvent::CompleteMessageGenerated(m) | ActorEvent::IntermediateMessageGenerated(m) => resolve(m),
            ActorEvent::FullResponseGenerated(msgs) => msgs.iter_mut().for_each(resolve),
            _ => {}
        }
        event
    }

    /// Wakes every sleeping agent a user explicitly mentioned, so that the
    /// moderator's `ResponseRequested` reaches an agent that acts on it.
    /// Mentions in agent responses wake nobody, matching the moderator, which
    /// does not route them either.
    async fn wake_mentioned(&self, event: &Envelope) {
        if !matches!(event.context.sender_id, ActorId::User(_)) {
            return;
        }
        if let ActorEvent::FullResponseGenerated(messages) = &event.event {
            for id in messages.iter().flat_map(|m| m.mentions.iter()) {
                if let Some(actor) = self.actors.get(id) {
                    if matches!(id, ActorId::Agent(_)) && actor.state() == ActorState::Sleeping {
                        if let Err(e) = actor.wakeup().await {
                            warn!("Cannot wake mentioned agent {}: {}", id, e);
                        }
                    }
                }
            }
        }
    }

    async fn route_event(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) {
        if matches!(event.event, ActorEvent::WakeupRequested) {
            let _ = self.wake_for_delivery().await;
        }
        self.wake_mentioned(&event).await;

        //if let ActorEvent::FullResponseGenerated(messages) = &event.event {
        //    if messages.iter().any(|m| m.role == MessageRole::User) {
        //        let _ = self.wakeup().await;
        //    }
        //}

        let is_cancelling = ActorState::from(self.processing_state.load(Ordering::SeqCst)) == ActorState::Cancelling;
        let broadcast = if !is_cancelling {
            self.route_event_impl(actors, event.clone()).await
        } else {
            true // cancellation only suppresses internal routing, not external visibility
        };

        if broadcast {
            let _ = self.event_tx.send(event);
        }
    }

    async fn route_event_impl(&self, actors: &Arc<HashMap<ActorId,ActorHandle>>, event: Envelope) -> bool {
        if event.context.span_depth >= self.max_route_decisions {
            warn!("Too many routings, stopping further propagation");
            return true
        }

        let span_id = event.context.span_id;
        let span_depth = event.context.span_depth + 1;
        let mut moderator = self.moderator.lock().await;
        let decision = moderator.route(&event).await;
        drop(moderator);

        // First, route all messages to all agents...
        for route in decision.forwards {
            let target_id = route.0;

            // Do not propagate message to sender again
            if target_id == event.context.sender_id {
                continue;
            }

            let mut target_event = route.1;
            let sender_id = target_event.context.sender_id;
            let event_desc = target_event.describe();
            target_event.context.channel_id = self.id;
            target_event.context.session_id = self.session_id;
            target_event.context.span_id = span_id;
            target_event.context.span_depth = span_depth;
            if let Some(actor) = actors.get(&target_id) {
                debug!(to = %target_id, "Forwarding {}", event_desc);
                self.record_send_event(&target_event).await;
                if let Err(e) = actor.sender.try_send(target_event) {
                    match e {
                        TransportError::Full => warn!("Queue full for actor {}", target_id),
                        TransportError::Filtered => {}, // Normal: FilteringEventSender drops unmatched events
                        _ => warn!("Could not forward event {} from={} to={}: {:?}", event_desc, sender_id, target_id, e)
                    }
                }
            }
        }

        decision.broadcast
    }

    async fn append_history(&self, envelope: &Envelope) {
        let mut state = self.state.lock().await;
        let mut changed = false;
        match &envelope.event {
            ActorEvent::FullResponseGenerated(msgs) => {
                state.history.extend(msgs.clone());
                changed = true;
            }
            // Handle other event types as needed
            _ => {}
        }

        if changed {
            if let Err(e) = self.state_store.put_channel_state(self.id, &state).await {
                warn!("Cannot store state of channel {}: {}", self.id, e);
            }
        }
    }
    async fn record_send_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_send(&event).await;
        drop(recorder);
    }
    async fn record_receive_event(&self, event: &Envelope) {
        let mut recorder = self.recorder.lock().await;
        recorder.record_receive(&event).await;
        drop(recorder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandExecutor;
    use crate::component::ComponentImpl;
    use crate::component_impl::command::{HelpCommandProvider, WakeupCommandProvider};
    use caelumora_model::component::ComponentFailPolicy;
    use crate::runtime::RuntimeManager;
    use caelumora_model::command::default_commands;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{DummySecurityManager, SecurityAttributes, SecurityManager};
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::User;

    /// Refuses everything, to see that a refusal reaches the caller.
    #[derive(Debug)]
    struct DenyAll;

    #[async_trait::async_trait]
    impl SecurityManager for DenyAll {
        async fn check(&self, _context: &SecurityContext, _action: ActionType<'_>) -> bool {
            false
        }
    }

    /// Decides by a rule over the action alone, to refuse one thing and
    /// allow the rest.
    #[derive(Debug)]
    struct Policy(fn(ActionType<'_>) -> bool);

    #[async_trait::async_trait]
    impl SecurityManager for Policy {
        async fn check(&self, _context: &SecurityContext, action: ActionType<'_>) -> bool {
            (self.0)(action)
        }
    }

    fn user(username: &str) -> User {
        User {
            id: UserId::new(),
            username: username.to_string(),
            email: String::new(),
            roles: Vec::new(),
            security: SecurityContext::default(),
        }
    }

    async fn channel_with(participants: Vec<Participant>, security: Arc<dyn SecurityManager>) -> Arc<ChannelLoop> {
        let store = Arc::new(MemoryStore::new());
        let secrets = Arc::new(SecretManager::new(vec![], security.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, security, default_commands()).unwrap());
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            participants,
            SecurityAttributes::default(),
        );
        let builtin = |id: &str, facet: Arc<dyn crate::command::CommandProvider>| ComponentImpl { id: id.to_string(), fail_policy: ComponentFailPolicy::FailOpen, facet };
        let executor = CommandExecutor::new(
            runtime.security_manager(),
            vec![builtin("help", Arc::new(HelpCommandProvider)), builtin("wakeup", Arc::new(WakeupCommandProvider))],
        ).unwrap();
        ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap()
    }

    #[tokio::test]
    async fn every_user_has_their_own_actor_and_a_stranger_has_none() {
        let alice = user("alice");
        let bob = user("bob");
        let channel = channel_with(
            vec![Participant::User(alice.clone()), Participant::User(bob.clone())],
            Arc::new(DummySecurityManager::new()),
        ).await;

        for participant in [&alice, &bob] {
            let actor = channel.user_actor(participant.id).expect("a participant has an actor");
            assert_eq!(actor.id(), ActorId::User(participant.id));
            assert_eq!(actor.name(), participant.username, "the actor writes in this user's name");
        }
        assert!(channel.user_actor(UserId::new()).is_none(), "someone who is not a participant has no actor");
    }

    #[tokio::test]
    async fn cancelling_and_waking_deliberately_ask_the_security_manager() {
        let alice = user("alice");
        let context = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let participants = vec![Participant::User(alice)];

        let refused = channel_with(participants.clone(), Arc::new(DenyAll)).await;
        for error in [
            refused.cancel(&context).await.expect_err("cancelling is refused"),
            refused.wakeup(&context).await.expect_err("waking is refused"),
        ] {
            assert!(crate::error::is_forbidden(&error), "the caller can tell a refusal from a failure: {error}");
            assert!(!error.to_string().contains("DenyAll"), "a refusal does not disclose the rule: {error}");
        }
        // Waking because something is on its way is not a wakeup of its own:
        // it follows from an action that was permitted.
        refused.wake_for_delivery().await.expect("delivering does not ask");

        let allowed = channel_with(participants, Arc::new(DummySecurityManager::new())).await;
        allowed.cancel(&context).await.expect("permitted");
        allowed.wakeup(&context).await.expect("permitted");
    }

    fn command_error(error: &anyhow::Error) -> &crate::command::CommandError {
        error.downcast_ref().unwrap_or_else(|| panic!("expected a CommandError, got {error:#}"))
    }

    #[tokio::test]
    async fn help_answers_the_issuer_alone_and_lists_what_they_may_see() {
        let alice = user("alice");
        // `/wakeup` is hidden from everybody.
        let policy = Policy(|action| !matches!(action, ActionType::ShowCommand("wakeup")));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(policy)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let reply = actor.execute_command("help", "").await.expect("/help runs");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Private, "/help answers the issuer alone");
        let text = reply.output;
        assert!(text.contains("/help"), "{text}");
        assert!(!text.contains("/wakeup"), "a hidden command is not listed: {text}");
        let names: Vec<String> = actor.visible_commands().await.into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["help"]);

        let error = actor.execute_command("wakeup", "").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "a hidden command looks unknown: {error}");
        let error = actor.execute_command("help", "wakeup").await.expect_err("hidden");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "/help does not describe it either: {error}");

        let quiet = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await;
        assert!(quiet.is_err(), "a private answer is not sent into the channel: {quiet:?}");
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_is_announced_to_the_channel_in_the_issuers_name() {
        let alice = user("alice");
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let mut events = channel.subscribe();
        let running = tokio::spawn({ let channel = channel.clone(); async move { channel.run().await } });

        let reply = actor.execute_command("wakeup", "").await.expect("/wakeup runs");
        assert_eq!(reply.visibility, caelumora_model::command::CommandVisibility::Channel, "the answer goes to the channel");
        assert_eq!(reply.output, "No agent was asleep.", "and back to the issuer, whom the channel does not route it to");

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("the announcement arrives")
            .unwrap();
        assert_eq!(envelope.context.sender_id, ActorId::User(alice.id));
        match envelope.event {
            ActorEvent::CommandExecuted { command, output } => {
                assert_eq!(command, "wakeup");
                assert_eq!(output, "No agent was asleep.");
            }
            other => panic!("expected CommandExecuted, got {:?}", other.kind()),
        }
        assert!(channel.history().await.is_empty(), "commands never enter the history");
        running.abort();
    }

    #[tokio::test]
    async fn wakeup_reports_refusals_and_unknown_agents() {
        let alice = user("alice");
        let refuse_waking = Policy(|action| !matches!(action, ActionType::WakeChannel(_)));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(refuse_waking)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("waking is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "{error}");
        assert!(crate::error::is_forbidden(&error), "{error}");

        let refuse_running = Policy(|action| !matches!(action, ActionType::ExecuteCommand("wakeup")));
        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(refuse_running)).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "").await.expect_err("running it is refused");
        assert!(matches!(command_error(&error), crate::command::CommandError::Forbidden(_)), "visible but not runnable: {error}");

        let channel = channel_with(vec![Participant::User(alice.clone())], Arc::new(DummySecurityManager::new())).await;
        let actor = channel.user_actor(alice.id).unwrap();
        let error = actor.execute_command("wakeup", "@nobody").await.expect_err("no such agent");
        assert!(matches!(command_error(&error), crate::command::CommandError::InvalidArguments { .. }), "{error}");
        let error = actor.execute_command("", "").await.expect_err("empty");
        assert!(matches!(command_error(&error), crate::command::CommandError::Empty), "{error}");
        let error = actor.execute_command("nonsense", "").await.expect_err("unknown");
        assert!(matches!(command_error(&error), crate::command::CommandError::Unknown(_)), "{error}");
    }

    #[tokio::test]
    async fn only_the_owner_of_an_agent_may_read_what_it_saw() {
        use caelumora_model::agent::{Agent, Personality, PersonalityId};
        use caelumora_model::meta::{Metadata, Scope};
        use caelumora_model::security::OwnershipSecurityManager;
        use caelumora_model::store::EntityStore;

        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let personality = Personality {
            id: PersonalityId::new(),
            metadata: Metadata { name: "plain".to_string(), ..Metadata::default() },
            components: Default::default(),
            security_attributes: Default::default(),
        };
        store.put_personality(&SecurityContext::system(), &personality).await.unwrap();

        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |u: &User| SecurityContext::new(ActorId::User(u.id), Default::default(), Default::default());
        let agent = Agent::new("helper".to_string(), Scope::GlobalScope, &personality, as_user(&alice));
        let agent_id = ActorId::Agent(agent.id);

        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store, secrets, policy, default_commands()).unwrap());
        let channel = Channel::new(
            SessionId::new(),
            "main".to_string(),
            "broadcast".to_string(),
            vec![Participant::User(alice.clone()), Participant::User(bob.clone()), Participant::Agent(agent)],
            SecurityAttributes::default(),
        );
        let executor = CommandExecutor::empty(runtime.security_manager());
        let channel = ChannelLoop::new(channel, ChannelRuntime::new(runtime, Arc::new(executor))).await.unwrap();

        let helper = channel.actor_by_id(agent_id).expect("the agent is a participant");
        assert!(helper.may_read_context(&as_user(&alice)).await, "alice created the agent");
        assert!(!helper.may_read_context(&as_user(&bob)).await, "bob only takes part in the channel");
        let bobs_actor = channel.actor_by_id(ActorId::User(bob.id)).unwrap();
        assert!(!bobs_actor.may_read_context(&as_user(&alice)).await, "a user has nothing to read");
    }
}
