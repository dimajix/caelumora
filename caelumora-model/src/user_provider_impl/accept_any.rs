use crate::security::SecurityContext;
use crate::store::{EntityStore, StoreError, StoreResultExt};
use crate::user::{User, UserId};
use crate::user_provider::{AuthError, AuthResult, Credential, UserProvider};
use crate::value::name::Name;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;

/// **Development only.** Accepts every username with any credential.
///
/// A stand-in until a real backend exists (issue 25), so that login and
/// sessions (issue 10) can be built against the `UserProvider` trait. It must
/// be enabled explicitly in the configuration and is never part of a default
/// chain; `AppConfig::create_user_manager` warns loudly when it is.
///
/// Users are kept in the `EntityStore`: the first login under a name creates
/// the user with a random id, later ones find it again. Ids are therefore as
/// stable as the store — across restarts with `JsonFileStore`, only for the
/// lifetime of the process with `MemoryStore`.
///
/// Find-or-create is not atomic: of two concurrent *first* logins under the
/// same name, the store lets one create the user and refuses the other its
/// name (issue 250), whose login then fails. Accepted for a development
/// stand-in.
#[derive(Debug)]
pub struct AcceptAnyUserProvider {
    store: Arc<dyn EntityStore>,
    /// The roles of the users named (issue 142).
    roles: BTreeMap<String, Vec<String>>,
    /// The roles of everybody else.
    default_roles: Vec<String>,
}

impl AcceptAnyUserProvider {
    /// A provider that gives nobody a role.
    pub fn new(store: Arc<dyn EntityStore>) -> Self {
        Self::with_roles(store, BTreeMap::new(), Vec::new())
    }

    /// A provider that gives the users named in `roles` theirs, and
    /// everybody else `default_roles`.
    pub fn with_roles(store: Arc<dyn EntityStore>, roles: BTreeMap<String, Vec<String>>, default_roles: Vec<String>) -> Self {
        Self { store, roles, default_roles }
    }

    /// The roles of `username`, as configured - named as names are
    /// compared (issue 250).
    fn roles_of(&self, username: &Name) -> Vec<String> {
        self.roles.iter()
            .find(|(configured, _)| *username == configured.as_str())
            .map_or_else(|| self.default_roles.clone(), |(_, roles)| roles.clone())
    }

    /// The provider keeps its own records: nobody's action, the system's.
    fn context() -> SecurityContext {
        SecurityContext::system()
    }

    /// The user `result` found, `None` if there is none.
    fn found(result: Result<User, StoreError>) -> AuthResult<Option<User>> {
        result.optional().map_err(|e| AuthError::Backend(e.to_string()))
    }
}

#[async_trait]
impl UserProvider for AcceptAnyUserProvider {
    async fn find_user(&self, username: &str) -> AuthResult<Option<User>> {
        Self::found(self.store.find_user_by_name(&Self::context(), username).await)
    }

    async fn get_user(&self, id: UserId) -> AuthResult<Option<User>> {
        Self::found(self.store.get_user(&Self::context(), id).await)
    }

    async fn authenticate(&self, username: &str, _credential: &Credential<'_>) -> AuthResult<Option<User>> {
        let Ok(username) = Name::new(username.trim()) else {
            return Err(AuthError::InvalidCredentials);
        };

        let roles = self.roles_of(&username);
        if let Some(user) = self.find_user(username.as_str()).await? {
            // The configuration decides the roles; a changed one takes effect
            // at the next login.
            if user.roles == roles {
                return Ok(Some(user));
            }
            let user = User { roles, ..user };
            self.store.update_user(&Self::context(), &user).await
                .map_err(|e| AuthError::Backend(e.to_string()))?;
            return Ok(Some(user));
        }

        let id = UserId::new();
        let user = User {
            id,
            username,
            email: String::new(),
            roles,
            deleted: None,
        };
        self.store.create_user(&Self::context(), &user).await
            .map_err(|e| AuthError::Backend(e.to_string()))?;
        tracing::warn!("AcceptAnyUserProvider created user '{}' ({}) without verifying any credential", user.username, user.id);
        Ok(Some(user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store_impl::memory::MemoryStore;

    fn provider() -> AcceptAnyUserProvider {
        AcceptAnyUserProvider::new(Arc::new(MemoryStore::new()))
    }

    #[tokio::test]
    async fn accepts_any_password_and_keeps_the_id_stable() {
        let provider = provider();

        let first = provider.authenticate("alice", &Credential::Password("one")).await.unwrap().expect("accepted");
        let second = provider.authenticate("alice", &Credential::Password("two")).await.unwrap().expect("accepted");
        let bob = provider.authenticate("bob", &Credential::Password("")).await.unwrap().expect("accepted");

        assert_eq!(first.id, second.id, "the same name must map to the same id");
        assert_ne!(first.id, bob.id, "different names must map to different ids");
    }

    #[tokio::test]
    async fn lookups_only_know_users_that_logged_in() {
        let provider = provider();
        assert!(provider.find_user("alice").await.unwrap().is_none());

        let alice = provider.authenticate("alice", &Credential::Password("x")).await.unwrap().expect("accepted");

        assert_eq!(provider.find_user("alice").await.unwrap().map(|u| u.id), Some(alice.id));
        assert_eq!(provider.get_user(alice.id).await.unwrap().map(|u| u.username), Some(Name::new("alice").unwrap()));
        assert!(provider.get_user(UserId::new()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn gives_the_configured_roles_and_follows_a_change() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let roles = BTreeMap::from([("alice".to_string(), vec!["hr".to_string()])]);
        let provider = AcceptAnyUserProvider::with_roles(Arc::clone(&store), roles, vec!["staff".to_string()]);
        let alice = provider.authenticate("alice", &Credential::Password("x")).await.unwrap().unwrap();
        let bob = provider.authenticate("bob", &Credential::Password("x")).await.unwrap().unwrap();
        assert_eq!((alice.roles, bob.roles), (vec!["hr".to_string()], vec!["staff".to_string()]));
        let shouted = provider.authenticate("ALICE", &Credential::Password("x")).await.unwrap().unwrap();
        assert_eq!((shouted.id, shouted.roles), (alice.id, vec!["hr".to_string()]), "a username is compared as a name");

        let changed = AcceptAnyUserProvider::with_roles(store, BTreeMap::new(), Vec::new());
        let again = changed.authenticate("alice", &Credential::Password("x")).await.unwrap().unwrap();
        assert!(again.roles.is_empty(), "the configuration decides, also for a user already stored");
        assert_eq!(changed.get_user(again.id).await.unwrap().unwrap().roles, Vec::<String>::new(), "and it is stored");
    }

    #[tokio::test]
    async fn rejects_an_empty_username() {
        let result = provider().authenticate("  ", &Credential::Password("x")).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn finds_users_created_before_it_existed() {
        let store: Arc<dyn EntityStore> = Arc::new(MemoryStore::new());
        let existing = User {
            id: UserId::new(),
            username: Name::new("user").unwrap(),
            email: "user@example.com".to_string(),
            roles: vec!["user".to_string()],
            deleted: None,
        };
        store.create_user(&SecurityContext::default(), &existing).await.unwrap();

        let provider = AcceptAnyUserProvider::new(store);
        let user = provider.authenticate("user", &Credential::Password("x")).await.unwrap().expect("accepted");

        assert_eq!(user.id, existing.id, "a user already in the store keeps its id");
    }
}
