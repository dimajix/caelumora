use arc_swap::ArcSwap;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityContext;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::session::{ChannelId, Session, SessionId, SessionUpdateEvent};
use caelumora_model::value::name::Name;
use caelumora_model::value::ValueError;
use caelumora_model::store::{EntityStore, StoreError, StoreResult};
use chrono::Utc;
use std::sync::Arc;
use super::check_restricting;

/// The one writer of a session's document.
#[derive(Debug)]
pub struct SessionController {
    id: SessionId,
    session: ArcSwap<Session>,
    event_publisher: Arc<dyn EventPublisher>,
    entity_store: Arc<dyn EntityStore>,
    /// Held while the session is changed and stored, so that two changes -
    /// say a rename and a new channel - cannot overwrite each other.
    write_lock: tokio::sync::Mutex<()>,
}

impl SessionController {
    pub fn new(session: Session, event_publisher: Arc<dyn EventPublisher>, entity_store: Arc<dyn EntityStore>) -> Self {
        Self {
            id: session.id,
            session: ArcSwap::from_pointee(session),
            event_publisher,
            entity_store,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<Session>> {
        self.session.load()
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn name(&self) -> Name {
        self.session.load().name.clone()
    }

    /// Renames the session on behalf of `ctx`, as [`Self::update`] does.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: &str) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::NameChanged(new_name.to_string())).await
    }

    /// Changes the session's privacy policy on behalf of `ctx` (issue 134),
    /// as [`Self::update`] does.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_privacy(&self, ctx: &SecurityContext, privacy: PrivacyConfig) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::PrivacyChanged(privacy)).await
    }

    /// Sets what the session restricts of the tool calls made in any of its
    /// channels (issue 13) on behalf of `ctx` (`UpdateSession`), as
    /// [`Self::update`] does: `rules` replace those it had. They apply to
    /// the next call of any agent in it.
    ///
    /// # Errors
    /// `StoreError::Validation` if a rule does not only ask or deny
    /// (`ApprovalRule::check_restricting`), otherwise see [`Self::update`].
    pub async fn update_approval_rules(&self, ctx: &SecurityContext, rules: Vec<ApprovalRule>) -> StoreResult<()> {
        check_restricting(&rules)?;
        self.update(ctx, SessionUpdateEvent::ApprovalRulesChanged(rules)).await
    }

    /// Says what the session is for (issue 245) on behalf of `ctx`
    /// (`UpdateSession`), as [`Self::update`] does: `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_description(&self, ctx: &SecurityContext, summary: Option<&str>, briefing: Option<&str>) -> StoreResult<()> {
        let event = SessionUpdateEvent::DescriptionChanged { summary: summary.map(String::from), briefing: briefing.map(String::from) };
        self.update(ctx, event).await
    }

