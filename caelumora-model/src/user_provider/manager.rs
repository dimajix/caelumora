//! `UserManager`: the ordered chain of `UserProvider`s, and the one writer
//! of user records (issue 306).

use super::{AuthError, AuthResult, Credential, UserProvider};
use crate::event::{Announcing, EventPublisher, GlobalEvent};
use crate::security::SecurityContext;
use crate::store::{EntityStore, StoreError, StoreResultExt};
use crate::user::{User, UserId, UserUpdateEvent};
use std::fmt::Debug;
use std::sync::Arc;

/// An ordered chain of `UserProvider`s; for each question, the first provider that knows the user
/// answers it.
///
/// Unlike `SecretManager`, lookups are not gated through the `AccessPolicy`: authentication is what
/// produces the `SecurityContext` such a check would need.
///
/// It is the one writer of user records (issue 306), which belong to the system (issue 94): what a
/// provider answers at a login or on a renewal is compared with the record stored, stored in the
/// system's name where it differs, and announced - a new user as created, a changed name or changed
/// roles as a `UserUpdateEvent` -, so that every `UserController`, in every instance, holds them as
/// they are. A user whose record was deleted is shut out (issue 500).
#[derive(Debug)]
pub struct UserManager {
    providers: Vec<Arc<dyn UserProvider>>,
    store: Arc<dyn EntityStore<User>>,
    events: Arc<dyn EventPublisher>,
}

impl UserManager {
    pub fn new(
        providers: Vec<Arc<dyn UserProvider>>,
        store: Arc<dyn EntityStore<User>>,
        events: Arc<dyn EventPublisher>,
    ) -> Self {
        Self { providers, store, events }
    }

    /// Whether the chain has no providers, i.e. nobody can authenticate.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Authenticates `username` with `credential` against the chain.
    ///
    /// # Errors
    /// `AuthError::InvalidCredentials` if the credential is wrong, no provider knows the user or
    /// their record was deleted; `AuthError::Backend` if a provider or the store failed.
    pub async fn authenticate(
        &self,
        username: &str,
        credential: &Credential<'_>,
    ) -> AuthResult<User> {
        for provider in &self.providers {
            match provider.authenticate(username, credential).await {
                Ok(Some(user)) => {
                    return self.record(user).await?.ok_or_else(|| {
                        tracing::info!(
                            "Authentication of '{}' failed: the user was deleted",
                            username
                        );
                        AuthError::InvalidCredentials
                    });
                }
                Ok(None) => continue,
                Err(AuthError::InvalidCredentials) => {
                    tracing::info!(
                        "Authentication of '{}' failed: invalid {}",
                        username,
                        credential.kind()
                    );
                    return Err(AuthError::InvalidCredentials);
                }
                Err(e) => {
                    tracing::error!(
                        "User provider {:?} failed while authenticating '{}': {}",
                        provider,
                        username,
                        e
                    );
                    return Err(e);
                }
            }
        }
        tracing::info!("Authentication of '{}' failed: no provider knows the user", username);
        Err(AuthError::InvalidCredentials)
    }

    /// Looks up a user by id, as when a token is renewed, and keeps their record as the provider
    /// answers it - so that roles given or taken meanwhile take effect. `Ok(None)` if no provider
    /// knows it, or its record was deleted.
    ///
    /// # Errors
    /// `AuthError::Backend` if a provider or the store failed.
    pub async fn get_user(&self, id: UserId) -> AuthResult<Option<User>> {
        for provider in &self.providers {
            if let Some(user) = provider.get_user(id).await? {
                return self.record(user).await;
            }
        }
        Ok(None)
    }

