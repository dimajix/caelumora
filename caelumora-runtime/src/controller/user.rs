use arc_swap::{ArcSwap, Guard};
use caelumora_model::actor::ActorId;
use caelumora_model::approval::{ApprovalRule, ApprovalRuleId};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::value::language::Language;
use caelumora_model::security::{Secured, SecurityContext};
use caelumora_model::store::{EntityStore, EntityStores, StateStore, StoreError, StoreResult};
use caelumora_model::history::Reader;
use caelumora_model::store::history::HistoryStore;
use caelumora_model::user::{ReadMark, User, UserId, UserProfile};
use caelumora_model::channel::Channel;
use caelumora_model::message::MessageId;
use tracing::debug;
use std::sync::Arc;
use super::document::{log_changed, take_over};
use chrono::Utc;
use tokio::sync::Mutex;

/// The live user of this instance, shared by all the user's actors in all
/// sessions (issue 121), with their profile. The user record is not changed
/// through it - it belongs to the system (issue 94) and is only kept current
/// when a change is announced - but the profile is: the controller is its
/// one writer in this instance (issue 13).
///
/// Unlike the others, it holds no `LiveDocument` (issue 358): the user it
/// does not write, and the profile is created with its first change rather
/// than changed, and announced as it then stands rather than by an update
/// event. It takes the user's changes over and logs its own alike.
///
/// It is also the one writer of the user's states in channels in this
/// instance (issue 404): up to where they read each - whether the channel
/// runs or not, and whether they take part in it or only read it.
#[derive(Debug)]
pub struct UserController {
    id: UserId,
    user: ArcSwap<User>,
    /// What the user says about themselves; an empty one of theirs while
    /// they never wrote one.
    profile: ArcSwap<UserProfile>,
    event_publisher: Arc<dyn EventPublisher>,
    profiles: Arc<dyn EntityStore<UserProfile>>,
    /// Where a channel the user marks read is reached, as they read it.
    channels: Arc<dyn EntityStore<Channel>>,
    states: Arc<dyn StateStore>,
    histories: Arc<dyn HistoryStore>,
    /// Held while the profile is changed and stored, like the other
    /// controllers.
    write_lock: Mutex<()>,
    /// Held while a state of the user in a channel is read, changed and
    /// stored: its puts never overlap.
    read_lock: Mutex<()>,
}