    /// Applies `event` on behalf of `ctx`: stored first - which is where the
    /// permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// `StoreError::Invalid` if what it says does not fit the session,
    /// otherwise whatever the store reports, `StoreError::PermissionDenied`
    /// included; the session is then unchanged.
    async fn update(&self, ctx: &SecurityContext, event: SessionUpdateEvent) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let session = self.apply(&event)?;
        self.entity_store.update_session(ctx, &session).await?;
        let attributes = session.security_attributes.clone();
        self.session.store(Arc::new(session));
        self.event_publisher.publish(GlobalEvent::SessionUpdated(self.id, event), attributes);
        Ok(())
    }

    /// Adds `channel` to the session on behalf of `ctx`: stored first, which
    /// is where the permission is checked (`UpdateSession`), and only then
    /// changed live. Announcing it is the caller's, who also starts it
    /// (issue 09).
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the session is then unchanged.
    pub async fn add_channel(&self, ctx: &SecurityContext, channel: ChannelId) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let mut session = (**self.session.load()).clone();
        session.channels.push(channel);
        self.store(ctx, session).await
    }

    /// Removes the channel `channel_id` from the session's list on behalf of
    /// `ctx`, like [`Self::add_channel`]. The channel itself is deleted by
    /// the caller.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the session has no such channel; whatever
    /// the store reports otherwise. The session is then unchanged.
    pub async fn remove_channel(&self, ctx: &SecurityContext, channel_id: ChannelId) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let mut session = (**self.session.load()).clone();
        let index = session.channels.iter().position(|c| *c == channel_id)
            .ok_or_else(|| StoreError::NotFound { entity: "channel", id: channel_id.to_string() })?;
        session.channels.remove(index);
        self.store(ctx, session).await
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced.
    pub fn process_global_event(&self, event: &GlobalEvent) {
        if let GlobalEvent::SessionUpdated(sid, update) = event {
            if *sid == self.id {
                // Checked where it was made; one that does not fit here is
                // left out rather than taken over half.
                match self.apply(update) {
                    Ok(session) => self.session.store(Arc::new(session)),
                    Err(e) => tracing::warn!(session = %self.id, error = %e, "A change of the session made elsewhere does not fit it"),
                }
            }
        }
    }

    /// Stores `session` as changed by `ctx` and takes it over live.
    async fn store(&self, ctx: &SecurityContext, mut session: Session) -> StoreResult<()> {
        session.updated_at = Utc::now();
        self.entity_store.update_session(ctx, &session).await?;
        self.session.store(Arc::new(session));
        Ok(())
    }

    /// The session with `event` applied, not yet stored anywhere; what it
    /// says is made into the session's values here (issue 248).
    ///
    /// # Errors
    /// `ValueError` if what it says does not fit.
    fn apply(&self, event: &SessionUpdateEvent) -> Result<Session, ValueError> {
        let mut session = (**self.session.load()).clone();
        match event {
            SessionUpdateEvent::NameChanged(name) => session.name = Name::new(name)?,
            SessionUpdateEvent::PrivacyChanged(privacy) => session.privacy = *privacy,
            SessionUpdateEvent::ApprovalRulesChanged(rules) => session.approval_rules = rules.clone(),
            SessionUpdateEvent::DescriptionChanged { summary, briefing } => session.describe(summary.as_deref(), briefing.as_deref())?,
            SessionUpdateEvent::WorkspaceChanged(workspace) => session.workspace = *workspace,
        }
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::store_impl::memory::MemoryStore;

    fn session() -> Session {
        Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            workspace: None,
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId::new(),
            name: Name::new("session").unwrap(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        }
    }

    #[derive(Debug, Default)]
    struct Announced(std::sync::Mutex<Vec<GlobalEvent>>);

    impl EventPublisher for Announced {
        fn publish(&self, event: GlobalEvent, _attributes: SecurityAttributes) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn a_renamed_session_is_stored_first() {
        let session = session();
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.create_session(&ctx, &session).await.unwrap();
        let controller = SessionController::new(session.clone(), Arc::new(()), Arc::<MemoryStore>::clone(&store));

        controller.update_name(&ctx, "renamed").await.unwrap();

        assert_eq!(controller.name(), "renamed");
        assert_eq!(store.get_session(&ctx, session.id).await.unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn a_description_is_checked_as_it_is_applied() {
        let session = session();
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.create_session(&ctx, &session).await.unwrap();
        let announced = Arc::new(Announced::default());
        let controller = SessionController::new(session.clone(), Arc::<Announced>::clone(&announced), Arc::<MemoryStore>::clone(&store));

        controller.update_description(&ctx, Some("Release 2.0"), Some("")).await.unwrap();
        let stored = store.get_session(&ctx, session.id).await.unwrap();
        assert_eq!((stored.summary.as_ref().map(|s| s.as_str()), stored.briefing), (Some("Release 2.0"), None), "empty is none");
        assert!(matches!(announced.0.lock().unwrap().as_slice(), [GlobalEvent::SessionUpdated(_, SessionUpdateEvent::DescriptionChanged { .. })]), "announced");

        let refused = controller.update_description(&ctx, Some("two\nlines"), None).await;
        assert!(matches!(&refused, Err(StoreError::Invalid(ValueError::LineBreak { field: "summary" }))), "{refused:?}");

        let renamed = controller.update_name(&ctx, " ").await;
        assert!(matches!(&renamed, Err(StoreError::Invalid(ValueError::Blank { field: "name" }))), "a name is checked so, too: {renamed:?}");
        assert_eq!(store.get_session(&ctx, session.id).await.unwrap().summary, stored.summary, "unchanged by what is refused");
        assert_eq!(announced.0.lock().unwrap().len(), 1);
    }
}