    /// Keeps `found`, a user as a provider answers them, as their record: made if there is none,
    /// changed where it differs, and announced. Written or made meanwhile elsewhere, the record is
    /// read again and changed once more (issues 492, 500). Answers the record as it now stands,
    /// `None` if it was deleted: a deleted user is shut out, and their record left as it is.
    async fn record(&self, mut found: User) -> AuthResult<Option<User>> {
        let system = SecurityContext::system();
        let backend = |e: StoreError| AuthError::Backend(e.to_string());
        let (user, renamed, roles_changed) = loop {
            let Some(stored) =
                self.store.get(&system, found.id).await.optional().map_err(backend)?
            else {
                let deleted =
                    self.store.get_deleted(&system, found.id).await.optional().map_err(backend)?;
                if deleted.is_some() {
                    return Ok(None);
                }
                if self
                    .store
                    .create(&system, &mut found)
                    .await
                    .created()
                    .map_err(backend)?
                    .is_none()
                {
                    continue;
                }
                self.events.announce(GlobalEvent::created::<User>(&found.id), &found);
                return Ok(Some(found));
            };
            let renamed = found.username != stored.username;
            let roles_changed = found.roles != stored.roles;
            if !renamed && !roles_changed && found.email == stored.email {
                return Ok(Some(stored));
            }
            let mut user = User {
                username: found.username.clone(),
                email: found.email.clone(),
                roles: found.roles.clone(),
                ..stored
            };
            if self.store.update(&system, &mut user).await.written().map_err(backend)?.is_some() {
                break (user, renamed, roles_changed);
            }
        };
        if renamed {
            self.events.announce(
                GlobalEvent::updated::<User>(
                    &user.id,
                    &UserUpdateEvent::NameChanged(user.username.to_string()),
                ),
                &user,
            );
        }
        if roles_changed {
            tracing::info!(user = %user.id, "The roles of the user changed");
            self.events.announce(
                GlobalEvent::updated::<User>(
                    &user.id,
                    &UserUpdateEvent::RolesChanged(user.roles.clone()),
                ),
                &user,
            );
        }
        Ok(Some(user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::confidentiality::Classification;
    use crate::event::EntityChange;
    use crate::removal::Removal;
    use crate::role::RoleId;
    use crate::security::SecurityAttributes;
    use crate::store::testing::ContendedStore;
    use crate::store::{EntityStores, Persistence, Policies, Stored};
    use crate::user::testing::make_user;
    use crate::value::name::Name;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::mem;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    /// What was announced.
    #[derive(Debug, Default)]
    struct Announced(Mutex<Vec<GlobalEvent>>);

    impl EventPublisher for Announced {
        fn publish(&self, event: GlobalEvent, _attributes: SecurityAttributes) {
            self.0.lock().unwrap().push(event);
        }

        fn publish_classified(
            &self,
            event: GlobalEvent,
            attributes: SecurityAttributes,
            _classifications: &[Classification],
        ) {
            self.publish(event, attributes);
        }
    }

    impl Announced {
        fn taken(&self) -> Vec<GlobalEvent> {
            mem::take(&mut *self.0.lock().unwrap())
        }
    }

    /// A manager of `providers`, keeping records in a store of its own.
    fn chain(providers: Vec<Arc<dyn UserProvider>>) -> UserManager {
        UserManager::new(
            providers,
            EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
                .unwrap()
                .users,
            Arc::new(Announced::default()),
        )
    }

    /// Knows a fixed set of users with plaintext passwords - fine for a test.
    #[derive(Debug)]
    struct MapUserProvider {
        users: HashMap<String, (User, String)>,
    }

    impl MapUserProvider {
        fn new(entries: &[(&User, &str)]) -> Self {
            let users = entries
                .iter()
                .map(|(u, p)| (u.username.to_string(), ((*u).clone(), p.to_string())))
                .collect();
            Self { users }
        }
    }

    #[async_trait]
    impl UserProvider for MapUserProvider {
        async fn find_user(&self, username: &str) -> AuthResult<Option<User>> {
            Ok(self.users.get(username).map(|(u, _)| u.clone()))
        }
        async fn get_user(&self, id: UserId) -> AuthResult<Option<User>> {
            Ok(self.users.values().find(|(u, _)| u.id == id).map(|(u, _)| u.clone()))
        }
        async fn authenticate(
            &self,
            username: &str,
            credential: &Credential<'_>,
        ) -> AuthResult<Option<User>> {
            match (self.users.get(username), credential) {
                (None, _) => Ok(None),
                (Some((u, p)), Credential::Password(password)) if p == password => {
                    Ok(Some(u.clone()))
                }
                (Some((u, _)), Credential::Trusted) => Ok(Some(u.clone())),
                (Some(_), Credential::Password(_)) => Err(AuthError::InvalidCredentials),
            }
        }
    }

    #[derive(Debug)]
    struct FailingUserProvider;

    #[async_trait]
    impl UserProvider for FailingUserProvider {
        async fn find_user(&self, _username: &str) -> AuthResult<Option<User>> {
            Err(AuthError::Backend("boom".to_string()))
        }
        async fn get_user(&self, _id: UserId) -> AuthResult<Option<User>> {
            Err(AuthError::Backend("boom".to_string()))
        }
        async fn authenticate(
            &self,
            _username: &str,
            _credential: &Credential<'_>,
        ) -> AuthResult<Option<User>> {
            Err(AuthError::Backend("boom".to_string()))
        }
    }

    /// Issue 492: a record written meanwhile is read again and changed once more.
    #[tokio::test]
    async fn a_record_is_changed_once_more_when_it_was_written_meanwhile() {
        let (stores, refusals) = ContendedStore::stores(User::KIND);
        let users = EntityStores::open(&stores, &Policies::permitting_all()).unwrap().users;
        let announced = Arc::new(Announced::default());
        let alice = make_user("alice");
        let manager = UserManager::new(
            vec![Arc::new(MapUserProvider::new(&[(&alice, "a")]))],
            Arc::clone(&users),
            Arc::<Announced>::clone(&announced),
        );
        manager.get_user(alice.id).await.unwrap().expect("recorded");
        let renamed = User { username: Name::new("alicia").unwrap(), ..alice.clone() };
        let manager = UserManager::new(
            vec![Arc::new(MapUserProvider::new(&[(&renamed, "a")]))],
            users,
            Arc::<Announced>::clone(&announced),
        );

        refusals.store(1, Ordering::SeqCst);
        let recorded = manager.get_user(alice.id).await.unwrap().expect("known");
        assert_eq!(recorded.username, "alicia");
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "refused once");
        assert_eq!(recorded.revision().number(), 2, "created, and written once more");
    }

    /// Issue 500: a record made meanwhile elsewhere is read again, and kept as it is found.
    #[tokio::test]
    async fn a_record_made_meanwhile_is_read_again() {
        let (stores, refusals) = ContendedStore::stores(User::KIND);
        let users = EntityStores::open(&stores, &Policies::permitting_all()).unwrap().users;
        let announced = Arc::new(Announced::default());
        let alice = make_user("alice");
        let manager = UserManager::new(
            vec![Arc::new(MapUserProvider::new(&[(&alice, "a")]))],
            Arc::clone(&users),
            Arc::<Announced>::clone(&announced),
        );

        refusals.store(1, Ordering::SeqCst);
        let recorded = manager.get_user(alice.id).await.unwrap().expect("recorded all the same");
        assert_eq!(refusals.load(Ordering::SeqCst), 0, "refused once");
        assert_eq!(recorded.username, "alice");
        assert!(announced.taken().is_empty(), "made and announced by the other writer");
    }

    /// Issue 500: a user whose record was deleted is shut out, as one no provider knows, and their
    /// record left as it is.
    #[tokio::test]
    async fn a_user_whose_record_was_deleted_is_shut_out() {
        let users = EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .users;
        let announced = Arc::new(Announced::default());
        let alice = make_user("alice");
        let manager = UserManager::new(
            vec![Arc::new(MapUserProvider::new(&[(&alice, "a")]))],
            Arc::clone(&users),
            Arc::<Announced>::clone(&announced),
        );
        manager.get_user(alice.id).await.unwrap().expect("recorded");
        let system = SecurityContext::system();
        assert!(users.delete(&system, alice.id, &Removal::now(ActorId::System)).await.unwrap());
        announced.taken();

        let refused = manager.authenticate("alice", &Credential::Password("a")).await;
        assert!(matches!(refused, Err(AuthError::InvalidCredentials)), "{refused:?}");
        assert!(manager.get_user(alice.id).await.unwrap().is_none(), "nor renewed");
        assert!(users.get_deleted(&system, alice.id).await.is_ok(), "left deleted");
        assert!(announced.taken().is_empty(), "nothing made");
    }

    #[tokio::test]
    async fn authenticate_falls_through_to_the_provider_that_knows_the_user() {
        let alice = make_user("alice");
        let bob = make_user("bob");
        let manager = chain(vec![
            Arc::new(MapUserProvider::new(&[(&alice, "a")])),
            Arc::new(MapUserProvider::new(&[(&bob, "b")])),
        ]);

        let result = manager
            .authenticate("bob", &Credential::Password("b"))
            .await
            .expect("bob authenticates");
        assert_eq!(result.id, bob.id);
    }

    #[tokio::test]
    async fn wrong_credential_ends_the_chain() {
        let first = make_user("alice");
        let second = make_user("alice");
        let manager = chain(vec![
            Arc::new(MapUserProvider::new(&[(&first, "a")])),
            Arc::new(MapUserProvider::new(&[(&second, "other")])),
        ]);

        let result = manager.authenticate("alice", &Credential::Password("other")).await;
        assert!(
            matches!(result, Err(AuthError::InvalidCredentials)),
            "a later provider must not override a rejection"
        );
    }

    #[tokio::test]
    async fn a_trusted_frontend_is_answered_by_the_first_provider_that_knows_the_user() {
        let alice = make_user("alice");
        let other_alice = make_user("alice");
        let bob = make_user("bob");
        let manager = chain(vec![
            Arc::new(MapUserProvider::new(&[(&alice, "a")])),
            Arc::new(MapUserProvider::new(&[(&other_alice, "a"), (&bob, "b")])),
        ]);

        let found = manager.authenticate("alice", &Credential::Trusted).await.expect("known");
        assert_eq!(found.id, alice.id, "not passed on past the provider that knows her");
        assert_eq!(
            manager.authenticate("bob", &Credential::Trusted).await.expect("known").id,
            bob.id
        );
        assert!(
            matches!(
                manager.authenticate("mallory", &Credential::Trusted).await,
                Err(AuthError::InvalidCredentials)
            ),
            "nobody knows him"
        );
    }

    #[tokio::test]
    async fn unknown_user_is_indistinguishable_from_a_wrong_password() {
        let alice = make_user("alice");
        let manager = chain(vec![Arc::new(MapUserProvider::new(&[(&alice, "a")]))]);

        let result = manager.authenticate("mallory", &Credential::Password("x")).await;
        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn failing_provider_denies_instead_of_falling_through() {
        let alice = make_user("alice");
        let manager = chain(vec![
            Arc::new(FailingUserProvider),
            Arc::new(MapUserProvider::new(&[(&alice, "a")])),
        ]);

        assert!(matches!(
            manager.authenticate("alice", &Credential::Password("a")).await,
            Err(AuthError::Backend(_))
        ));
        assert!(matches!(manager.get_user(alice.id).await, Err(AuthError::Backend(_))));
    }

    #[tokio::test]
    async fn lookups_return_the_first_match_or_none() {
        let alice = make_user("alice");
        let manager = chain(vec![
            Arc::new(MapUserProvider::new(&[])),
            Arc::new(MapUserProvider::new(&[(&alice, "a")])),
        ]);

        assert_eq!(
            manager.get_user(alice.id).await.unwrap().map(|u| u.username),
            Some(Name::new("alice").unwrap())
        );
        assert!(manager.get_user(UserId::new()).await.unwrap().is_none());
        assert!(chain(vec![]).get_user(alice.id).await.unwrap().is_none());
    }

    /// Issue 306: the manager keeps the records - a new user made and
    /// announced, roles given or taken at a login or on a renewal stored and
    /// announced, nothing written where nothing changed.
    #[tokio::test]
    async fn the_manager_keeps_and_announces_the_records_the_providers_answer() {
        let store = EntityStores::open(&Persistence::in_memory(), &Policies::permitting_all())
            .unwrap()
            .users;
        let announced = Arc::new(Announced::default());
        // A manager whose one provider knows `user` with the password "a".
        let answering = |user: &User| {
            UserManager::new(
                vec![Arc::new(MapUserProvider::new(&[(user, "a")]))],
                Arc::clone(&store),
                Arc::<Announced>::clone(&announced),
            )
        };
        let staff = RoleId::new();
        let alice = make_user("alice");
        answering(&alice).authenticate("alice", &Credential::Password("a")).await.unwrap();
        assert!(
            matches!(&announced.taken()[..], [event] if event.about::<User>() == Some((alice.id, &EntityChange::Created))),
            "a new user is made"
        );
        answering(&alice).authenticate("alice", &Credential::Password("a")).await.unwrap();
        assert!(announced.taken().is_empty(), "nothing changed, nothing announced");

        let found = answering(&User { roles: vec![staff], ..alice.clone() })
            .get_user(alice.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.roles, [staff], "on a renewal, as the provider answers");
        assert!(
            matches!(&announced.taken()[..], [event] if matches!(event.about::<User>(), Some((id, change)) if id == alice.id && matches!(change.update::<UserUpdateEvent>(), Some(UserUpdateEvent::RolesChanged(roles)) if roles == [staff])))
        );
        let stored = store.get(&SecurityContext::system(), alice.id).await.unwrap();
        assert_eq!(stored.roles, [staff], "and stored");

        answering(&alice).authenticate("alice", &Credential::Password("a")).await.unwrap();
        assert!(
            matches!(&announced.taken()[..], [event] if matches!(event.about::<User>().and_then(|(_, change)| change.update::<UserUpdateEvent>()), Some(UserUpdateEvent::RolesChanged(roles)) if roles.is_empty())),
            "taken at a login"
        );
    }
}
