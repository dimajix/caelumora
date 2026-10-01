//! Views: the sessions, their channels and their members, as one viewer
//! navigates them (issue 163). A view is a node the viewer stands on; its
//! methods are the edges to the next ones, and each edge asks its own
//! access question, in the viewer's name. A node is data the viewer was
//! allowed to reach; it decides nothing - an operation still checks its own
//! permission when it acts (issue 80).
//!
//! What a node shows is what is stored - the documents from the
//! `EntityStore`, the agents' states from the `StateStore` - reached
//! through the running loop where there is one, which holds the truth in
//! memory and writes the store after. So one
//! navigation serves running and stopped sessions alike, and reaching a
//! session does not start it. The root is [`SessionManager::views`].
//!
//! A node answers with a snapshot (`caelumora_model::snapshot`, issue 80)
//! for a client that lists and offers: the node's data with its questions
//! answered at one moment.

mod channel;
mod context;
mod session;

pub use channel::{ChannelView, MemberView};
pub use context::{AgentContextView, ContextError};
pub use session::SessionView;

use crate::runtime::RuntimeManager;
use crate::session::{Found, SessionManager};
use caelumora_model::actor::ActorId;
use caelumora_model::channel::Participant;
use caelumora_model::security::{SecurityAttributes, SecurityContext};
use caelumora_model::session::{ChannelId, SessionId};
use std::sync::Arc;

/// Who navigates: in whose name every edge asks its question (issue 131).
#[derive(Debug, Clone)]
pub struct Viewer {
    context: SecurityContext,
}

impl Viewer {
    pub fn new(context: SecurityContext) -> Self {
        Self { context }
    }

    pub fn context(&self) -> &SecurityContext {
        &self.context
    }
}

/// Why an edge could not be followed. The messages are meant for users
/// (see `crate::error::user_message`).
///
/// What does not exist and what the viewer may not read are one case each,
/// so that nothing can be probed - as `SessionError::ChannelNotFound` holds
/// for operations.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("There is no session {0}")]
    SessionNotFound(SessionId),
    #[error("Session {session} has no channel {channel}")]
    ChannelNotFound { session: SessionId, channel: ChannelId },
    #[error("Channel {channel} has no member {actor}")]
    MemberNotFound { channel: ChannelId, actor: ActorId },
    /// Reached, but not the viewer's to read: what an agent saw.
    #[error("You are not permitted to see what {actor} saw")]
    Forbidden { actor: ActorId },
    /// Only agents have a context.
    #[error("There is nothing {actor} saw: it is no agent, or no longer exists")]
    NoContext { actor: ActorId },
    /// Reaching it failed; the cause is in the log.
    #[error("Reading failed")]
    Failed(#[source] anyhow::Error),
}

pub type ViewResult<T> = std::result::Result<T, ViewError>;

/// The root of the navigation: every session the viewer may read.
#[derive(Clone)]
pub struct Views {
    manager: Arc<SessionManager>,
    viewer: Viewer,
}

impl Views {
    pub(crate) fn new(manager: Arc<SessionManager>, viewer: Viewer) -> Self {
        Self { manager, viewer }
    }

    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    pub(crate) fn runtime(&self) -> &RuntimeManager {
        self.manager.runtime_manager()
    }

    /// Every session the viewer may read, in the order the store lists
    /// them.
    ///
    /// # Errors
    /// `Failed` if the store cannot list them.
    pub async fn sessions(&self) -> ViewResult<Vec<SessionView>> {
        let sessions = self.runtime().entity_store().list_sessions(self.viewer.context()).await
            .map_err(|e| ViewError::Failed(anyhow::Error::new(e).context("listing the sessions")))?;
        // The store lists what the viewer may read; a running session is
        // reached through its loop.
        Ok(sessions.into_iter()
            .map(|session| match self.manager.running(session.id) {
                Some(session_loop) => Found::Running(session_loop),
                None => Found::Stored(session),
            })
            .map(|found| SessionView::of(self.clone(), found))
            .collect())
    }

    /// Session `id`, if the viewer may read it (`GetSession`).
    ///
    /// # Errors
    /// `SessionNotFound` if there is no such session or the viewer may not
    /// read it.
    pub async fn session(&self, id: SessionId) -> ViewResult<SessionView> {
        let found = self.manager.find(self.viewer.context(), id).await.map_err(|e| {
            if crate::error::is_not_found(&e, "session") { ViewError::SessionNotFound(id) } else { ViewError::Failed(e) }
        })?;
        Ok(SessionView::of(self.clone(), found))
    }

