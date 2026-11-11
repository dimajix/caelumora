//! The backends of a program of one person (issue 554), each laying out what it keeps below one
//! data directory: the stores (`HomeStore`), every session a directory of its own and what lies in
//! a scope in that scope's directory, and the content (`HomeContent`), with what it belongs to;
//! registered as the backends `home` by its plugin (`HomeStorePlugin`).

mod content;
mod definitions;
mod layout;
mod places;
pub mod plugin;

pub use content::{HomeContent, HomeContentConfig};
pub use definitions::{DefinitionFiles, FileFormat};

use caelumora_domain::session::SessionId;
use caelumora_domain::store::{EntityKind, StoreResult};
use caelumora_store::documents::{DocumentOf, DocumentStorage};
use caelumora_store::history::HistoryStore;
use caelumora_store::sessions::{SessionHold, SessionLocks};
use caelumora_store::transcript::TranscriptStore;
use caelumora_store::{PersistenceBackend, StateStore};
use caelumora_store_json::{
    lock_file, to_backend_error, FileDocuments, JsonFormat, JsonStates, DEFAULT_JOURNAL_FLOOR,
};
use definitions::Definitions;
use layout::{HomeDocuments, HomeStates};
use places::Places;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, TryLockError};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

/// The parameters of the backend `home`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeStoreConfig {
    /// What the person writes: the definitions of the global scope.
    pub config_dir: PathBuf,
    /// What the program writes.
    pub data_dir: PathBuf,
    /// Which kinds are definitions, by their names, and how each is written; any other is kept as
    /// state.
    #[serde(default)]
    pub definitions: BTreeMap<String, DefinitionFiles>,
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
    definitions: BTreeMap<String, DefinitionFiles>,
    states: Arc<JsonStates>,
    /// The storage of every kind asked for, by its name.
    documents: RwLock<HashMap<EntityKind, Arc<FileDocuments>>>,
}

impl HomeStore {
    /// The store of the files below the data directory of `config`.
    ///
    /// # Errors
    /// If the directories cannot be made, so that a path that cannot be written fails the start;
    /// `StoreError::Validation` for definitions of a directory that is not one of their own.
    pub fn new(config: &HomeStoreConfig) -> StoreResult<Self> {
        for (kind, files) in &config.definitions {
            files.check(kind)?;
        }
        let places = Arc::new(Places::new(
            config.data_dir.clone(),
            Some(config.config_dir.clone()),
            config.within.clone(),
        ));
        fs::create_dir_all(places.locks()).map_err(to_backend_error)?;
        let states =
            JsonStates::new(Arc::new(HomeStates::new(Arc::clone(&places))), DEFAULT_JOURNAL_FLOOR);
        Ok(Self {
            places,
            definitions: config.definitions.clone(),
            states: Arc::new(states),
            documents: RwLock::default(),
        })
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
        let lock = self.places.locks().join(kind.name());
        let storage = Arc::new(match self.definitions.get(kind.name()) {
            Some(files) => {
                let places = Arc::clone(&self.places);
                let definitions =
                    Arc::new(Definitions::new(places, kind, files, document_of, lock));
                FileDocuments::new(
                    kind,
                    Arc::<Definitions>::clone(&definitions),
                    definitions,
                    document_of,
                )
            }
            None => {
                let shelf = self.places.shelf_of(kind);
                let layout = HomeDocuments::new(Arc::clone(&self.places), shelf, lock);
                FileDocuments::new(kind, Arc::new(layout), Arc::new(JsonFormat), document_of)
            }
        });
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

    fn session_locks(self: Arc<Self>) -> Option<Arc<dyn SessionLocks>> {
        Some(self)
    }
}

/// A session is held by the lock of its file of the file system (`Places::session_lock`), which
/// every program over the same data directory asks, and the end of the process lets go of - also
/// one that crashed. The file stays until the session is purged.
impl SessionLocks for HomeStore {
    fn try_hold(&self, id: SessionId) -> StoreResult<Option<SessionHold>> {
        let file = lock_file(&self.places.session_lock(id)).map_err(to_backend_error)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(SessionHold::new(file))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(to_backend_error(e)),
        }
    }

