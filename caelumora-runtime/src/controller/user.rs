use arc_swap::ArcSwap;
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::value::language::Language;
use caelumora_model::security::{Secured, SecurityContext};
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use caelumora_model::user::{User, UserId, UserProfile, UserUpdateEvent};
use std::sync::Arc;

/// The live user of this instance, shared by all the user's actors in all
/// sessions (issue 121), with their profile. The user record is not changed
/// through it - it belongs to the system (issue 94) and is only kept current
/// when a change is announced - but the profile is: the controller is its
/// one writer in this instance (issue 13).
#[derive(Debug)]
pub struct UserController {
    id: UserId,
    user: ArcSwap<User>,
    /// What the user says about themselves; an empty one of theirs while
    /// they never wrote one.
    profile: ArcSwap<UserProfile>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
    /// Held while the profile is changed and stored, like the other
    /// controllers.
    write_lock: tokio::sync::Mutex<()>,
}

impl UserController {
    /// The live `user` with their `profile`, `None` if they have none yet.
    /// Made by the `ControllerManager`, which keeps one per user.
    pub(crate) fn new(user: User, profile: Option<UserProfile>, event_publisher: Arc<dyn EventPublisher>, entity_store: Arc<dyn EntityStore>) -> Self {
        let id = user.id;
        Self {
            id,
            user: ArcSwap::from_pointee(user),
            profile: ArcSwap::from_pointee(profile.unwrap_or_else(|| UserProfile::new(id, None))),
            event_publisher,
            entity_store,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<User>> {
        self.user.load()
    }

    /// The user's profile as it stands. Handed out as the runtime holds it:
    /// whoever acts with it in someone's name checks that themselves.
    pub fn profile(&self) -> arc_swap::Guard<Arc<UserProfile>> {
        self.profile.load()
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn username(&self) -> String {
        self.user.load().username.clone()
    }

    /// Sets how the user introduces themselves and the language they prefer
    /// (issues 176, 56), on behalf of `ctx`; the rest of the profile stays.
    /// Stored first - which is where the permission is checked - and only
    /// then changed live and announced. Answers the profile as it now
    /// stands.
    ///
    /// # Errors
    /// `StoreError::Invalid` for an introduction or a language that does
    /// not fit, otherwise whatever the store reports; the profile is then
    /// unchanged.
    pub async fn update_profile(&self, ctx: &SecurityContext, introduction: Option<&str>, preferred_language: Option<&str>) -> StoreResult<UserProfile> {
        self.change(ctx, |profile| {
            profile.introduce(introduction)?;
            profile.preferred_language = Language::optional(preferred_language)?;
            Ok(())
        }).await.map(|(_, profile)| profile)
    }

    /// Adds `rule` to the user's answers to tool calls given in advance
    /// (issue 13), on behalf of `ctx`, as `update_profile` changes the
    /// profile. They answer from the next request on, in every channel of
    /// the user's in this instance.
    ///
    /// # Errors
    /// `StoreError::Validation` for a rule a person may not keep, or one too
    /// many (`UserProfile::add_approval_rule`), otherwise whatever the store
    /// reports; the profile is then unchanged.
    pub async fn add_approval_rule(&self, ctx: &SecurityContext, rule: ApprovalRule) -> StoreResult<()> {
        self.change(ctx, |profile| profile.add_approval_rule(rule).map_err(|e| StoreError::Validation(e.to_string()))).await.map(|_| ())
    }

    /// Takes the user's rule `id` back (issue 13), on behalf of `ctx`, as
    /// `update_profile` changes the profile.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the user has no such rule, otherwise
    /// whatever the store reports; the profile is then unchanged.
    pub async fn remove_approval_rule(&self, ctx: &SecurityContext, id: ApprovalRuleId) -> StoreResult<()> {
        self.change(ctx, |profile| match profile.remove_approval_rule(id) {
            true => Ok(()),
            false => Err(StoreError::NotFound { entity: "approval_rule", id: id.to_string() }),
        }).await.map(|_| ())
    }

    /// Changes the profile as `change` says, on behalf of `ctx`: stored
    /// first - which is where the permission is checked - and only then
    /// taken over live and announced, as it now stands. Rules that expired
    /// are dropped with every change.
    ///
    /// # Errors
    /// What `change` reports, or the store; the profile is then unchanged.
    async fn change<T>(&self, ctx: &SecurityContext, change: impl FnOnce(&mut UserProfile) -> StoreResult<T>) -> StoreResult<(T, UserProfile)> {
        let _guard = self.write_lock.lock().await;
        let mut profile = (**self.profile.load()).clone();
        let changed = change(&mut profile)?;
        profile.prune_approval_rules(chrono::Utc::now());
        self.save(ctx, &profile).await?;
        self.profile.store(Arc::new(profile.clone()));
        let attributes = profile.security_attributes().clone();
        self.event_publisher.publish(GlobalEvent::UserProfileUpdated(self.id, profile.clone()), attributes);
        Ok((changed, profile))
    }

    /// Stores `profile` as `ctx`, whether the user has one already or not:
    /// changed if there is one, else created - and changed after all if one
    /// was created meanwhile, in another instance. The store asks whether
    /// `ctx` may.
    async fn save(&self, ctx: &SecurityContext, profile: &UserProfile) -> StoreResult<()> {
        match self.entity_store.update_user_profile(ctx, profile).await {
            Err(StoreError::NotFound { .. }) => match self.entity_store.create_user_profile(ctx, profile).await {
                Err(StoreError::Conflict(_)) => self.entity_store.update_user_profile(ctx, profile).await,
                created => created,
            },
            updated => updated,
        }
    }

    fn process_user_update(&self, event: &UserUpdateEvent) {
        match event {
            UserUpdateEvent::NameChanged(name) => {
                let mut user = (**self.user.load()).clone();
                user.username = name.clone();
                self.user.store(Arc::new(user));
            }
        }
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced. An event carries the resulting value, so taking over
    /// this controller's own change again changes nothing.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        match event {
            GlobalEvent::UserUpdated(uid, update) if *uid == self.id => {
                self.process_user_update(update);
            }
            GlobalEvent::UserProfileUpdated(uid, profile) if *uid == self.id => {
                self.profile.store(Arc::new(profile.clone()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::value::text::Introduction;
    use caelumora_model::actor::ActorId;
    use caelumora_model::store_impl::memory::MemoryStore;
    use caelumora_model::security::OwnershipSecurityManager;
    use std::sync::Mutex;

    /// What was announced, in order.
    #[derive(Debug, Default)]
    struct Announced(Mutex<Vec<GlobalEvent>>);

    impl EventPublisher for Announced {
        fn publish(&self, event: GlobalEvent, _attributes: caelumora_model::security::SecurityAttributes) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn alice() -> User {
        User { id: UserId::new(), username: "alice".to_string(), email: String::new(), roles: Vec::new(), deleted: None }
    }

    fn as_user(id: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
    }

    #[tokio::test]
    async fn a_profile_is_stored_first_then_taken_over_and_announced() {
        let store = Arc::new(MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new())));
        let announced = Arc::new(Announced::default());
        let user = alice();
        let controller = UserController::new(user.clone(), None, Arc::<Announced>::clone(&announced), Arc::<MemoryStore>::clone(&store));
        let hers = as_user(user.id);

        for text in ["I run the budget.", "I run the budget and the team."] {
            let profile = controller.update_profile(&hers, Some(text), None).await.expect("her own, created once and changed after");
            assert_eq!(profile.introduction.as_ref().map(Introduction::as_str), Some(text));
            assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some(text), "live");
            assert_eq!(store.get_user_profile(&hers, user.id).await.unwrap().introduction.as_ref().map(Introduction::as_str), Some(text), "stored");
        }
        assert!(matches!(announced.0.lock().unwrap().as_slice(), [GlobalEvent::UserProfileUpdated(..), GlobalEvent::UserProfileUpdated(id, p)] if *id == user.id && p.introduction.as_ref().map(Introduction::as_str) == Some("I run the budget and the team.")));
    }

    #[tokio::test]
    async fn a_refused_profile_changes_nothing() {
        let store = Arc::new(MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new())));
        let announced = Arc::new(Announced::default());
        let user = alice();
        let controller = UserController::new(user.clone(), None, Arc::<Announced>::clone(&announced), Arc::<MemoryStore>::clone(&store));

        let invalid = controller.update_profile(&as_user(user.id), Some("a\u{1b}[2Jb"), None).await;
        assert!(matches!(invalid, Err(StoreError::Invalid(_))), "{invalid:?}");
        let invalid = controller.update_profile(&as_user(user.id), None, Some("Deutsch")).await;
        assert!(matches!(invalid, Err(StoreError::Invalid(_))), "a language, too: {invalid:?}");

        let forged = controller.update_profile(&as_user(UserId::new()), Some("I am alice."), None).await;
        assert!(matches!(forged, Err(StoreError::PermissionDenied)), "not by someone else: {forged:?}");

        assert_eq!(controller.profile().introduction, None);
        assert!(announced.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_rule_is_added_and_taken_back_by_the_user_alone() {
        use caelumora_model::approval::{Reach, RuleDecision, ToolMatcher};
        let store = Arc::new(MemoryStore::with_security_manager(Arc::new(OwnershipSecurityManager::new())));
        let user = alice();
        let controller = UserController::new(user.clone(), None, Arc::new(()), Arc::<MemoryStore>::clone(&store));
        let hers = as_user(user.id);
        controller.update_profile(&hers, Some("I run the budget."), None).await.unwrap();
        let rule = ApprovalRule::new(ToolMatcher { tool: "shell".to_string(), arguments: Default::default() }, RuleDecision::Allow, Reach::Everywhere, None).unwrap();

        assert!(matches!(controller.add_approval_rule(&as_user(UserId::new()), rule.clone()).await, Err(StoreError::PermissionDenied)), "not by someone else");
        controller.add_approval_rule(&hers, rule.clone()).await.expect("hers");
        assert_eq!(controller.profile().approval_rules(), [rule.clone()], "live");
        assert_eq!(store.get_user_profile(&hers, user.id).await.unwrap().approval_rules(), [rule.clone()], "stored");
        assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some("I run the budget."), "the rest stays");
        controller.update_profile(&hers, None, None).await.unwrap();
        assert_eq!(controller.profile().approval_rules(), [rule.clone()], "and stays through a change of the rest");

        let asking = ApprovalRule { decision: RuleDecision::Ask, ..rule.clone() };
        assert!(matches!(controller.add_approval_rule(&hers, asking).await, Err(StoreError::Validation(_))), "a person allows or denies");
        controller.remove_approval_rule(&hers, rule.id).await.expect("taken back");
        assert!(matches!(controller.remove_approval_rule(&hers, rule.id).await, Err(StoreError::NotFound { .. })), "once");
        assert!(controller.profile().approval_rules().is_empty());
    }

    #[tokio::test]
    async fn a_profile_changed_elsewhere_is_taken_over() {
        let user = alice();
        let controller = UserController::new(user.clone(), None, Arc::new(()), Arc::new(MemoryStore::new()));
        let changed = UserProfile::new(user.id, Some(Introduction::new("I run the budget.").unwrap()));

        controller.process_global_event(&GlobalEvent::UserProfileUpdated(UserId::new(), UserProfile::new(UserId::new(), Some(Introduction::new("not hers").unwrap()))));
        assert_eq!(controller.profile().introduction, None, "another user's");
        controller.process_global_event(&GlobalEvent::UserProfileUpdated(user.id, changed));
        assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some("I run the budget."));
    }
}
