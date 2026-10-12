use crate::role::RoleId;
use crate::security::SecurityContext;
use crate::store::{EntityStore, StoreError, StoreResultExt};
use crate::user::{User, UserId};
use crate::user_provider::{AuthError, AuthResult, Credential, UserProvider};
use crate::value::name::{Name, ScopedName};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use crate::scope::Scope;

/// **Development only.** Accepts every username with any credential.
///
/// A stand-in until a real backend exists (issue 25), so that login and
/// sessions (issue 10) can be built against the `UserProvider` trait. It must
/// be enabled explicitly in the configuration and is never part of a default
/// chain; `AppConfig::create_user_manager` warns loudly when it is.
///
/// Its source is the `EntityStore` itself and the configured roles: a name it
/// finds there is that user, any other a new one with a random id, which the
/// `UserManager` stores (issue 306) - later logins find it again. Ids are
/// therefore as stable as the store — across restarts with `JsonFileStore`,
/// only for the lifetime of the process with `MemoryStore`.
///
/// Find-or-create is not atomic: of two concurrent *first* logins under the
/// same name, the store lets one create the user and refuses the other its
/// name (issue 250), whose login then fails. Accepted for a development
/// stand-in.
#[derive(Debug)]
pub struct AcceptAnyUserProvider {
    store: Arc<dyn EntityStore<User>>,
    /// The roles of the users named (issue 142), by their ids (issue 23).
    roles: BTreeMap<String, Vec<RoleId>>,
    /// The roles of everybody else.
    default_roles: Vec<RoleId>,
}

impl AcceptAnyUserProvider {
    /// A provider that gives nobody a role.
    pub fn new(store: Arc<dyn EntityStore<User>>) -> Self {
        Self::with_roles(store, BTreeMap::new(), Vec::new())
    }

    /// A provider that gives the users named in `roles` theirs, and
    /// everybody else `default_roles`.
    pub fn with_roles(store: Arc<dyn EntityStore<User>>, roles: BTreeMap<String, Vec<RoleId>>, default_roles: Vec<RoleId>) -> Self {
        Self { store, roles, default_roles }
    }

    /// The roles of `username`, as configured - named as names are
    /// compared (issue 250).
    fn roles_of(&self, username: &Name) -> Vec<RoleId> {
        self.roles.iter()
            .find(|(configured, _)| *username == configured.as_str())
            .map_or_else(|| self.default_roles.clone(), |(_, roles)| roles.clone())
    }

    /// The provider keeps its own records: nobody's action, the system's.
    fn context() -> SecurityContext {
        SecurityContext::system()
    }

    /// `user` with the roles the configuration gives them: it decides, also
    /// for a user already stored.
    fn configured(&self, user: User) -> User {
        let roles = self.roles_of(&user.username);
        User { roles, ..user }
    }

    /// The user `result` found, `None` if there is none.
    fn found(result: Result<User, StoreError>) -> AuthResult<Option<User>> {
        result.optional().map_err(|e| AuthError::Backend(e.to_string()))
    }
}

#[async_trait]
impl UserProvider for AcceptAnyUserProvider {
    async fn find_user(&self, username: &str) -> AuthResult<Option<User>> {
        // A username is a name in the global scope (issue 250); one that
        // is no valid name is nobody's.
        let Ok(name) = ScopedName::new(username, Scope::GlobalScope) else { return Ok(None) };
        Self::found(self.store.find_by_name(&Self::context(), &name).await)
    }

    /// The user as stored, with the roles the configuration gives them now -
    /// what the `UserManager` compares with the record when a token is
    /// renewed.
    async fn get_user(&self, id: UserId) -> AuthResult<Option<User>> {
        Ok(Self::found(self.store.get(&Self::context(), id).await)?.map(|user| self.configured(user)))
    }