    /// The name of `participant` and, for an agent, its attributes; `None`
    /// for a user or agent that no longer exists. Read in the runtime's
    /// name: taking part in a session makes a name known to everybody in
    /// it - it stands at every message - even where the record itself is
    /// not theirs to read (issue 120).
    async fn look_up(&self, participant: &Participant) -> (Option<String>, Option<SecurityAttributes>) {
        let store = self.runtime().entity_store();
        let system = SecurityContext::system();
        match participant {
            Participant::User(id) => (store.get_user(&system, *id).await.ok().map(|user| user.username), None),
            Participant::Agent(id) => match store.get_agent(&system, *id).await {
                Ok(agent) => (Some(agent.metadata.name), Some(agent.security_attributes)),
                Err(_) => (None, None),
            },
            Participant::Approver(_) => (Some("Approver".to_string()), None),
        }
    }

    /// The roles of the viewer's user, which decide what they are cleared
    /// for (issue 133). Without them, only what is public: a failed lookup
    /// shows less.
    async fn roles(&self) -> Vec<String> {
        self.runtime().roles_of(self.viewer.context().initiator_id).await.unwrap_or_default()
    }
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Views").field("viewer", &self.viewer).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeManager;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::channel::Channel;
    use caelumora_model::command::default_commands;
    use caelumora_model::confidentiality::{Classification, Visibility};
    use caelumora_model::meta::{Metadata, Scope};
    use caelumora_model::secret::SecretManager;
    use caelumora_model::security::{OwnershipSecurityManager, SecurityManager};
    use caelumora_model::session::Session;
    use caelumora_model::store::EntityStore;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::user::{User, UserId};

    /// A runtime under the ownership policy, and a manager on it.
    fn owned_runtime() -> (Arc<MemoryStore>, Arc<SessionManager>) {
        let policy: Arc<dyn SecurityManager> = Arc::new(OwnershipSecurityManager::new());
        let store = Arc::new(MemoryStore::with_security_manager(policy.clone()));
        let secrets = Arc::new(SecretManager::new(vec![], policy.clone()));
        let runtime = Arc::new(RuntimeManager::new(store.clone(), store.clone(), secrets, policy, default_commands(), Default::default(), crate::component::ComponentRegistry::built_in(store.clone())).unwrap());
        (store, Arc::new(SessionManager::new(runtime)))
    }

    fn user(name: &str) -> User {
        User { id: UserId::new(), username: name.to_string(), email: String::new(), roles: Vec::new() }
    }

    fn as_user(id: UserId) -> Viewer {
        Viewer::new(SecurityContext::new(ActorId::User(id), Default::default(), Default::default()))
    }

    async fn record_users(store: &MemoryStore, users: &[&User]) -> Vec<Participant> {
        let mut participants = Vec::new();
        for user in users {
            store.create_user(&SecurityContext::system(), user).await.unwrap();
            participants.push(Participant::User(user.id));
        }
        participants
    }

