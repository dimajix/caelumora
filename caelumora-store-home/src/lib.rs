//! The backend of the stores of a program of one person (issue 554): every session is a directory
//! of its own below the data directory, and what lies in a scope lies in that scope's directory
//! (`HomeStore`), registered as the backend `home` by its plugin (`HomeStorePlugin`).

mod layout;
mod places;
pub mod plugin;

use caelumora_domain::store::{EntityKind, StoreResult};
use caelumora_store::documents::{DocumentOf, DocumentStorage};
use caelumora_store::history::HistoryStore;
use caelumora_store::transcript::TranscriptStore;
use caelumora_store::{PersistenceBackend, StateStore};
use caelumora_store_json::{
    to_backend_error, FileDocuments, JsonFormat, JsonStates, DEFAULT_JOURNAL_FLOOR,
};
use layout::{HomeDocuments, HomeStates};
use places::Places;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

/// The parameters of the backend `home`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeStoreConfig {
    /// What the program writes.
    pub data_dir: PathBuf,
    /// The kinds that lie with the document of their id of another kind - `memory` with `agent` -,
    /// by their names.
    #[serde(default)]
    pub within: BTreeMap<String, String>,
}

/// The stores of a program of one person, each kind where `Places` says. Several programs may use
/// one data directory at a time, a lock per kind keeping their writes apart.
#[derive(Debug)]
pub struct HomeStore {
    places: Arc<Places>,
    states: Arc<JsonStates>,
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<FileDocuments>>>,
}

impl HomeStore {
    /// The store of the files below the data directory of `config`.
    ///
    /// # Errors
    /// If the directories cannot be made, so that a path that cannot be written fails the start.
    pub fn new(config: &HomeStoreConfig) -> StoreResult<Self> {
        let places = Arc::new(Places::new(config.data_dir.clone(), config.within.clone()));
        fs::create_dir_all(places.locks()).map_err(to_backend_error)?;
        let states =
            JsonStates::new(Arc::new(HomeStates::new(Arc::clone(&places))), DEFAULT_JOURNAL_FLOOR);
        Ok(Self { places, states: Arc::new(states), documents: RwLock::default() })
    }
}

impl PersistenceBackend for HomeStore {
    fn documents(
        &self,
        kind: EntityKind,
        document_of: DocumentOf,
    ) -> StoreResult<Arc<dyn DocumentStorage>> {
        let mut all = self.documents.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(storage) = all.get(&kind) {
            return Ok(Arc::<FileDocuments>::clone(storage));
        }
        let layout = HomeDocuments::new(
            Arc::clone(&self.places),
            self.places.shelf_of(kind),
            self.places.locks().join(kind.name()),
        );
        let format = Arc::new(JsonFormat);
        let storage = Arc::new(FileDocuments::new(kind, Arc::new(layout), format, document_of));
        all.insert(kind, Arc::clone(&storage));
        Ok(storage)
    }

    fn state_store(self: Arc<Self>) -> StoreResult<Arc<dyn StateStore>> {
        Ok(Arc::<JsonStates>::clone(&self.states))
    }

    fn history_store(self: Arc<Self>) -> StoreResult<Arc<dyn HistoryStore>> {
        Ok(Arc::<JsonStates>::clone(&self.states))
    }