impl UserController {
    /// The live `user` with their `profile`, `None` if they have none yet.
    /// Made by the `ControllerManager`, which keeps one per user.
    pub(crate) fn new(user: User, profile: Option<UserProfile>, event_publisher: Arc<dyn EventPublisher>, stores: &EntityStores, states: Arc<dyn StateStore>, histories: Arc<dyn HistoryStore>) -> Self {
        let id = user.id;
        Self {
            id,
            user: ArcSwap::from_pointee(user),
            profile: ArcSwap::from_pointee(profile.unwrap_or_else(|| UserProfile::new(id, None))),
            event_publisher,
            profiles: Arc::clone(&stores.user_profiles),
            channels: Arc::clone(&stores.channels),
            states,
            histories,
            write_lock: Mutex::new(()),
            read_lock: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Guard<Arc<User>> {
        self.user.load()
    }

    /// The user's profile as it stands. Handed out as the runtime holds it:
    /// whoever acts with it in someone's name checks that themselves.
    pub fn profile(&self) -> Guard<Arc<UserProfile>> {
        self.profile.load()
    }

    pub fn id(&self) -> UserId {
        self.id
    }

    pub fn username(&self) -> String {
        self.user.load().username.to_string()
    }

    /// The context of the user acting for themselves in `channel` of
    /// `session`, with their roles as they are now (issue 304): what the
    /// `UserManager` stored and announced last (issue 306).
    pub fn security_context(&self, session: SessionId, channel: ChannelId) -> SecurityContext {
        SecurityContext::new(ActorId::User(self.id), session, channel).with_roles(self.user.load().roles.clone(), None)
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
        profile.prune_approval_rules(Utc::now());
        self.save(ctx, &profile).await?;
        log_changed(ctx, &profile);
        self.profile.store(Arc::new(profile.clone()));
        let attributes = profile.security_attributes().clone();
        self.event_publisher.publish(GlobalEvent::updated::<UserProfile>(&self.id, &profile), attributes);
        Ok((changed, profile))
    }

    /// Stores `profile` as `ctx`, whether the user has one already or not:
    /// changed if there is one, else created - and changed after all if one
    /// was created meanwhile, in another instance. The store asks whether
    /// `ctx` may.
    async fn save(&self, ctx: &SecurityContext, profile: &UserProfile) -> StoreResult<()> {
        match self.profiles.update(ctx, profile).await {
            Err(StoreError::NotFound { .. }) => match self.profiles.create(ctx, profile).await {
                Err(StoreError::Conflict(_)) => self.profiles.update(ctx, profile).await,
                created => created,
            },
            updated => updated,
        }
    }

    /// Marks channel `channel_id` of session `session_id` read by the user up to message
    /// `message_id` (issue 404), on behalf of `ctx`, the user's own: the
    /// newest message of someone else they were shown. The channel is
    /// reached as `ctx` reads it - the store asks whether they may read it
    /// and are cleared for it -, and the message must be one of its history
    /// they see, from where their view of it begins. The mark only moves
    /// forward: a message before it changes nothing.
    ///
    /// # Errors
    /// `StoreError::NotFound` for a channel `ctx` does not reach in the
    /// session and for a
    /// message the user does not see in it; `Validation` for a context not
    /// the user's; otherwise whatever the stores report. Nothing is changed
    /// then.
    pub async fn mark_read(&self, ctx: &SecurityContext, session_id: SessionId, channel_id: ChannelId, message_id: MessageId) -> StoreResult<()> {
        let reader = ActorId::User(self.id);
        if ctx.caller_id != reader {
            return Err(StoreError::Validation(format!("only user {} marks what they read", self.id)));
        }
        let channel = self.channels.get(ctx, channel_id).await?;
        if channel.session_id != session_id {
            return Err(StoreError::NotFound { entity: "channel", id: channel_id.to_string() });
        }
        let not_seen = || StoreError::NotFound { entity: "message", id: message_id.to_string() };
        let (position, _) = self.histories.said(channel_id, &Reader::Participant(reader), message_id).await?.ok_or_else(not_seen)?;
        let _guard = self.read_lock.lock().await;
        let mut state = self.states.get_user_channel_state(self.id, channel_id).await?;
        if state.read_up_to(ReadMark { position, message: message_id }) {
            self.states.put_user_channel_state(self.id, channel_id, &mut state).await?;
            debug!(user = %self.id, channel = %channel_id, "Read up to message {}", message_id);
        }
        Ok(())
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced: of the user as a `LiveDocument` takes it over, of the
    /// profile as it now stands. An event carries the resulting value, so
    /// taking over this controller's own change again changes nothing.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        take_over(&self.user, &self.id, event);
        if let Some(profile) = event.about::<UserProfile>().filter(|(uid, _)| *uid == self.id).and_then(|(_, change)| change.update::<UserProfile>()) {
            self.profile.store(Arc::new(profile));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::value::name::Name;
    use caelumora_model::value::text::Introduction;
    use caelumora_model::actor::ActorId;
    use caelumora_model::role::RoleId;
    use caelumora_model::user::UserUpdateEvent;
    use caelumora_model::store::memory::MemoryStore;
    use caelumora_model::store::Persistence;
    use caelumora_model::security::OwnershipPolicy;
    use std::sync::Mutex;
    use caelumora_model::confidentiality::{Classification, ConfidentialityRules};
    use caelumora_model::security::SecurityAttributes;

    /// What was announced, in order.
    #[derive(Debug, Default)]
    struct Announced(Mutex<Vec<GlobalEvent>>);

    impl EventPublisher for Announced {
        fn publish(&self, event: GlobalEvent, _attributes: SecurityAttributes) {
            self.0.lock().unwrap().push(event);
        }

        fn publish_classified(&self, event: GlobalEvent, attributes: SecurityAttributes, _classifications: &[Classification]) {
            self.publish(event, attributes);
        }
    }

    fn alice() -> User {
        User { id: UserId::new(), username: Name::new("alice").unwrap(), email: String::new(), roles: Vec::new(), deleted: None }
    }

    fn as_user(id: UserId) -> SecurityContext {
        SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
    }

    /// The controller of `user`, keeping everything in `store`.
    fn controller_in(store: &Persistence, user: &User, event_publisher: Arc<dyn EventPublisher>) -> UserController {
        UserController::new(user.clone(), None, event_publisher, &store.entity_stores().unwrap(), store.state_store(), store.history_store())
    }

    #[tokio::test]
    async fn a_profile_is_stored_first_then_taken_over_and_announced() {
        let store = Persistence::new(Arc::new(MemoryStore::new()), Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let announced = Arc::new(Announced::default());
        let user = alice();
        let controller = controller_in(&store, &user, Arc::<Announced>::clone(&announced));
        let hers = as_user(user.id);

        for text in ["I run the budget.", "I run the budget and the team."] {
            let profile = controller.update_profile(&hers, Some(text), None).await.expect("her own, created once and changed after");
            assert_eq!(profile.introduction.as_ref().map(Introduction::as_str), Some(text));
            assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some(text), "live");
            assert_eq!(store.entity_stores().unwrap().user_profiles.get(&hers, user.id).await.unwrap().introduction.as_ref().map(Introduction::as_str), Some(text), "stored");
        }
        assert!(matches!(announced.0.lock().unwrap().as_slice(), [_, event] if matches!(event.about::<UserProfile>(), Some((id, change)) if id == user.id && change.update::<UserProfile>().is_some_and(|p| p.introduction.as_ref().map(Introduction::as_str) == Some("I run the budget and the team.")))));
    }

    /// Issue 306: the roles the `UserManager` stored and announced are the
    /// controller's, too; another user's change is not.
    #[test]
    fn the_roles_follow_their_announcement() {
        let user = alice();
        let controller = controller_in(&Persistence::in_memory(), &user, Arc::new(Announced::default()));
        let staff = RoleId::new();
        controller.process_global_event(&GlobalEvent::updated::<User>(&UserId::new(), &UserUpdateEvent::RolesChanged(vec![staff])));
        assert!(controller.load().roles.is_empty(), "not someone else's");
        controller.process_global_event(&GlobalEvent::updated::<User>(&user.id, &UserUpdateEvent::RolesChanged(vec![staff])));
        assert_eq!(controller.load().roles, [staff], "given");
        controller.process_global_event(&GlobalEvent::updated::<User>(&user.id, &UserUpdateEvent::RolesChanged(Vec::new())));
        assert!(controller.load().roles.is_empty(), "and taken");
    }

    #[tokio::test]
    async fn a_refused_profile_changes_nothing() {
        let store = Persistence::new(Arc::new(MemoryStore::new()), Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let announced = Arc::new(Announced::default());
        let user = alice();
        let controller = controller_in(&store, &user, Arc::<Announced>::clone(&announced));

        let invalid = controller.update_profile(&as_user(user.id), Some("a\u{1b}[2Jb"), None).await;
        assert!(matches!(invalid, Err(StoreError::Invalid(_))), "{invalid:?}");
        let invalid = controller.update_profile(&as_user(user.id), None, Some("Deutsch")).await;
        assert!(matches!(invalid, Err(StoreError::Invalid(_))), "a language, too: {invalid:?}");

        let forged = controller.update_profile(&as_user(UserId::new()), Some("I am alice."), None).await;
        assert!(matches!(forged, Err(StoreError::PermissionDenied(_))), "not by someone else: {forged:?}");

        assert_eq!(controller.profile().introduction, None);
        assert!(announced.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_rule_is_added_and_taken_back_by_the_user_alone() {
        use caelumora_model::approval::{Reach, RuleDecision, ToolMatcher};
        let store = Persistence::new(Arc::new(MemoryStore::new()), Arc::new(OwnershipPolicy::new()), Arc::new(ConfidentialityRules::default()));
        let user = alice();
        let controller = controller_in(&store, &user, Arc::new(()));
        let hers = as_user(user.id);
        controller.update_profile(&hers, Some("I run the budget."), None).await.unwrap();
        let rule = ApprovalRule::new(ToolMatcher { tool: "shell".to_string(), arguments: Default::default() }, RuleDecision::Allow, Reach::Everywhere, None).unwrap();

        assert!(matches!(controller.add_approval_rule(&as_user(UserId::new()), rule.clone()).await, Err(StoreError::PermissionDenied(_))), "not by someone else");
        controller.add_approval_rule(&hers, rule.clone()).await.expect("hers");
        assert_eq!(controller.profile().approval_rules(), std::slice::from_ref(&rule), "live");
        assert_eq!(store.entity_stores().unwrap().user_profiles.get(&hers, user.id).await.unwrap().approval_rules(), std::slice::from_ref(&rule), "stored");
        assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some("I run the budget."), "the rest stays");
        controller.update_profile(&hers, None, None).await.unwrap();
        assert_eq!(controller.profile().approval_rules(), std::slice::from_ref(&rule), "and stays through a change of the rest");

        let asking = ApprovalRule { decision: RuleDecision::Ask, ..rule.clone() };
        assert!(matches!(controller.add_approval_rule(&hers, asking).await, Err(StoreError::Validation(_))), "a person allows or denies");
        controller.remove_approval_rule(&hers, rule.id).await.expect("taken back");
        assert!(matches!(controller.remove_approval_rule(&hers, rule.id).await, Err(StoreError::NotFound { .. })), "once");
        assert!(controller.profile().approval_rules().is_empty());
    }

    #[tokio::test]
    async fn a_profile_changed_elsewhere_is_taken_over() {
        let user = alice();
        let controller = controller_in(&Persistence::in_memory(), &user, Arc::new(()));
        let changed = UserProfile::new(user.id, Some(Introduction::new("I run the budget.").unwrap()));

        controller.process_global_event(&GlobalEvent::updated::<UserProfile>(&UserId::new(), &UserProfile::new(UserId::new(), Some(Introduction::new("not hers").unwrap()))));
        assert_eq!(controller.profile().introduction, None, "another user's");
        controller.process_global_event(&GlobalEvent::updated::<UserProfile>(&user.id, &changed));
        assert_eq!(controller.profile().introduction.as_ref().map(Introduction::as_str), Some("I run the budget."));
    }
}