    fn make_session(name: &str, owner: ActorId, contributors: Vec<ActorId>) -> Session {
        Session {
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId::new(),
            metadata: Metadata { name: name.to_string(), scope: Scope::GlobalScope, ..Metadata::default() },
            participants: Vec::new(),
            channels: vec![ChannelId::new()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            security_attributes: SecurityAttributes { owner_id: owner, contributors },
        }
    }

    fn main_channel(session: &Session) -> Channel {
        Channel {
            id: session.channels[0],
            session_id: session.id,
            name: "main".to_string(),
            moderator: "broadcast".to_string(),
            classification: Classification::CHANNEL_DEFAULT,
            history_limits: Vec::new(),
            participants: session.participants.clone(),
            security_attributes: session.security_attributes.clone(),
        }
    }

    async fn an_agent_of(store: &MemoryStore, owner: ActorId) -> Agent {
        let personality = Personality {
            reasoning_visibility: Visibility::Channel,
            id: PersonalityId::new(),
            metadata: Metadata { name: "plain".to_string(), ..Metadata::default() },
            components: Default::default(),
            security_attributes: Default::default(),
        };
        store.create_personality(&SecurityContext::system(), &personality).await.unwrap();
        let agent = Agent::new("Helper".to_string(), Scope::GlobalScope, &personality, owner);
        store.create_agent(&SecurityContext::system(), &agent).await.unwrap();
        agent
    }

    #[tokio::test]
    async fn the_views_show_what_their_viewer_may_read_and_do_whether_the_session_runs_or_not() {
        let (store, manager) = owned_runtime();
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;

        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::User(bob.id), ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let main = main_channel(&session);
        let running = manager.create_session(as_user(alice.id).context(), session, vec![main]).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = running.create_channel(as_user(alice.id).context(), crate::session::ChannelSpec { name: "private".to_string(), moderator: None, classification: None, participants: vec![] }).await.unwrap().id();
        let id = running.id();

        let check = |state: &'static str| {
            let manager = &manager;
            async move {
                let names = |channels: &[ChannelView]| channels.iter().map(|c| c.name().to_string()).collect::<Vec<_>>();
                let for_alice = manager.views(as_user(alice.id)).session(id).await.unwrap();
                assert_eq!(names(&for_alice.channels().await), ["main", "private"], "{state}: the owner reads every channel");
                assert_eq!(for_alice.snapshot().await.participants.iter().filter_map(|p| p.name.as_deref()).collect::<Vec<_>>(), ["alice", "bob", "Helper"], "{state}");
                let for_bob = manager.views(as_user(bob.id)).session(id).await.unwrap();
                assert_eq!(names(&for_bob.channels().await), ["main"], "{state}: bob is not in the private channel");
                assert!(for_alice.may_change().await && !for_bob.may_change().await, "{state}: the owner changes the session, a participant does not");

                let main = for_alice.channel(for_alice.channels().await[0].id()).await.unwrap();
                assert!(main.may_change().await, "{state}: the owner changes the channel");
                let agent = main.member(ActorId::Agent(helper.id)).await.expect("the agent is a member");
                assert_eq!(agent.handle().as_deref(), Some("helper"), "{state}");
                assert!(agent.may_read_context().await, "{state}: alice made the agent");
                let main = &for_bob.channels().await[0];
                assert!(!main.may_change().await, "{state}: a participant does not change it");
                for member in main.members().await {
                    assert!(!member.may_read_context().await, "{state}: bob reads no agent's context");
                }

                let hidden = for_bob.channel(hers).await.expect_err("not bob's to read");
                assert!(matches!(hidden, ViewError::ChannelNotFound { .. }), "{state}: absent, not refused: {hidden}");
                assert_eq!(for_alice.channel(hers).await.unwrap().name(), "private", "{state}");
                let refused = manager.views(as_user(carol.id)).session(id).await.expect_err("carol may not see it");
                assert!(matches!(refused, ViewError::SessionNotFound(_)), "{state}: {refused}");
                assert!(manager.views(as_user(carol.id)).sessions().await.unwrap().is_empty(), "{state}: nor is it listed for her");
                let listed = manager.views(as_user(bob.id)).sessions().await.unwrap();
                assert_eq!(listed.len(), 1, "{state}");
                assert_eq!(listed[0].snapshot().await, for_bob.snapshot().await, "{state}: listed as it is reached");
            }
        };
        check("running").await;
        assert!(manager.stop_session(&SecurityContext::system(), id).await.unwrap(), "it ran");
        check("stopped").await;
        assert!(manager.running(id).is_none(), "reaching a session does not start it");
    }

    #[tokio::test]
    async fn a_channel_is_described_with_handles_only_for_who_can_be_mentioned() {
        use caelumora_model::approval::{ApprovalStrategy, Approver, ApproverId};

        let (store, manager) = owned_runtime();
        let alice = User { id: UserId::new(), username: "Alice Example".to_string(), email: String::new(), roles: Vec::new() };
        let gone = UserId::new();
        let approver = Approver { id: ApproverId(uuid::Uuid::new_v4()), strategy: ApprovalStrategy::Any };
        let mut session = make_session("s", ActorId::User(alice.id), vec![]);
        session.participants = record_users(&store, &[&alice]).await;
        let mut channel = main_channel(&session);
        channel.participants.extend([Participant::User(gone), Participant::Approver(approver)]);
        store.create_session(&SecurityContext::system(), &session).await.unwrap();
        store.create_channel(&SecurityContext::system(), &channel).await.unwrap();

        let view = manager.views(as_user(alice.id)).session(session.id).await.unwrap().channel(channel.id).await.unwrap().snapshot().await;

        let [alice, gone, approver] = view.members.as_slice() else { panic!("three members, in order: {view:?}") };
        assert_eq!((alice.name.as_deref(), alice.handle.as_deref()), (Some("Alice Example"), Some("alice-example")));
        assert_eq!((gone.name.as_deref(), gone.handle.as_deref()), (None, None), "a deleted user is listed, but cannot be mentioned");
        assert!(approver.handle.is_none(), "an approver cannot be mentioned");
        assert!(view.members.iter().all(|m| !m.can_read_context), "no member is an agent");
    }