    fn transcript_store(self: Arc<Self>) -> StoreResult<Arc<dyn TranscriptStore>> {
        Ok(Arc::<JsonStates>::clone(&self.states))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{Agent, AgentChannelState, AgentId, PersonalityId};
    use caelumora_domain::channel::testing::make_channel;
    use caelumora_domain::channel::{Channel, ChannelId, ChannelState};
    use caelumora_domain::group::Group;
    use caelumora_domain::project::Project;
    use caelumora_domain::query::Query;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{UserChannelState, UserId, UserProfile};
    use caelumora_domain::value::name::{Name, ScopedName};
    use caelumora_security::{OwnershipPolicy, Policies, SecurityContext};
    use caelumora_store::{suite, EntityStores, Persistence};
    use std::path::Path;
    use tempfile::tempdir;
    use tokio::task::JoinSet;

    /// The store below `dir`, a profile lying with its user.
    fn store(dir: &Path) -> Arc<HomeStore> {
        let within = [("user_profile".to_string(), "user".to_string())].into();
        Arc::new(HomeStore::new(&HomeStoreConfig { data_dir: dir.to_path_buf(), within }).unwrap())
    }

    fn persistence(store: &Arc<HomeStore>) -> Persistence {
        Persistence::over(Arc::<HomeStore>::clone(store)).unwrap()
    }

    fn opened(store: &Arc<HomeStore>) -> EntityStores {
        EntityStores::open(&persistence(store), &Policies::permitting_all()).unwrap()
    }

    /// The stores below `dir`, which permit everything.
    fn stores(dir: &Path) -> EntityStores {
        opened(&store(dir))
    }

    fn removal() -> Removal {
        Removal::now(ActorId::System)
    }

    #[tokio::test]
    async fn behaves_as_an_entity_store() {
        suite::entity_store_behaves(&stores(tempdir().unwrap().path())).await;
    }

    #[tokio::test]
    async fn lists_what_check_decides() {
        let dir = tempdir().unwrap();
        let (memberships, access_policy) = suite::admitting_policy();
        let store = persistence(&store(dir.path()));
        let policies = Policies::by_default_rules(Arc::clone(&access_policy));
        let folders = store.entity_store::<suite::Folder>(&policies).unwrap();
        suite::lists_admit_what_check_decides(
            &EntityStores::open(&store, &policies).unwrap(),
            folders.as_ref(),
            &memberships,
            access_policy.as_ref(),
        )
        .await;
    }

    #[tokio::test]
    async fn answers_queries() {
        let dir = tempdir().unwrap();
        let store = persistence(&store(dir.path()));
        suite::queries_behave(&EntityStores::open(&store, &Policies::permitting_all()).unwrap())
            .await;
        suite::queries_reach_parts(
            store.entity_store::<suite::Folder>(&Policies::permitting_all()).unwrap().as_ref(),
        )
        .await;
    }

    #[tokio::test]
    async fn keeps_the_states() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let states = Arc::clone(&store).state_store().unwrap();
        let histories = Arc::clone(&store).history_store().unwrap();
        let transcripts = store.transcript_store().unwrap();
        suite::state_store_behaves(states.as_ref()).await;
        suite::work_under_way_is_indexed(states.as_ref()).await;
        suite::history_store_behaves(states.as_ref(), histories.as_ref()).await;
        suite::transcript_store_behaves(states.as_ref(), transcripts.as_ref()).await;
        suite::given_store_behaves(states.as_ref(), transcripts.as_ref()).await;
    }

    #[tokio::test]
    async fn keeps_what_is_deleted_marked() {
        suite::deleting_marks(&stores(tempdir().unwrap().path())).await;
    }