    fn forget(&self, id: SessionId) -> StoreResult<()> {
        match fs::remove_file(self.places.session_lock(id)) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(to_backend_error(e)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::{Agent, AgentChannelState, AgentId, Personality, PersonalityId};
    use caelumora_domain::channel::testing::make_channel;
    use caelumora_domain::channel::{Channel, ChannelId, ChannelState};
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::project::Project;
    use caelumora_domain::query::Query;
    use caelumora_domain::removal::Removal;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::session::testing::bare_session;
    use caelumora_domain::session::SessionId;
    use caelumora_domain::store::{StoreError, Stored};
    use caelumora_domain::template::testing::make_template;
    use caelumora_domain::template::SessionTemplate;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{UserChannelState, UserId, UserProfile};
    use caelumora_domain::value::name::{Name, ScopedName};
    use caelumora_domain::value::text::Briefing;
    use caelumora_domain::value::text::Description;
    use caelumora_security::{OwnershipPolicy, Policies, SecurityContext};
    use caelumora_store::{suite, EntityStores, Persistence};
    use definitions::derived_id;
    use std::path::Path;
    use tempfile::tempdir;
    use tokio::task::JoinSet;
    use uuid::Uuid;

    /// The definitions of `kind` in the directory `dir`, written as `format` says.
    fn definitions(kind: &str, dir: &str, format: FileFormat) -> (String, DefinitionFiles) {
        (kind.to_string(), DefinitionFiles { dir: dir.to_string(), format })
    }

    /// What the store below `dir` is made of: its data in `dir`, its definitions - the
    /// personalities, the templates and the bundles - in `dir/config`, a profile lying with its
    /// user.
    fn config(dir: &Path) -> HomeStoreConfig {
        HomeStoreConfig {
            config_dir: dir.join("config"),
            data_dir: dir.to_path_buf(),
            definitions: [
                definitions("personality", "personalities", FileFormat::Yaml),
                definitions("session_template", "templates", FileFormat::Yaml),
                definitions("component_bundle", "bundles", FileFormat::Yaml),
            ]
            .into(),
            within: [("user_profile".to_string(), "user".to_string())].into(),
        }
    }

    /// The store below `dir`, as `config` makes it.
    fn store(dir: &Path) -> Arc<HomeStore> {
        Arc::new(HomeStore::new(&config(dir)).unwrap())
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

    /// The personality `name` of the global scope.
    fn global(name: &str) -> ScopedName {
        ScopedName::new(name, Scope::GlobalScope).unwrap()
    }

    /// The text of the file at `path`.
    fn text(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// The id of a name is the same always (issue 554): one that changed would show that names
    /// are folded otherwise, and every file without an id would be another definition.
    #[test]
    fn the_id_of_a_name_stays_the_same() {
        assert_eq!(
            derived_id(Personality::KIND, &global("plain")).to_string(),
            "345476c2-4f68-5832-8e80-c61710dd286a"
        );
    }

    /// A definition written by hand is read by its name, with the id its name derives.
    #[tokio::test]
    async fn a_definition_written_by_hand_is_found_by_its_name() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        fs::write(personalities.join("plain.yaml"), "description: A plain one\n").unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();

        let found = stores.personalities.find_by_name(&system, &global("plain")).await.unwrap();

        assert_eq!(found.id.0, derived_id(Personality::KIND, &global("plain")));
        assert_eq!(found.description.unwrap().as_str(), "A plain one");
        assert!(stores.personalities.get(&system, found.id).await.is_ok(), "and by its id");
    }

    /// What the file's name and place tell, and what reading it without gives, is not written.
    #[tokio::test]
    async fn a_definition_is_written_without_what_its_file_tells() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let id = derived_id(Personality::KIND, &global("plain"));
        let mut personality = Personality { id: id.into(), ..make_personality("plain") };
        stores.personalities.create(&system, &mut personality).await.unwrap();

        let path = dir.path().join("config/personalities/plain.yaml");
        let written = text(&path);
        for left_out in ["id:", "name:", "scope:", "security_attributes:"] {
            assert!(!written.contains(left_out), "{left_out} in {written}");
        }
        assert!(written.contains("revision:"), "a revision another than reading gives: {written}");

        personality.name = global("renamed");
        stores.personalities.update(&system, &mut personality).await.unwrap();
        let renamed = dir.path().join("config/personalities/renamed.yaml");
        assert!(!path.exists() && renamed.is_file(), "renamed, its file is");
        assert!(text(&renamed).contains(&id.to_string()), "with the id its name no longer derives");
        let found = stores.personalities.find_by_name(&system, &global("renamed")).await.unwrap();
        assert_eq!(found.id, personality.id);
    }

    /// A file moved by hand without an id is another definition; one with an id is the same.
    #[tokio::test]
    async fn a_file_moved_by_hand_keeps_its_definition_by_its_id_alone() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        fs::write(personalities.join("plain.yaml"), "{}\n").unwrap();
        let kept = Uuid::new_v4();
        fs::write(personalities.join("kept.yaml"), format!("id: {kept}\n")).unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let plain = derived_id(Personality::KIND, &global("plain"));

        fs::rename(personalities.join("plain.yaml"), personalities.join("moved.yaml")).unwrap();
        fs::rename(personalities.join("kept.yaml"), personalities.join("still.yaml")).unwrap();

        assert!(stores.personalities.get(&system, plain.into()).await.is_err(), "a new identity");
        let moved = stores.personalities.find_by_name(&system, &global("moved")).await.unwrap();
        assert_eq!(moved.id.0, derived_id(Personality::KIND, &global("moved")));
        let still = stores.personalities.get(&system, kept.into()).await.expect("found by its id");
        assert_eq!(still.name, global("still"));
    }

    /// Two files of one id, or of one name as names compare, are both left out.
    #[tokio::test]
    async fn two_files_of_one_definition_are_both_left_out() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        let id = Uuid::new_v4();
        fs::write(personalities.join("one.yaml"), format!("id: {id}\n")).unwrap();
        fs::write(personalities.join("copy.yaml"), format!("id: {id}\n")).unwrap();
        fs::write(personalities.join("Zoë.yaml"), "{}\n").unwrap();
        fs::write(personalities.join("zoe.yaml"), "{}\n").unwrap();
        fs::write(personalities.join("alone.yaml"), "{}\n").unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();

        let listed = stores.personalities.list(&system, &Query::all()).await.unwrap().items;

        let names: Vec<_> = listed.iter().map(|p| p.name.as_str().to_string()).collect();
        assert_eq!(names, ["alone"]);
        assert!(stores.personalities.get(&system, id.into()).await.is_err());
    }