    /// Whoever `username` is, without any credential: the user stored under
    /// it, or a new one, with the roles the configuration gives them. Stored
    /// - or changed - by the `UserManager` (issue 306), not here.
    async fn authenticate(&self, username: &str, _credential: &Credential<'_>) -> AuthResult<Option<User>> {
        let Ok(username) = Name::new(username.trim()) else {
            return Err(AuthError::InvalidCredentials);
        };
        if let Some(user) = self.find_user(username.as_str()).await? {
            return Ok(Some(self.configured(user)));
        }
        let user = User { id: UserId::new(), username, email: String::new(), roles: Vec::new(), deleted: None };
        tracing::warn!("AcceptAnyUserProvider accepted the new user '{}' ({}) without verifying any credential", user.username, user.id);
        Ok(Some(self.configured(user)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;
    use crate::user_provider::UserManager;

    /// The provider in a chain of its own, whose manager keeps the records
    /// in `store`.
    fn chain(store: &Arc<dyn EntityStore<User>>, provider: AcceptAnyUserProvider) -> UserManager {
        UserManager::new(vec![Arc::new(provider)], Arc::clone(store), Arc::new(()))
    }

    fn store() -> Arc<dyn EntityStore<User>> {
        Arc::new(MemoryStore::new()).entity_stores().users
    }

    #[tokio::test]
    async fn accepts_any_password_and_keeps_the_id_stable() {
        let store = store();
        let users = chain(&store, AcceptAnyUserProvider::new(Arc::clone(&store)));

        let first = users.authenticate("alice", &Credential::Password("one")).await.expect("accepted");
        let second = users.authenticate("alice", &Credential::Password("two")).await.expect("accepted");
        let bob = users.authenticate("bob", &Credential::Password("")).await.expect("accepted");

        assert_eq!(first.id, second.id, "the same name must map to the same id");
        assert_ne!(first.id, bob.id, "different names must map to different ids");
    }

    #[tokio::test]
    async fn writes_nothing_itself() {
        let store = store();
        let provider = AcceptAnyUserProvider::new(Arc::clone(&store));
        let alice = provider.authenticate("alice", &Credential::Password("x")).await.unwrap().expect("accepted");
        assert!(provider.find_user("alice").await.unwrap().is_none(), "the manager stores her, not the provider");
        assert!(provider.get_user(alice.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn lookups_only_know_users_that_logged_in() {
        let store = store();
        let provider = AcceptAnyUserProvider::new(Arc::clone(&store));
        assert!(provider.find_user("alice").await.unwrap().is_none());

        let alice = chain(&store, AcceptAnyUserProvider::new(Arc::clone(&store))).authenticate("alice", &Credential::Password("x")).await.expect("accepted");

        assert_eq!(provider.find_user("alice").await.unwrap().map(|u| u.id), Some(alice.id));
        assert_eq!(provider.get_user(alice.id).await.unwrap().map(|u| u.username), Some(Name::new("alice").unwrap()));
        assert!(provider.get_user(UserId::new()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn gives_the_configured_roles_and_follows_a_change() {
        let store = store();
        let (hr, staff) = (RoleId::new(), RoleId::new());
        let roles = BTreeMap::from([("alice".to_string(), vec![hr])]);
        let users = chain(&store, AcceptAnyUserProvider::with_roles(Arc::clone(&store), roles, vec![staff]));
        let alice = users.authenticate("alice", &Credential::Password("x")).await.unwrap();
        let bob = users.authenticate("bob", &Credential::Password("x")).await.unwrap();
        assert_eq!((alice.roles, bob.roles), (vec![hr], vec![staff]));
        let shouted = users.authenticate("ALICE", &Credential::Password("x")).await.unwrap();
        assert_eq!((shouted.id, shouted.roles), (alice.id, vec![hr]), "a username is compared as a name");

        let changed = chain(&store, AcceptAnyUserProvider::with_roles(Arc::clone(&store), BTreeMap::new(), Vec::new()));
        let again = changed.get_user(alice.id).await.unwrap().unwrap();
        assert!(again.roles.is_empty(), "the configuration decides, also for a user already stored, on a renewal too");
        assert_eq!(store.get(&SecurityContext::system(), alice.id).await.unwrap().roles, Vec::<RoleId>::new(), "and it is stored");
    }

    #[tokio::test]
    async fn rejects_an_empty_username() {
        let result = AcceptAnyUserProvider::new(store()).authenticate("  ", &Credential::Password("x")).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn finds_users_created_before_it_existed() {
        let store = store();
        let existing = User {
            id: UserId::new(),
            username: Name::new("user").unwrap(),
            email: "user@example.com".to_string(),
            roles: vec![RoleId::new()],
            deleted: None,
        };
        store.create(&SecurityContext::default(), &existing).await.unwrap();

        let provider = AcceptAnyUserProvider::new(store);
        let user = provider.authenticate("user", &Credential::Password("x")).await.unwrap().expect("accepted");

        assert_eq!(user.id, existing.id, "a user already in the store keeps its id");
    }
}