    #[tokio::test]
    async fn judges_writes_on_their_revision() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        suite::writes_are_judged_on_their_revision(&opened(&store)).await;
        suite::storage_judges_writes_on_their_revision(store.as_ref()).await;
    }

    #[tokio::test]
    async fn begins_purges() {
        suite::purges_are_begun(&stores(tempdir().unwrap().path())).await;
    }

    #[tokio::test]
    async fn keeps_names_unique() {
        suite::names_are_unique(&stores(tempdir().unwrap().path())).await;
        let dir = tempdir().unwrap();
        suite::names_are_unique_beyond_what_is_read(
            &EntityStores::open(
                &persistence(&store(dir.path())),
                &Policies::by_default_rules(Arc::new(OwnershipPolicy::new())),
            )
            .unwrap(),
        )
        .await;
    }

    /// An agent named `name` in `scope`.
    fn agent(name: &str, scope: Scope) -> Agent {
        Agent::new(
            ScopedName::new(name, scope).unwrap(),
            PersonalityId::new(),
            ActorId::User(UserId::new()),
        )
    }

    #[tokio::test]
    async fn what_lies_in_a_scope_lies_in_its_directory() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let session_dir = dir.path().join("sessions").join(session.to_string());
        for (scope, holder) in [
            (Scope::GlobalScope, dir.path().to_path_buf()),
            (Scope::UserScope { user_id: UserId::new() }, dir.path().to_path_buf()),
            (Scope::SessionScope { session_id: session }, session_dir.clone()),
            (
                Scope::ChannelScope { session_id: session, channel_id: channel },
                session_dir.join("channels").join(channel.to_string()),
            ),
        ] {
            let mut agent = agent("helper", scope);
            stores.agents.create(&system, &mut agent).await.unwrap();
            let path = holder.join("agents").join(agent.id.to_string()).join("agent.json");
            assert!(path.is_file(), "{}", path.display());
            let found = stores.agents.get(&system, agent.id).await.expect("found by its id");
            assert_eq!(found.id, agent.id);
            assert_eq!(
                stores.agents.find_by_name(&system, &agent.name).await.expect("and its name").id,
                agent.id
            );
        }
    }

    /// What lies in a project lies where the project does: a session's own in the directory of
    /// its session, a shared one in `projects`.
    #[tokio::test]
    async fn what_lies_in_a_project_lies_where_the_project_does() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let (session, owner) = (SessionId::new(), UserId::new());
        let mut own = Project::own_of(session, owner);
        let name = ScopedName::new("report", Scope::UserScope { user_id: owner }).unwrap();
        let mut shared = Project::new(name, owner);
        stores.projects.create(&system, &mut own).await.unwrap();
        stores.projects.create(&system, &mut shared).await.unwrap();

        let sessions = dir.path().join("sessions").join(session.to_string());
        for (project, parent) in [(&own, sessions), (&shared, dir.path().to_path_buf())] {
            let holder = parent.join("projects").join(project.id.to_string());
            assert!(holder.join("project.json").is_file(), "{}", holder.display());
            let mut group = Group::of_project(project);
            stores.groups.create(&system, &mut group).await.unwrap();
            assert!(holder.join("groups").join(group.id.to_string()).join("group.json").is_file());
            assert!(stores.groups.get(&system, group.id).await.is_ok(), "found by its id");
        }
    }

    #[tokio::test]
    async fn a_session_is_one_directory_that_goes_with_it() {
        let dir = tempdir().unwrap();
        let store = store(dir.path());
        let stores = opened(&store);
        let states = Arc::clone(&store).state_store().unwrap();
        let system = SecurityContext::system();
        let id = SessionId::new();
        stores.sessions.create(&system, &mut bare_session(id)).await.unwrap();
        let mut channel: Channel = make_channel(id, "main");
        stores.channels.create(&system, &mut channel).await.unwrap();
        let (agent, user) = (AgentId::new(), UserId::new());
        states.put_channel_state(channel.id, &mut ChannelState::new(channel.id)).await.unwrap();
        states
            .put_agent_channel_state(agent, channel.id, &mut AgentChannelState::default())
            .await
            .unwrap();
        states
            .put_user_channel_state(user, channel.id, &mut UserChannelState::default())
            .await
            .unwrap();

        let session_dir = dir.path().join("sessions").join(id.to_string());
        let channel_dir = session_dir.join("channels").join(channel.id.to_string());
        assert!(session_dir.join("session.json").is_file());
        assert!(channel_dir.join("channel.json").is_file());
        assert!(channel_dir.join("state/channel.json").is_file());
        assert!(channel_dir.join(format!("state/agents/{agent}.json")).is_file());
        assert!(channel_dir.join(format!("state/users/{user}.json")).is_file());

        channel.name = Name::new("renamed").unwrap();
        stores.channels.update(&system, &mut channel).await.unwrap();
        assert!(channel_dir.join("channel.json").is_file(), "a new name moves nothing");

        states.delete_channel_state(channel.id).await.unwrap();
        stores.channels.delete(&system, channel.id, &removal()).await.unwrap();
        assert!(stores.channels.purge(&system, channel.id).await.unwrap());
        assert!(!channel_dir.exists(), "a channel goes whole");
        stores.sessions.delete(&system, id, &removal()).await.unwrap();
        assert!(stores.sessions.purge(&system, id).await.unwrap());
        assert!(!session_dir.exists(), "and its session");
    }

    /// What is in a document's directory and did not go before it stays: only what is empty goes.
    #[tokio::test]
    async fn what_is_left_in_a_directory_stays() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let id = SessionId::new();
        stores.sessions.create(&system, &mut bare_session(id)).await.unwrap();
        let session_dir = dir.path().join("sessions").join(id.to_string());
        fs::write(session_dir.join("notes.txt"), "mine").unwrap();

        stores.sessions.delete(&system, id, &removal()).await.unwrap();
        assert!(stores.sessions.purge(&system, id).await.unwrap());

        assert!(session_dir.join("notes.txt").is_file());
        assert!(!session_dir.join("session.json").exists());
    }

    #[tokio::test]
    async fn the_state_of_a_channel_no_document_names_lies_apart() {
        let dir = tempdir().unwrap();
        let states = store(dir.path()).state_store().unwrap();
        let channel = ChannelId::new();
        states.put_channel_state(channel, &mut ChannelState::new(channel)).await.unwrap();
        assert!(dir.path().join(format!("channels/{channel}/state/channel.json")).is_file());
    }

    #[tokio::test]
    async fn a_kind_named_so_lies_with_the_document_of_its_id() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let mut user = make_user("alice");
        stores.users.create(&system, &mut user).await.unwrap();
        let mut profile = UserProfile::new(user.id, None);
        stores.user_profiles.create(&system, &mut profile).await.unwrap();

        let user_dir = dir.path().join("users").join(user.id.to_string());
        assert!(user_dir.join("user.json").is_file());
        assert!(user_dir.join("user_profile.json").is_file());
        assert!(stores.user_profiles.get(&system, user.id).await.is_ok());
        assert_eq!(stores.user_profiles.list(&system, &Query::all()).await.unwrap().items.len(), 1);

        let alone = UserId::new();
        stores.user_profiles.create(&system, &mut UserProfile::new(alone, None)).await.unwrap();
        assert!(dir.path().join(format!("users/{alone}/user_profile.json")).is_file());
        assert!(stores.user_profiles.get(&system, alone).await.is_ok(), "none it lies beside yet");
    }

    /// A copy of a directory, not named by an id, is nothing of what it holds: what lies in it
    /// is found once.
    #[tokio::test]
    async fn a_directory_not_named_by_an_id_is_passed_over() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let id = SessionId::new();
        stores.sessions.create(&system, &mut bare_session(id)).await.unwrap();
        let mut channel = make_channel(id, "main");
        stores.channels.create(&system, &mut channel).await.unwrap();
        let session_dir = dir.path().join("sessions").join(id.to_string());
        let copy = dir.path().join("sessions").join(format!("{id}.bak"));
        let channel_dir = copy.join("channels").join(channel.id.to_string());
        fs::create_dir_all(&channel_dir).unwrap();
        let original = session_dir.join("channels").join(channel.id.to_string());
        fs::copy(original.join("channel.json"), channel_dir.join("channel.json")).unwrap();

        let listed = stores.channels.list(&system, &Query::all()).await.unwrap();
        assert_eq!(listed.items.len(), 1, "the copy is no channel");
    }

    /// What another program writes and removes is seen, although it was remembered as it was.
    #[tokio::test]
    async fn what_another_program_changes_is_seen() {
        let dir = tempdir().unwrap();
        let (mine, other) = (stores(dir.path()), stores(dir.path()));
        let system = SecurityContext::system();
        let id = SessionId::new();
        other.sessions.create(&system, &mut bare_session(id)).await.unwrap();
        assert!(mine.sessions.get(&system, id).await.is_ok(), "found though it was not looked for");

        other.sessions.delete(&system, id, &removal()).await.unwrap();
        assert!(other.sessions.purge(&system, id).await.unwrap());
        assert!(
            matches!(mine.sessions.get(&system, id).await, Err(StoreError::NotFound { .. })),
            "and is gone though it was remembered"
        );
    }

    #[tokio::test]
    async fn two_programs_do_not_take_one_name() {
        let dir = tempdir().unwrap();
        for round in 0..8 {
            let mut writes = JoinSet::new();
            for _ in 0..2 {
                let personalities = stores(dir.path()).personalities;
                let mut personality = make_personality(&format!("taken-{round}"));
                writes.spawn(async move {
                    personalities.create(&SecurityContext::system(), &mut personality).await
                });
            }
            let answers = writes.join_all().await;
            let made = answers.iter().filter(|answer| answer.is_ok()).count();
            assert_eq!(made, 1, "{answers:?}");
        }
    }
}