    /// A deleted definition frees its name, and takes it again when it is restored.
    #[tokio::test]
    async fn a_deleted_definition_frees_its_name() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let mut first = make_personality("plain");
        stores.personalities.create(&system, &mut first).await.unwrap();
        let path = dir.path().join("config/personalities/plain.yaml");

        stores.personalities.delete(&system, first.id, &removal()).await.unwrap();
        assert!(!path.exists(), "out of the way");
        let deleted = dir.path().join(format!("config/personalities/.deleted/{}.yaml", first.id));
        assert!(text(&deleted).contains("plain"), "by its id, its name in it");
        let mut second = make_personality("plain");
        stores.personalities.create(&system, &mut second).await.expect("its name is free");
        assert!(matches!(
            stores.personalities.restore(&system, first.id).await,
            Err(StoreError::NameTaken { .. })
        ));
        stores.personalities.delete(&system, second.id, &removal()).await.unwrap();
        assert!(stores.personalities.purge(&system, second.id).await.unwrap());
        stores.personalities.restore(&system, first.id).await.expect("its name free again");
        assert!(path.is_file(), "by its name again");
    }

    /// A definition of a session, a user lies in their directories, its scope told by them.
    #[tokio::test]
    async fn a_definition_lies_in_the_directory_of_its_scope() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let session = SessionId::new();
        let user = UserId::new();
        let group = GroupId::new();
        for (scope, place) in [
            (Scope::SessionScope { session_id: session }, format!("sessions/{session}")),
            (Scope::UserScope { user_id: user }, format!("users/{user}")),
            (Scope::GroupScope { group_id: group }, format!("groups/{group}")),
        ] {
            let name = ScopedName::new("plain", scope).unwrap();
            let mut personality = Personality { name: name.clone(), ..make_personality("x") };
            stores.personalities.create(&system, &mut personality).await.unwrap();
            let path = dir.path().join(place).join("personalities/plain.yaml");
            assert!(!text(&path).contains("scope:"), "told by its directory: {}", path.display());
            assert_eq!(stores.personalities.find_by_name(&system, &name).await.unwrap().name, name);
        }
    }

    /// A definition in Markdown has the text after its front matter as a field.
    #[tokio::test]
    async fn a_definition_in_markdown_has_its_text_as_a_field() {
        let dir = tempdir().unwrap();
        let mut config = config(dir.path());
        let body = "description".to_string();
        config.definitions.insert(
            "personality".to_string(),
            DefinitionFiles {
                dir: "personalities".to_string(),
                format: FileFormat::Markdown { body },
            },
        );
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        fs::write(
            personalities.join("plain.md"),
            "---\nintroduction: I am plain.\n---\nA plain one",
        )
        .unwrap();
        let stores = opened(&Arc::new(HomeStore::new(&config).unwrap()));
        let system = SecurityContext::system();

        let mut found = stores.personalities.find_by_name(&system, &global("plain")).await.unwrap();
        assert_eq!(found.description.as_ref().unwrap().as_str(), "A plain one");
        assert_eq!(found.introduction.as_ref().unwrap().as_str(), "I am plain.");

        found.description = Some(Description::new("Plainer").unwrap());
        stores.personalities.update(&system, &mut found).await.unwrap();
        let written = text(&personalities.join("plain.md"));
        assert!(written.starts_with("---\n") && written.ends_with("---\nPlainer"), "{written}");
        let read = stores.personalities.get(&system, found.id).await.unwrap();
        assert_eq!(read.description.unwrap().as_str(), "Plainer");
        assert_eq!(read.introduction.unwrap().as_str(), "I am plain.", "the front matter kept");
    }

    /// A document named so has a link beside its directory by its name, which follows a new name
    /// and goes with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_named_document_has_a_link_by_its_name() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let session = SessionId::new();
        let mut channel = make_channel(session, "main");
        stores.channels.create(&system, &mut channel).await.unwrap();
        let channels = dir.path().join(format!("sessions/{session}/channels"));
        let id = channel.id.to_string();
        assert_eq!(fs::read_link(channels.join("main")).unwrap(), Path::new(&id));

        channel.name = Name::new("renamed").unwrap();
        stores.channels.update(&system, &mut channel).await.unwrap();
        assert_eq!(fs::read_link(channels.join("renamed")).unwrap(), Path::new(&id));
        assert!(fs::symlink_metadata(channels.join("main")).is_err(), "the old one gone");

        stores.channels.delete(&system, channel.id, &removal()).await.unwrap();
        assert!(stores.channels.purge(&system, channel.id).await.unwrap());
        assert!(fs::symlink_metadata(channels.join("renamed")).is_err(), "and with it");
    }

    /// What lies in a channel's scope does not keep the channel's link, nor its session's
    /// directory, from going.
    #[tokio::test]
    async fn a_session_goes_whole_with_what_lay_in_its_channels() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let session = SessionId::new();
        stores.sessions.create(&system, &mut bare_session(session)).await.unwrap();
        let mut channel = make_channel(session, "main");
        stores.channels.create(&system, &mut channel).await.unwrap();
        let in_channel = Scope::ChannelScope { session_id: session, channel_id: channel.id };
        let mut helper = agent("helper", in_channel);
        stores.agents.create(&system, &mut helper).await.unwrap();

        stores.channels.delete(&system, channel.id, &removal()).await.unwrap();
        assert!(stores.channels.purge(&system, channel.id).await.unwrap());
        stores.agents.delete(&system, helper.id, &removal()).await.unwrap();
        assert!(stores.agents.purge(&system, helper.id).await.unwrap());
        stores.sessions.delete(&system, session, &removal()).await.unwrap();
        assert!(stores.sessions.purge(&system, session).await.unwrap());

        assert!(!dir.path().join(format!("sessions/{session}")).exists());
    }

    /// What the program writes in YAML and Markdown is read back as it was, many lines too.
    #[tokio::test]
    async fn what_is_written_is_read_back() {
        let dir = tempdir().unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        let briefing = "One line,\n\nanother after a blank one,\n  and one indented.";
        let mut template = SessionTemplate {
            briefing: Some(Briefing::new(briefing).unwrap()),
            ..make_template("briefed")
        };
        stores.session_templates.create(&system, &mut template).await.unwrap();
        let read = stores.session_templates.get(&system, template.id).await.unwrap();
        assert_eq!(read.briefing.unwrap().as_str(), briefing);
    }

    /// A field `name` in a file named by its name is not what it is found by.
    #[tokio::test]
    async fn a_definition_is_named_by_its_file() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        fs::write(personalities.join("plain.yaml"), "name: other\n").unwrap();
        fs::write(personalities.join(".swap.yaml"), "{}\n").unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();

        assert!(stores.personalities.find_by_name(&system, &global("plain")).await.is_ok());
        assert!(stores.personalities.find_by_name(&system, &global("other")).await.is_err());
        let listed = stores.personalities.list(&system, &Query::all()).await.unwrap().items;
        assert_eq!(listed.len(), 1, "a hidden file is no definition");
    }

    /// A copy made after the definitions were read is seen, and both are left out.
    #[tokio::test]
    async fn a_copy_made_later_is_seen() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        let id = Uuid::new_v4();
        fs::write(personalities.join("one.yaml"), format!("id: {id}\n")).unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();
        assert!(stores.personalities.get(&system, id.into()).await.is_ok());

        fs::copy(personalities.join("one.yaml"), personalities.join("copy.yaml")).unwrap();

        assert!(stores.personalities.get(&system, id.into()).await.is_err());
    }

    #[test]
    fn definitions_lie_in_one_directory_of_their_own() {
        let dir = tempdir().unwrap();
        for wrong in ["", "a/b", ".deleted", ".."] {
            let mut config = config(dir.path());
            config.definitions.insert(
                "personality".to_string(),
                DefinitionFiles { dir: wrong.to_string(), format: FileFormat::Yaml },
            );
            assert!(HomeStore::new(&config).is_err(), "{wrong}");
        }
    }

    /// An id put into a file stays in it, also one its name derives, where its file is moved:
    /// deleted and restored.
    #[tokio::test]
    async fn an_id_once_in_a_file_stays() {
        let dir = tempdir().unwrap();
        let personalities = dir.path().join("config/personalities");
        fs::create_dir_all(&personalities).unwrap();
        let id = derived_id(Personality::KIND, &global("plain"));
        fs::write(personalities.join("plain.yaml"), format!("id: {id}\n")).unwrap();
        let stores = stores(dir.path());
        let system = SecurityContext::system();

        stores.personalities.delete(&system, id.into(), &removal()).await.unwrap();
        let deleted = personalities.join(format!(".deleted/{id}.yaml"));
        assert!(text(&deleted).contains("id:"), "{}", text(&deleted));
        stores.personalities.restore(&system, id.into()).await.unwrap();

        let restored = text(&personalities.join("plain.yaml"));
        assert!(restored.contains(&format!("id: {id}")), "{restored}");
    }

    /// Issue 554: a session is held by one process - here, by one hold, as a second process asks
    /// with a file of its own -, let go as the hold is dropped; what held a session purged goes.
    #[test]
    fn a_session_is_held_once_until_it_is_let_go() {
        let dir = tempfile::tempdir().unwrap();
        let locks = store(dir.path()).session_locks().expect("shared by processes");
        let (one, other) = (SessionId::new(), SessionId::new());

        let held = locks.try_hold(one).unwrap().expect("held");
        assert!(locks.try_hold(one).unwrap().is_none(), "not twice");
        let beside = locks.try_hold(other).unwrap().expect("another session beside it");
        drop(held);
        let again = locks.try_hold(one).unwrap().expect("held again once let go");

        let file = dir.path().join("locks/sessions").join(one.to_string());
        assert!(file.is_file(), "{}", file.display());
        locks.forget(one).unwrap();
        assert!(!file.exists(), "nothing of it stays once it is purged");
        locks.forget(one).unwrap();
        drop((again, beside));
    }
}