    #[tokio::test]
    async fn what_a_channel_holds_is_read_through_its_nodes_and_a_members_context_by_who_may() {
        use caelumora_model::channel::{ChannelState, HistoryLimit, HistoryStart};
        use caelumora_model::ledger::Change;
        use caelumora_model::message::{Message, MessageId};
        use caelumora_model::note::{NoteId, NoteOrigin};
        use caelumora_model::store::StateStore;

        let (store, manager) = owned_runtime();
        let (alice, bob) = (user("alice"), user("bob"));
        let helper = an_agent_of(&store, ActorId::User(alice.id)).await;
        let mut session = make_session("hers", ActorId::User(alice.id), vec![ActorId::User(bob.id), ActorId::Agent(helper.id)]);
        session.participants = record_users(&store, &[&alice, &bob]).await;
        session.participants.push(Participant::Agent(helper.id));
        let mut channel = main_channel(&session);
        let system = SecurityContext::system();
        store.create_session(&system, &session).await.unwrap();

        // What was said, of which bob - who joined later - sees the end only.
        let said = |t: &str| Message { content: Some(t.to_string()), ..Message::new(Classification::CHANNEL_DEFAULT) };
        let (first, second) = (said("first"), said("second"));
        channel.history_limits.push(HistoryLimit { participant: ActorId::User(bob.id), start: HistoryStart::After(first.id) });
        store.create_channel(&system, &channel).await.unwrap();
        let mut state = ChannelState::new(channel.id);
        state.history.extend(vec![first.clone(), second.clone()]);
        store.put_channel_state(channel.id, &state).await.unwrap();
        // What the agent noted and saw.
        let mut agent_state = caelumora_model::agent::AgentChannelState::default();
        agent_state.notes.push(Change::Add { id: NoteId(1), text: "noted".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.private_notes.push(Change::Add { id: NoteId(1), text: "mine".to_string(), classification: Classification::CHANNEL_DEFAULT }, NoteOrigin::default(), MessageId::default());
        agent_state.transcript.push(first.clone());
        store.put_agent_channel_state(helper.id, channel.id, &agent_state).await.unwrap();

        let reach = |viewer: Viewer| {
            let manager = &manager;
            async move { manager.views(viewer).session(session.id).await.unwrap().channel(channel.id).await.unwrap() }
        };
        let for_alice = reach(as_user(alice.id)).await;
        let texts = |messages: &[Message]| messages.iter().filter_map(|m| m.content.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&for_alice.history().await.unwrap().messages), ["first", "second"]);
        assert_eq!(texts(&reach(as_user(bob.id)).await.history().await.unwrap().messages), ["second"], "bob joined after the first");
        let notes = for_alice.notes().await.unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].agent, notes[0].name.as_str(), notes[0].notes[0].text.as_str()), (ActorId::Agent(helper.id), "Helper", "noted"));

        let agent = for_alice.member(ActorId::Agent(helper.id)).await.unwrap();
        let seen = agent.context().await.expect("alice made the agent");
        assert_eq!(seen.private_notes().0[0].text, "mine");
        assert_eq!(seen.transcript().messages().len(), 1);
        assert!(matches!(seen.context_of(first.id), Err(ContextError::NotRecorded { .. })), "the agent did not produce it");
        let refused = reach(as_user(bob.id)).await.member(ActorId::Agent(helper.id)).await.unwrap().context().await.expect_err("not bob's to read");
        assert!(matches!(refused, ViewError::Forbidden { .. }), "{refused}");
        let none = for_alice.member(ActorId::User(bob.id)).await.unwrap().context().await.expect_err("a user saw nothing");
        assert!(matches!(none, ViewError::NoContext { .. }), "{none}");
        assert!(matches!(for_alice.member(ActorId::System).await, Err(ViewError::MemberNotFound { .. })));
    }
}
