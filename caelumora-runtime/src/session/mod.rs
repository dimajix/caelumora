//! A running session: `SessionLoop`, how it is built and runs its channels.

use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::command::{CommandExecutor, ChannelCommandProvider};
use crate::runtime::{ChannelRuntime, RuntimeManager};
use crate::event::EventBus;
use anyhow::{anyhow, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::Channel;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::value::language::Language;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext, SecurityManager};
use crate::view::{Viewer, Views};
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::store::{EntityStore, StoreError, StoreResult, StoreResultExt};
use caelumora_model::scope::Scope;
use caelumora_model::session::{ChannelId, Session, SessionId};
use caelumora_model::group::GroupId;
use crate::group::purge_participants;
use caelumora_model::workspace::WorkspaceId;
use crate::controller::SessionController;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{AbortHandle, JoinSet};

mod channels;
mod manager;
mod settings;
#[cfg(test)]
pub(crate) mod testing;

pub use channels::ChannelSpec;
pub use manager::SessionManager;
pub(crate) use manager::Found;

/// A request for a channel that cannot be met as asked (issue 09). The
/// messages are meant for users (see `crate::error::user_message`).
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The channel's components do not make a moderator (issue 174): a kind
    /// that does not exist, two moderators, two policies. Found when the
    /// channel is built; a channel created so is taken back out.
    #[error("The channel's moderator cannot be built: {0}")]
    InvalidModerator(#[source] crate::component::ComponentError),
    #[error("{participant} is not a participant of session {session}")]
    NotAParticipant { session: SessionId, participant: ActorId },
    /// Only a user owns a session made from a template (issue 88).
    #[error("{0} cannot own a session: only a user can")]
    OwnerNotAUser(ActorId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    /// No classification that can be given (`require_classifiable`, issue
    /// 304): it names a compartment that is not configured.
    #[error("A channel cannot be classified {0}: a compartment is not configured or may not be given")]
    ClassificationRefused(Classification),
    /// The workspace lies in another session's scope, and is used within
    /// that session alone (issue 251, `Scope::admits`).
    #[error("The workspace {0} lies in another session's scope")]
    WorkspaceOutOfScope(caelumora_model::workspace::WorkspaceId),
}

/// Nothing if a channel may be given `classification` at all - it names
/// only configured compartments (issues 133, 304) - checked before anything
/// is stored.
fn require_classification(runtime_manager: &RuntimeManager, classification: &Classification) -> Result<()> {
    runtime_manager.confidentiality().require_classifiable(classification, || SessionError::ClassificationRefused(classification.clone()))?;
    Ok(())
}

/// The language the user `context` acts for prefers (issue 56), which
/// what they create is held in unless they say otherwise; none if they
/// prefer none or act for no user.
///
/// # Errors
/// Whatever reading their profile reports, except that there is none.
async fn preferred_language(store: &dyn EntityStore, context: &SecurityContext) -> StoreResult<Option<Language>> {
    let Some(user) = context.principal_id.as_user_id() else { return Ok(None) };
    let profile = store.get_user_profile(context, user).await.optional()?;
    Ok(profile.and_then(|profile| profile.preferred_language))
}

/// A channel of the session and, once `run` started it, its task.
#[derive(Debug)]
struct ChannelEntry {
    channel: Arc<ChannelLoop>,
    task: Option<AbortHandle>,
}

#[derive(Debug)]
pub struct SessionLoop {
    id: SessionId,
    session_controller: Arc<SessionController>,
    /// What builds its channels' runtimes and the views of it.
    runtime_manager: Arc<RuntimeManager>,
    /// The services it works with, taken from `runtime_manager` once (issue
    /// 229).
    security: Arc<dyn SecurityManager>,
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    entities: Arc<dyn EntityStore>,
    events: Arc<EventBus>,
    /// The user commands of every channel of the session.
    commands: Arc<CommandExecutor<dyn ChannelCommandProvider>>,
    /// The session's channels, and which of them run (issue 116). A channel
    /// deleted while it waits to be started has no entry any more, so `run`
    /// never starts it.
    channels: DashMap<ChannelId, ChannelEntry>,
    /// Channels to start. `run` starts them, those of the session as well as
    /// those created later, so that all of them run in its `JoinSet` - and
    /// stop with it when the session is stopped.
    start_tx: mpsc::UnboundedSender<ChannelId>,
    /// Taken by `run`, which is how it knows it is not already running.
    start_rx: Mutex<mpsc::UnboundedReceiver<ChannelId>>,
}

/// Removes the document of channel `channel_id` for good, in the runtime's
/// name, with the group of its participants: marked deleted first if it is
/// not - a channel of a session being purged, which is hidden with the
/// session, or one whose creation failed - since only what was deleted is
/// purged (issue 164).
async fn purge_channel_document(store: &dyn EntityStore, channel_id: ChannelId) -> StoreResult<bool> {
    let system = SecurityContext::system();
    if let Err(e) = purge_participants(store, GroupId::of_channel(channel_id)).await {
        if !matches!(e, StoreError::NotFound { .. }) {
            return Err(e);
        }
    }
    store.delete_channel(&system, channel_id).await?;
    store.purge_channel(&system, channel_id).await
}

/// Asks whether `context` may attach the workspace `id` to the session
/// `session_id` (issue 253): it must be theirs to share (`GetWorkspace`,
/// `UpdateWorkspace` - its owner, issue 251, decision 12) and lie where the
/// session may use it, not in another session's scope. Whether they may
/// change the session is the caller's to ask - or not, for a session they
/// are making (issue 266).
///
/// # Errors
/// `StoreError::NotFound` if there is no workspace they may read,
/// `StoreError::PermissionDenied` if they may not share it,
/// `SessionError::WorkspaceOutOfScope`.
async fn require_attachable(security: &dyn SecurityManager, entities: &dyn EntityStore, context: &SecurityContext, id: WorkspaceId, session_id: SessionId) -> Result<()> {
    let shared = entities.get_workspace(context, id).await?;
    security.require(context, ActionType::UpdateWorkspace(&shared.security_attributes), || StoreError::PermissionDenied).await?;
    if !shared.name.scope.admits(&Scope::SessionScope { session_id }) {
        return Err(SessionError::WorkspaceOutOfScope(id).into());
    }
    Ok(())
}

impl SessionLoop {
    /// The loop of `session`, with its `channels` - documents of their own
    /// (issue 70), in the order the session lists them - each attached to
    /// it, so that a channel reaches the others of its session (issues 17,
    /// 123).
    pub async fn new(session: Session, channels: Vec<Channel>, runtime_manager: Arc<RuntimeManager>) -> Result<Arc<Self>> {
        let id = session.id;
        let commands = Arc::new(runtime_manager.new_command_executor(&session).await?);
        let session_controller = runtime_manager.controller_manager().new_session_controller(session);
        let (start_tx, start_rx) = mpsc::unbounded_channel();
        let channel_docs = channels;
        let channels = DashMap::new();
        for channel in channel_docs {
            let runtime = ChannelRuntime::new(Arc::clone(&runtime_manager), Arc::clone(&commands), Arc::clone(&session_controller));
            let cl = ChannelLoop::new(channel, runtime).await?;
            let id = cl.id();
            channels.insert(id, ChannelEntry { channel: cl, task: None });
            let _ = start_tx.send(id);
        }

        let session_loop = Arc::new(Self {
            id,
            session_controller,
            security: Arc::clone(runtime_manager.security_manager()),
            confidentiality: Arc::clone(runtime_manager.confidentiality()),
            entities: Arc::clone(runtime_manager.entity_store()),
            events: Arc::clone(runtime_manager.event_bus()),
            runtime_manager,
            commands,
            channels,
            start_tx,
            start_rx: Mutex::new(start_rx),
        });
        for entry in session_loop.channels.iter() {
            entry.channel.attach(&session_loop);
        }
        Ok(session_loop)
    }

    /// The views of an agent in this session, as `viewer` (issue 163):
    /// this session alone, reached through its loop.
    pub(crate) fn views(self: &Arc<Self>, viewer: Viewer) -> Views {
        Views::of_own_session(&self.runtime_manager, Arc::clone(self), viewer)
    }

    pub fn id(&self) -> SessionId {
        self.session_controller.id()
    }

    /// The session as it is now.
    pub fn session(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session_controller.load()
    }

    /// The session as privacy sees it now (issue 134).
    pub fn privacy(&self) -> SessionPrivacy {
        SessionPrivacy::of(&self.session())
    }

    pub fn name(&self) -> Name {
        self.session_controller.name()
    }

    /// Runs the session's channels, those it has and those created while it
    /// runs, until the session is stopped - which aborts this, and with it
    /// every channel.
    pub async fn run(&self) -> Result<()> {
        let mut start_rx = self.start_rx.try_lock()
            .map_err(|_| anyhow!("Already running"))?;

        let mut join_set = JoinSet::new();
        // The session's own events; those about its users and agents are the
        // `ControllerManager`'s to take in (issue 121).
        let mut global_rx = self.events.subscribe_session(self.id());

        loop {
            tokio::select! {
                Some(channel_id) = start_rx.recv() => {
                    self.start_channel(channel_id, &mut join_set);
                }
                Some(res) = join_set.join_next(), if !join_set.is_empty() => {
                    match res {
                        Ok(channel_id) => {
                            let attributes = self.channels.get_mut(&channel_id)
                                .map(|mut entry| {
                                    entry.task = None;
                                    entry.channel.security_attributes()
                                });
                            // Gone: a deleted channel, whose actors all
                            // stopped in time; `delete_channel` announces it.
                            if let Some(attributes) = attributes {
                                self.events.publish(GlobalEvent::ChannelStopped(self.id, channel_id), attributes);
                            }
                        }
                        // A deleted channel; `delete_channel` announces it.
                        Err(e) if e.is_cancelled() => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                maybe_global = global_rx.recv() => {
                    self.handle_global_event(maybe_global).await;
                }
            }
        }
    }

    async fn handle_global_event(&self, maybe_global: Option<GlobalEvent>) {
        if let Some(global_event) = maybe_global {
            self.session_controller.process_global_event(&global_event);
        }
    }

    /// Starts the channel `id` in `join_set` - unless it was deleted while
    /// it waited to be started. The entry stays locked while its task is
    /// set, so that `delete_channel` either finds the task or no channel.
    fn start_channel(&self, id: ChannelId, join_set: &mut JoinSet<ChannelId>) {
        let Some(mut entry) = self.channels.get_mut(&id) else { return };
        let channel = Arc::clone(&entry.channel);
        self.events.publish(GlobalEvent::ChannelStarted(self.id, id), channel.security_attributes());
        entry.task = Some(join_set.spawn(async move {
            let _ = channel.run().await;
            channel.id()
        }));
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::value::name::Name;
    use crate::channel::ChannelLoop;
    use crate::runtime::{ChannelRuntime, RuntimeManager};
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::{SecurityAttributes, SecurityContext};
    use crate::view::Viewer;
    use caelumora_model::session::SessionId;
    use std::sync::Arc;
    use caelumora_model::value::name::ScopedName;
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::DummySecurityManager;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::actor::Rejection;
    use crate::session::{ChannelSpec, SessionError, SessionManager};
    use crate::session::testing::*;

    #[tokio::test]
    async fn a_renamed_user_is_renamed_in_every_channel_of_the_session() {
        use crate::actor::Actor;
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserUpdateEvent;

        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let ActorId::User(owner) = ctx.initiator_id else { unreachable!() };
        let first = session.create_channel(&ctx, spec("first", vec![other])).await.unwrap();
        let second = session.create_channel(&ctx, spec("second", vec![other])).await.unwrap();

        // Announced about the user, as whoever changes users will: the
        // session takes it into the one controller both actors share.
        let renamed = async {
            loop {
                runtime.event_bus().publish(GlobalEvent::UserUpdated(owner, UserUpdateEvent::NameChanged("renamed".to_string())), Default::default());
                let names: Vec<String> = [&first, &second].iter().map(|c| c.user_actor(owner).expect("takes part").name().to_string()).collect();
                if names.iter().all(|n| n == "renamed") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), renamed).await.expect("both channels show the new name");
        assert_eq!(first.user_actor(other.as_user_id().unwrap()).unwrap().name(), "other", "nobody else is renamed");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_reads_another_by_their_classification_not_by_who_takes_part() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![])]).unwrap();
        let (policy, confidentiality, store) = crate::session::testing::owned_store(ConfidentialityRules::new(compartments));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::clone(&confidentiality), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));

        let user = |name: &str| User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, bob) = (user("alice"), user("bob"));
        let alice_id = ActorId::User(alice.id);
        let bob_id = ActorId::User(bob.id);
        // Her context carries her roles, as her controller gives it (issue 304).
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default()).with_roles(alice.roles.clone(), None);
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes::owned_by(alice_id).granting(caelumora_model::security::Subject::Actor(bob_id), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification, participants| ChannelSpec { classification: Some(classification), ..spec(name, participants) };
        let public = session.create_channel(&alice_ctx, with("public", classified(Confidentiality::Public, &[]), vec![bob_id])).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]), vec![bob_id])).await.unwrap();
        let secret = session.create_channel(&alice_ctx, with("secret", classified(Confidentiality::Confidential, &[]), vec![])).await.unwrap();
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]), vec![])).await.unwrap();

        let confidentiality = runtime.confidentiality();
        let may_read = |reader: &Arc<ChannelLoop>, source: &Arc<ChannelLoop>| {
            let (reader, source, confidentiality, context) = (Arc::clone(reader), Arc::clone(source), Arc::clone(confidentiality), alice_ctx.clone());
            async move {
                use caelumora_model::confidentiality::{ConfidentialityQuestions, Place};
                let place = |channel: &ChannelLoop| Place { channel: channel.id(), classification: channel.classification(), session: channel.session_loop().map(|s| s.privacy()).unwrap_or_default() };
                confidentiality.may_copy_information(&context, &place(&source), &place(&reader))
            }
        };
        assert!(may_read(&secret, &secret).await, "a channel reads itself");
        assert!(may_read(&team, &public).await);
        assert!(!may_read(&public, &team).await, "a public channel reads only what is public");
        assert!(!may_read(&team, &secret).await, "confidential is not read from internal");
        assert!(may_read(&secret, &team).await, "although bob, who is in team, is not in secret");
        assert!(may_read(&hr, &team).await, "what is general flows into HR");
        assert!(!may_read(&secret, &hr).await, "what is HR's never into a general channel");

        let refused = session.create_channel(&alice_ctx, with("typo", classified(Confidentiality::Internal, &["hrr"]), vec![])).await.expect_err("not configured");
        assert!(matches!(refused.downcast_ref::<SessionError>(), Some(SessionError::ClassificationRefused(_))), "{refused}");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_reader_knows_the_privacy_of_its_session_and_looks_up_others() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::privacy::PrivacyConfig;
        use caelumora_model::user::{User, UserId};

        let (policy, confidentiality, store) = crate::session::testing::owned_store(Default::default());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::clone(&confidentiality), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = SessionManager::new(Arc::clone(&runtime));
        let user = |name: &str| User { id: UserId::new(), username: Name::new(name).unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let (alice, carol) = (user("alice"), user("carol"));
        let (alice_id, carol_id) = (ActorId::User(alice.id), ActorId::User(carol.id));
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());

        let mut hers = make_session("hers");
        hers.security_attributes = SecurityAttributes::owned_by(alice_id);
        hers.participants = record_users(&manager, &[&alice]).await;
        hers.privacy = PrivacyConfig::SameOwner;
        let hers = create(&manager, &alice_ctx, hers).await.unwrap();
        let mut theirs = make_session("theirs");
        theirs.security_attributes = SecurityAttributes::owned_by(carol_id);
        persist(&runtime, &SecurityContext::new(carol_id, Default::default(), Default::default()), &theirs).await;

        let channel = Arc::clone(&hers.channels()[0]);
        let channel_runtime = ChannelRuntime::new(Arc::clone(&runtime), Arc::clone(&hers.commands), Arc::clone(&hers.session_controller));
        channel_runtime.attach(&channel);
        let views = channel_runtime.new_actor_runtime(alice_id, hers.id(), channel.id()).views();
        let session = views.session(&alice_ctx, channel.id(), channel.classification()).await.expect("attached to its session");
        let stranger = SecurityContext::new(ActorId::Agent(caelumora_model::agent::AgentId::new()), Default::default(), Default::default());
        assert!(views.session(&stranger, channel.id(), channel.classification()).await.is_none(), "reached by who may read it alone (issue 304)");

        let own = session.privacy();
        assert_eq!((own.policy, own.owner), (PrivacyConfig::SameOwner, alice_id));
        assert_eq!(own.people, [alice.id].into_iter().collect(), "its users and its owner");
        let other = session.views().privacy_of(theirs.id).await.unwrap().expect("stored, running or not");
        assert_eq!((other.policy, other.owner), (PrivacyConfig::Private, carol_id), "even a session the reader may not read");
        assert!(session.views().privacy_of(SessionId::new()).await.unwrap().is_none(), "one that is gone");
        manager.stop_session(&SecurityContext::system(), hers.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_released_message_is_read_where_its_channel_is_not() {
        use crate::view::ViewError;
        use caelumora_model::actor::ActorId;
        use caelumora_model::confidentiality::{Compartment, Compartments, ConfidentialityRules, Confidentiality};
        use caelumora_model::user::{User, UserId};

        let compartments = Compartments::new([(Compartment::new("hr"), vec![]), (Compartment::new("finance"), vec![])]).unwrap();
        let (policy, confidentiality, store) = crate::session::testing::owned_store(ConfidentialityRules::new(compartments));
        let secrets = Arc::new(SecretManager::new(vec![], Arc::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::clone(&confidentiality), &caelumora_model::config::AppConfig::default(), crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store))).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let alice = User { id: UserId::new(), username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let alice_id = ActorId::User(alice.id);
        let alice_ctx = SecurityContext::new(alice_id, Default::default(), Default::default());
        let mut session = make_session("hers");
        session.security_attributes = SecurityAttributes::owned_by(alice_id);
        session.participants = record_users(&manager, &[&alice]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();

        let classified = |level, compartments: &[&str]| Classification::new(level, compartments.iter().map(|c| Compartment::new(*c))).unwrap();
        let with = |name: &str, classification| ChannelSpec { classification: Some(classification), ..spec(name, vec![]) };
        let hr = session.create_channel(&alice_ctx, with("hr", classified(Confidentiality::Internal, &["hr"]))).await.unwrap();
        let finance = session.create_channel(&alice_ctx, with("finance", classified(Confidentiality::Internal, &["finance"]))).await.unwrap();
        let team = session.create_channel(&alice_ctx, with("team", classified(Confidentiality::Internal, &[]))).await.unwrap();

        let writer = hr.user_actor(alice.id).expect("the creator takes part");
        let mut events = writer.watch().events;
        writer.send_message("for HR".to_string(), None).await.unwrap();
        writer.send_message("for finance too".to_string(), Some(classified(Confidentiality::Internal, &["hr", "finance"]))).await.unwrap();
        writer.send_message("too secret".to_string(), Some(classified(Confidentiality::Confidential, &["hr"]))).await.expect("sent: the channel decides");
        assert_eq!(rejection(&mut events).await.1, Rejection::ClassificationRefused, "above the channel, and its writer is told (issue 304)");
        let history = history_of(&hr, 2).await;
        assert_eq!(history[0].classification, classified(Confidentiality::Internal, &["hr"]), "the channel's, unless classified otherwise");

        // Read as an agent in another channel reads: through its turn's views.
        let reading_from = |channel: &Arc<ChannelLoop>| {
            let channel_runtime = ChannelRuntime::new(Arc::clone(&runtime), Arc::clone(&session.commands), Arc::clone(&session.session_controller));
            channel_runtime.attach(channel);
            let views = channel_runtime.new_actor_runtime(alice_id, session.id(), channel.id()).views();
            let (context, channel_id, classification) = (alice_ctx.clone(), channel.id(), channel.classification());
            async move { views.session(&context, channel_id, classification).await.expect("attached to its session") }
        };
        let read = reading_from(&finance).await.channel_named("hr").await.expect("reachable for what was released").history().await.expect("what was released to finance");
        let texts: Vec<_> = read.messages().iter().map(|m| m.content.clone().unwrap_or_default()).collect();
        assert_eq!(texts, ["for finance too"], "only what was released");
        let nothing = reading_from(&team).await.channel_named("hr").await.expect("reachable, but").history().await;
        assert!(matches!(nothing, Err(ViewError::NoChannelNamed(_))), "nothing was released to a general channel");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// A model whose answer never comes.
    #[derive(Debug)]
    struct Silent;

    #[async_trait::async_trait]
    impl crate::llm::LLMClient for Silent {
        async fn chat_completion(&self, _: &[caelumora_model::message::Message], _: &[caelumora_model::tool::Tool])
            -> futures::stream::BoxStream<'static, crate::llm::LLMResult<caelumora_model::message::MessageEvent>>
        {
            Box::pin(futures::stream::pending())
        }

        fn profile(&self) -> caelumora_model::context::ModelProfile {
            Default::default()
        }
    }

    #[tokio::test]
    async fn a_member_s_answer_under_way_is_in_the_snapshot_of_its_channel() {
        use caelumora_model::actor::{ActorEvent, ActorId};
        use caelumora_model::agent::{Agent, Personality, PersonalityId, PersonalityKind};
        use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
        use caelumora_model::scope::Scope;
        use caelumora_model::snapshot::Detail;
        use caelumora_model::store::EntityStore;
        use caelumora_model::user::{User, UserId};

        let store = Arc::new(MemoryStore::new());
        let mut registry = crate::component::ComponentRegistry::built_in(Arc::<MemoryStore>::clone(&store));
        registry.register(crate::component::ComponentKind::new("silent").provides::<dyn crate::llm::LLMClient>(), |_descriptor, _context| {
            let mut facets = crate::component::ComponentFacets::empty();
            facets.insert::<dyn crate::llm::LLMClient>(Arc::new(Silent));
            Ok(facets)
        });
        let policy = Arc::new(DummySecurityManager::new());
        let secrets = Arc::new(SecretManager::new(vec![], Arc::<DummySecurityManager>::clone(&policy)));
        let runtime = Arc::new(RuntimeManager::new(Arc::<MemoryStore>::clone(&store), Arc::<MemoryStore>::clone(&store), secrets, policy, Arc::new(caelumora_model::confidentiality::ConfidentialityRules::default()), &caelumora_model::config::AppConfig::default(), registry).unwrap());
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
        let system = SecurityContext::system();

        let alice = User { id: UserId::new(), username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None };
        let participants = record_users(&manager, &[&alice]).await;
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("thinker", Default::default()).unwrap(), kind: PersonalityKind::Internal,
            roles: None, description: None, introduction: None,
            components: ComponentConfig { component_nodes: vec![ComponentDescriptor::new("model", "silent")] },
            security_attributes: Default::default(), reasoning_visibility: Default::default(),
            deleted: None,
        };
        store.create_personality(&system, &personality).await.unwrap();
        let thinker = Agent::new(ScopedName::new("thinker", Scope::GlobalScope).unwrap(), &personality, ActorId::User(alice.id));
        store.create_agent(&system, &thinker).await.unwrap();
        let agent = ActorId::Agent(thinker.id);

        let alice_ctx = SecurityContext::new(ActorId::User(alice.id), Default::default(), Default::default());
        let mut session = make_session("thinking");
        session.participants = participants;
        session.participants.push(caelumora_model::session::Participant::Agent(thinker.id));
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let channel = session.create_channel(&alice_ctx, spec("talk", vec![ActorId::User(alice.id), agent])).await.unwrap();
        let mut events = channel.user_actor(alice.id).unwrap().watch().events;
        let shown = |events: &mut tokio::sync::broadcast::Receiver<caelumora_model::actor::Envelope>, wanted: &'static str| {
            let mut events = events.resubscribe();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let envelope = events.recv().await.expect("the channel is running");
                        if envelope.context.sender_id == agent && envelope.event.kind() == wanted { break envelope.event }
                    }
                }).await.unwrap_or_else(|_| panic!("the agent sends {wanted}"))
            }
        };
        let member = || async {
            let view = manager.views(Viewer::new(alice_ctx.clone())).session(session.id()).await.unwrap().channel(channel.id()).await.unwrap();
            view.snapshot(Detail::MEMBERS).await.members.into_iter().find(|m| m.id == agent).expect("a member")
        };
        assert_eq!(member().await.turn_under_way, None, "nothing under way yet");

        let started = shown(&mut events, "ResponseStarted");
        channel.user_actor(alice.id).unwrap().send_message("hello".to_string(), None).await.unwrap();
        let ActorEvent::ResponseStarted { turn_id } = started.await else { unreachable!() };
        assert_eq!(member().await.turn_under_way, Some(turn_id), "the turn its start announced");

        let ended = shown(&mut events, "TurnFinished");
        channel.cancel_actor(&alice_ctx, agent).await.unwrap();
        ended.await;
        assert_eq!(member().await.turn_under_way, None, "over once it ended");
        manager.stop_session(&system, session.id()).await.unwrap();
    }
}
