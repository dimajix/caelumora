//! `SessionController`: the one writer of a running session's document.

use super::check_restricting;
use super::document::LiveDocument;
use crate::group::keep_participants;
use arc_swap::Guard;
use caelumora_model::approval::ApprovalRule;
use caelumora_model::channel::Participant;
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::group::Group;
use caelumora_model::privacy::PrivacyConfig;
use caelumora_model::project::ProjectId;
use caelumora_model::security::SecurityContext;
use caelumora_model::session::{ChannelId, Session, SessionId, SessionUpdateEvent};
use caelumora_model::store::{EntityStore, EntityStores, StoreError, StoreResult};
use caelumora_model::value::name::Name;
use caelumora_model::workspace::WorkspaceId;
use std::sync::Arc;
use tracing::error;

/// The one writer of a session's document.
#[derive(Debug)]
pub struct SessionController {
    session: LiveDocument<Session>,
    groups: Arc<dyn EntityStore<Group>>,
}

impl SessionController {
    pub fn new(
        session: Session,
        event_publisher: Arc<dyn EventPublisher>,
        stores: &EntityStores,
    ) -> Self {
        Self {
            session: LiveDocument::new(
                session.id,
                session,
                Arc::clone(&stores.sessions),
                Arc::clone(&stores.groups),
                event_publisher,
            ),
            groups: Arc::clone(&stores.groups),
        }
    }

    pub fn load(&self) -> Guard<Arc<Session>> {
        self.session.load()
    }

