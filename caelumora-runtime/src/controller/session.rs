use arc_swap::ArcSwap;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::security::SecurityContext;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::session::{ChannelId, Session, SessionId, SessionUpdateEvent};
use caelumora_model::value::text::{Briefing, Summary};
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

    pub fn name(&self) -> String {
        self.session.load().name.clone()
    }

    /// Renames the session on behalf of `ctx`, as [`Self::update`] does.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_name(&self, ctx: &SecurityContext, new_name: String) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::NameChanged(new_name)).await
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
    /// replace what it said - `None` removes it.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_description(&self, ctx: &SecurityContext, summary: Option<Summary>, briefing: Option<Briefing>) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::DescriptionChanged { summary, briefing }).await
    }

    /// Applies `event` on behalf of `ctx`: stored first - which is where the
    /// permission is checked (issue 11) - and only then changed live and
    /// announced.
    ///
    /// # Errors
    /// Whatever the store reports, `StoreError::PermissionDenied` included;
    /// the session is then unchanged.
    async fn update(&self, ctx: &SecurityContext, event: SessionUpdateEvent) -> StoreResult<()> {
        let _guard = self.write_lock.lock().await;
        let session = self.apply(&event);
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
                self.session.store(Arc::new(self.apply(update)));
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

    /// The session with `event` applied, not yet stored anywhere.
    fn apply(&self, event: &SessionUpdateEvent) -> Session {
        let mut session = (**self.session.load()).clone();
        match event {
            SessionUpdateEvent::NameChanged(name) => session.name = name.clone(),
            SessionUpdateEvent::PrivacyChanged(privacy) => session.privacy = *privacy,
            SessionUpdateEvent::ApprovalRulesChanged(rules) => session.approval_rules = rules.clone(),
            SessionUpdateEvent::DescriptionChanged { summary, briefing } => {
                session.summary = summary.clone();
                session.briefing = briefing.clone();
            }
        }
        session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::security::SecurityAttributes;
    use caelumora_model::store_impl::memory::MemoryStore;

    #[tokio::test]
    async fn a_renamed_session_is_stored_first() {
        let session = Session {
            summary: None, briefing: None,
            approval_rules: Vec::new(),
            commands: Default::default(),
            privacy: Default::default(),
            id: SessionId::new(),
            name: "session".to_string(),
            participants: Vec::new(),
            channels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            security_attributes: SecurityAttributes::default(),
            deleted: None,
        };
        let store = Arc::new(MemoryStore::new());
        let ctx = SecurityContext::default();
        store.create_session(&ctx, &session).await.unwrap();
        let controller = SessionController::new(session.clone(), Arc::new(()), Arc::<MemoryStore>::clone(&store));

        controller.update_name(&ctx, "renamed".to_string()).await.unwrap();

        assert_eq!(controller.name(), "renamed");
        assert_eq!(store.get_session(&ctx, session.id).await.unwrap().name, "renamed");
    }
}