    pub fn id(&self) -> SessionId {
        *self.session.address()
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

    /// Has the channels of the session that hold no language of their own
    /// held in `language` - or in none in particular - on behalf of `ctx`
    /// (`UpdateSession`, issue 425), as [`Self::update`] does.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `language` is no language tag, otherwise see
    /// [`Self::update`].
    pub async fn update_language(
        &self,
        ctx: &SecurityContext,
        language: Option<&str>,
    ) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::LanguageChanged(language.map(String::from))).await
    }

    /// Has the session held in the time zone `time_zone` - or in UTC - on
    /// behalf of `ctx` (`UpdateSession`, issue 424), as [`Self::update`]
    /// does.
    ///
    /// # Errors
    /// `StoreError::Invalid` if `time_zone` is no time zone, otherwise see
    /// [`Self::update`].
    pub async fn update_time_zone(
        &self,
        ctx: &SecurityContext,
        time_zone: Option<&str>,
    ) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::TimeZoneChanged(time_zone.map(String::from))).await
    }

    /// Changes the session's privacy policy on behalf of `ctx` (issue 134),
    /// as [`Self::update`] does.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_privacy(
        &self,
        ctx: &SecurityContext,
        privacy: PrivacyConfig,
    ) -> StoreResult<()> {
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
    pub async fn update_approval_rules(
        &self,
        ctx: &SecurityContext,
        rules: Vec<ApprovalRule>,
    ) -> StoreResult<()> {
        check_restricting(&rules)?;
        self.update(ctx, SessionUpdateEvent::ApprovalRulesChanged(rules)).await
    }

    /// Says what the session is for (issue 245) on behalf of `ctx`
    /// (`UpdateSession`), as [`Self::update`] does: `summary` and `briefing`
    /// replace what it said - a missing or empty one removes it.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_description(
        &self,
        ctx: &SecurityContext,
        summary: Option<&str>,
        briefing: Option<&str>,
    ) -> StoreResult<()> {
        let event = SessionUpdateEvent::DescriptionChanged {
            summary: summary.map(String::from),
            briefing: briefing.map(String::from),
        };
        self.update(ctx, event).await
    }

    /// Attaches `workspace` to the session, or with `None` takes the one
    /// attached away (issue 253), on behalf of `ctx` (`UpdateSession`), as
    /// [`Self::update`] does. Whether the workspace may be shared is the
    /// caller's to have asked.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_workspace(
        &self,
        ctx: &SecurityContext,
        workspace: Option<WorkspaceId>,
    ) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::WorkspaceChanged(workspace)).await
    }

    /// Has the session's results go to `project` from now on (issue 467), on behalf of `ctx`
    /// (`UpdateSession`), as [`Self::update`] does. Whether the session may lie in it is the
    /// caller's to have asked.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn update_project(
        &self,
        ctx: &SecurityContext,
        project: ProjectId,
    ) -> StoreResult<()> {
        self.update(ctx, SessionUpdateEvent::ProjectChanged(project)).await
    }

    /// Takes `participant` into the session on behalf of `ctx` (issue 221)
    /// as [`Self::update`] does - whoever may change it (`UpdateSession`),
    /// or the runtime for one who joined by themselves (`JOIN`) - using up
    /// an invitation (`Session::take_in`). The group of its participants
    /// follows (issue 286) before it is announced, so that the new one hears
    /// of it too. Whether they may be taken in is the caller's to have
    /// asked.
    ///
    /// # Errors
    /// See [`Self::update`].
    pub async fn add_participant(
        &self,
        ctx: &SecurityContext,
        participant: Participant,
    ) -> StoreResult<()> {
        let event = SessionUpdateEvent::ParticipantJoined(participant);
        let mut writing = self.session.write().await;
        let session = writing.apply(ctx, &event).await?;
        self.keep_participants(&session).await;
        writing.announce_update(&event, &session);
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
        let mut writing = self.session.write().await;
        writing
            .change(ctx, |session| {
                session.channels.push(channel);
                Ok(())
            })
            .await
            .map(drop)
    }

    /// Removes the channel `channel_id` from the session's list on behalf of
    /// `ctx`, like [`Self::add_channel`]. The channel itself is deleted by
    /// the caller.
    ///
    /// # Errors
    /// `StoreError::NotFound` if the session has no such channel; whatever
    /// the store reports otherwise. The session is then unchanged.
    pub async fn remove_channel(
        &self,
        ctx: &SecurityContext,
        channel_id: ChannelId,
    ) -> StoreResult<()> {
        let mut writing = self.session.write().await;
        writing
            .change(ctx, |session| {
                let index =
                    session.channels.iter().position(|c| *c == channel_id).ok_or_else(|| {
                        StoreError::NotFound { entity: "channel", id: channel_id.to_string() }
                    })?;
                session.channels.remove(index);
                Ok(())
            })
            .await
            .map(drop)
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced (`LiveDocument::take_over`).
    pub fn process_global_event(&self, event: &GlobalEvent) {
        self.session.take_over(event);
    }

    /// Applies `event` on behalf of `ctx` (`LiveDocument::update`): stored
    /// first - which is where the permission is checked (issue 11) - and
    /// only then changed live and announced.
    ///
    /// # Errors
    /// `StoreError::Invalid` if what it says does not fit the session,
    /// otherwise whatever the store reports, `StoreError::PermissionDenied`
    /// included; the session is then unchanged.
    async fn update(&self, ctx: &SecurityContext, event: SessionUpdateEvent) -> StoreResult<()> {
        self.session.update(ctx, &event).await.map(drop)
    }

    /// Lets the group of the participants of `session` follow them, in the
    /// runtime's name (issue 286), as a channel's does: the change was
    /// permitted and is stored. A failure is logged and corrected by the
    /// next change, which derives the group anew.
    async fn keep_participants(&self, session: &Session) {
        if let Err(e) = keep_participants(self.groups.as_ref(), Group::of_session(session)).await {
            error!(session = %session.id, "The group of the session's participants does not follow them: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::confidentiality::Classification;
    use caelumora_model::security::{Policies, SecurityAttributes};
    use caelumora_model::session::testing::bare_session;
    use caelumora_model::store::Persistence;
    use caelumora_model::value::ValueError;
    use chrono::{Duration, Utc};
    use std::sync::Mutex;

    /// A session's document alone, owned by the system: what the controller
    /// changes and stores. Its tests are about storing a change, not about a
    /// session the runtime makes, with the groups of its participants and a
    /// project of its own (issue 486).
    fn session() -> Session {
        bare_session(SessionId::new())
    }

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

    #[tokio::test]
    async fn a_renamed_session_is_stored_first() {
        let mut session = session();
        let store = Persistence::in_memory();
        let stores = EntityStores::open(&store, &Policies::permitting_all()).unwrap();
        let ctx = SecurityContext::default();
        stores.sessions.create(&ctx, &mut session).await.unwrap();
        let controller = SessionController::new(session.clone(), Arc::new(()), &stores);

        controller.update_name(&ctx, "renamed").await.unwrap();

        assert_eq!(controller.name(), "renamed");
        assert_eq!(stores.sessions.get(&ctx, session.id).await.unwrap().name, "renamed");
    }

    /// Issue 492: a change made by hand is made again when the session was written past the
    /// controller meanwhile, on the session as written.
    #[tokio::test]
    async fn a_change_is_made_again_when_the_session_was_written_meanwhile() {
        let mut session = session();
        let store = Persistence::in_memory();
        let ctx = SecurityContext::default();
        let stores = EntityStores::open(&store, &Policies::permitting_all()).unwrap();
        stores.sessions.create(&ctx, &mut session).await.unwrap();
        let controller = SessionController::new(session.clone(), Arc::new(()), &stores);
        let mut renamed = session.clone();
        renamed.name = Name::new("renamed").unwrap();
        stores.sessions.update(&ctx, &mut renamed).await.unwrap();

        let channel = ChannelId::new();
        controller.add_channel(&ctx, channel).await.expect("made again on what was written");

        let stored = stores.sessions.get(&ctx, session.id).await.unwrap();
        assert_eq!(stored.name, "renamed", "what was written meanwhile stands");
        assert_eq!(stored.channels, [channel], "and the change is made");
        assert_eq!(controller.name(), "renamed", "taken over live");
    }

    /// Issue 358: whatever changes the session moves `updated_at`, an update
    /// event as a channel added.
    #[tokio::test]
    async fn every_change_moves_updated_at() {
        let long_ago = Utc::now() - Duration::days(1);
        let mut session = Session { updated_at: long_ago, ..session() };
        let store = Persistence::in_memory();
        let stores = EntityStores::open(&store, &Policies::permitting_all()).unwrap();
        let ctx = SecurityContext::default();
        stores.sessions.create(&ctx, &mut session).await.unwrap();
        let controller = SessionController::new(session.clone(), Arc::new(()), &stores);
        let stored = || async { stores.sessions.get(&ctx, session.id).await.unwrap().updated_at };

        controller.update_name(&ctx, "renamed").await.unwrap();
        let renamed = stored().await;
        assert!(renamed > long_ago, "by a rename");
        assert_eq!(controller.load().updated_at, renamed, "live as stored");
        controller.add_channel(&ctx, ChannelId::new()).await.unwrap();
        assert!(stored().await > renamed, "by a channel added");
    }

    #[tokio::test]
    async fn a_description_is_checked_as_it_is_applied() {
        let mut session = session();
        let store = Persistence::in_memory();
        let stores = EntityStores::open(&store, &Policies::permitting_all()).unwrap();
        let ctx = SecurityContext::default();
        stores.sessions.create(&ctx, &mut session).await.unwrap();
        let announced = Arc::new(Announced::default());
        let controller =
            SessionController::new(session.clone(), Arc::<Announced>::clone(&announced), &stores);

        controller.update_description(&ctx, Some("Release 2.0"), Some("")).await.unwrap();
        let stored = stores.sessions.get(&ctx, session.id).await.unwrap();
        assert_eq!(
            (stored.summary.as_ref().map(|s| s.as_str()), stored.briefing),
            (Some("Release 2.0"), None),
            "empty is none"
        );
        assert!(
            matches!(announced.0.lock().unwrap().as_slice(), [event] if matches!(event.about::<Session>().and_then(|(_, change)| change.update::<SessionUpdateEvent>()), Some(SessionUpdateEvent::DescriptionChanged { .. }))),
            "announced"
        );

        let refused = controller.update_description(&ctx, Some("two\nlines"), None).await;
        assert!(
            matches!(
                &refused,
                Err(StoreError::Invalid(ValueError::LineBreak { field: "summary" }))
            ),
            "{refused:?}"
        );

        let renamed = controller.update_name(&ctx, " ").await;
        assert!(
            matches!(&renamed, Err(StoreError::Invalid(ValueError::Blank { field: "name" }))),
            "a name is checked so, too: {renamed:?}"
        );
        assert_eq!(
            stores.sessions.get(&ctx, session.id).await.unwrap().summary,
            stored.summary,
            "unchanged by what is refused"
        );
        assert_eq!(announced.0.lock().unwrap().len(), 1);
    }
}
