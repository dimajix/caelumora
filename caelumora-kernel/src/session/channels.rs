//! The channels of a running session: finding them, adding and removing them as they run, and
//! holding those that do not run.

use super::{ChannelEntry, ChannelRun, Listed, SessionError, SessionLoop};
use crate::channel::ChannelLoop;
use crate::view::{channel_reading, ReadingBasis};
use anyhow::{Error, Result};
use caelumora_api::component::ComponentError;
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::{self, Channel, ChannelId};
use caelumora_domain::confidentiality::Place;
use caelumora_domain::event::{Announcing, GlobalEvent};
use caelumora_domain::value::name::Name;
use caelumora_security::SecurityContext;
use futures::future;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

/// What `error` of building a channel's loop says if its components cannot be read
/// (`ComponentError::is_unavailable`, issue 463); `error` itself otherwise - a store that failed
/// among it.
pub(crate) fn unavailable_components(error: Error) -> Result<ComponentError, Error> {
    match error.downcast::<SessionError>() {
        Ok(SessionError::InvalidComponents(cause)) if cause.is_unavailable() => Ok(cause),
        Ok(other) => Err(other.into()),
        Err(error) => Err(error),
    }
}

impl SessionLoop {
    /// The running channels, in the order of the session: those from its
    /// template first, then those created later, oldest first.
    pub fn channels(&self) -> Vec<Arc<ChannelLoop>> {
        self.session().channels.iter().filter_map(|c| self.channel(*c)).collect()
    }

    /// Every channel the session lists, in its order, with its loop if it
    /// runs or why it does not (issue 463).
    pub(crate) fn listed_channels(&self) -> Vec<Listed> {
        self.session().channels.iter().filter_map(|c| self.listed_channel(*c)).collect()
    }

    /// The channel `channel_id` with its loop if it runs, or why it does not
    /// (issue 463); `None` if the session lists no such channel.
    pub(crate) fn listed_channel(&self, channel_id: ChannelId) -> Option<Listed> {
        self.channels.get(&channel_id).map(|entry| entry.listed())
    }

    /// Has every channel of the session follow whether its agents exist (issue 314), for
    /// `procedure::participant::Follow` and a session as it starts (`SessionManager::open_held`) -
    /// those of `only`, or every agent if `None`: a deleted one rests in them, a restored one runs
    /// again (`ChannelLoop::follow_agents`).
    pub(crate) async fn follow_agents_held(&self, only: Option<&HashSet<AgentId>>) {
        for channel in self.channels() {
            channel.follow_agents(only).await;
        }
    }

    /// Has every channel of the session take in that `participant` is called `name` now, as
    /// announced (`ChannelLoop::participant_renamed`, issue 585).
    pub(super) fn participant_renamed(&self, participant: ActorId, name: &Name) {
        for channel in self.channels() {
            channel.participant_renamed(participant, name);
        }
    }

    /// Has every channel of the session build the handles of its participants anew from the names
    /// of its actors (`ChannelLoop::rebuild_mentions`): for renames whose announcements were lost
    /// (issue 585).
    pub(super) fn rebuild_mentions(&self) {
        for channel in self.channels() {
            channel.rebuild_mentions();
        }
    }

    /// Tells the actors of every channel to stop (`ChannelLoop::stop_actors`),
    /// for a session that stops (issue 377): they keep what they are in the
    /// middle of, to be resumed when it runs again. All channels at once;
    /// returns once their actors have ended, or `STOP_GRACE` has passed.
    pub(crate) async fn stop_actors(&self) {
        future::join_all(self.channels().iter().map(|channel| channel.stop_actors())).await;
    }

    /// Has the actors of every channel leave for good
    /// (`ChannelLoop::leave_actors`), for a session that is deleted (issue
    /// 377): as when each of its channels is deleted. All channels at once;
    /// returns once their actors have ended, or `STOP_GRACE` has passed.
    pub(crate) async fn leave_actors(&self) {
        future::join_all(self.channels().iter().map(|channel| channel.leave_actors())).await;
    }

    /// The channel `channel_id` if it runs.
    pub fn channel(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.get(&channel_id).and_then(|entry| entry.running().map(Arc::clone))
    }

    /// The channel `channel_id`, if the user `context` acts for may read it,
    /// as `procedure::session::Open` opens a session (issues 70, 102):
    /// reached as the views reach a channel for a user (`channel_reading`,
    /// issue 304), by `GetChannel` and their clearance for it as a whole.
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if there is no such channel, and
    ///   equally if `context` may not read it, so that channels cannot be
    ///   probed.
    /// - `SessionError::ChannelUnavailable` if they may read it, but it does
    ///   not run: its components cannot be read (issue 463).
    pub async fn channel_for(
        &self,
        context: &SecurityContext,
        channel_id: ChannelId,
    ) -> Result<Arc<ChannelLoop>> {
        let (_, run) = self
            .listed_channel_for(context, channel_id)
            .await
            .ok_or(SessionError::ChannelNotFound { session: self.id, channel: channel_id })?;
        self.running(channel_id, run)
    }

    /// The loop of the channel `channel_id` if `run` says it runs.
    ///
    /// # Errors
    /// `SessionError::ChannelUnavailable` if it does not (issue 463).
    fn running(&self, channel_id: ChannelId, run: ChannelRun) -> Result<Arc<ChannelLoop>> {
        match run {
            ChannelRun::Running(channel_loop) => Ok(channel_loop),
            ChannelRun::Unavailable(cause) => Err(SessionError::ChannelUnavailable {
                session: self.id,
                channel: channel_id,
                cause,
            }
            .into()),
        }
    }

    /// The channel `channel_id` with whether it runs, if `context` may read
    /// it - asked as `channel_for` asks it; `None` if there is no such
    /// channel, and equally if `context` may not read it.
    pub(crate) async fn listed_channel_for(
        &self,
        context: &SecurityContext,
        channel_id: ChannelId,
    ) -> Option<Listed> {
        let (channel, run) = self.listed_channel(channel_id)?;
        let at = Place::of(&channel, self.privacy());
        channel_reading(
            &self.policies,
            context,
            &channel.security_attributes,
            &at,
            // Reached for a user alone, who reads by their clearance.
            ReadingBasis::of(None),
        )
        .await?;
        Some((channel, run))
    }

    /// Nothing if every channel the session lists runs.
    ///
    /// # Errors
    /// `SessionError::ChannelUnavailable` for the first that does not (issue
    /// 463).
    pub(super) fn require_running(&self) -> Result<()> {
        for (channel, run) in self.listed_channels() {
            self.running(channel.id, run)?;
        }
        Ok(())
    }

    /// The channel `channel_id` if it runs, for whoever joins it: not asked
    /// whether they may read it - joining it decides (`ChannelLoop::require_entry`).
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` if the session lists no such channel,
    /// `SessionError::ChannelUnavailable` if it does not run (issue 463).
    pub(crate) fn channel_to_join(&self, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
        let (_, run) = self
            .listed_channel(channel_id)
            .ok_or(SessionError::ChannelNotFound { session: self.id, channel: channel_id })?;
        self.running(channel_id, run)
    }

    /// The running channel named `name`.
    pub fn channel_named(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        self.channels
            .iter()
            .filter_map(|entry| entry.running().map(Arc::clone))
            .find(|channel| channel.name() == name)
    }

    /// Stops channel `channel_id` and takes it out of the session's channels - as it is deleted,
    /// and for the compensation of creating or restoring it: its actors leave, given the time to
    /// end by themselves before its task - and with it whoever did not - is aborted (issue 187);
    /// they store their state on their way out, which stays. Announced as stopped where it was
    /// started; one whose task never started is never to run. One that does not run (issue 463) is
    /// only taken out of the session's channels: nobody builds it again meanwhile, as building it
    /// again (`procedure::channel::Start`) holds it as deleting it does.
    pub(crate) async fn remove_channel(&self, channel_id: ChannelId) {
        let Some((_, ChannelEntry::Running { channel: running, task })) =
            self.channels.remove(&channel_id)
        else {
            return;
        };
        running.leave_actors().await;
        match task {
            Some(task) => {
                task.abort();
                let stopped =
                    GlobalEvent::custom::<Channel>(&(self.id, channel_id), channel::STOPPED, &());
                self.events.announce(stopped, &*running.channel());
            }
            None => running.never_to_run(),
        }
    }

    /// Builds `channel`, which the session lists, and has the session run it - in place of one it
    /// held as not running (issue 463), if any -, as it opens, and as a procedure creates,
    /// restores or builds again a channel (`procedure::channel`); removed by `remove_channel`.
    ///
    /// # Errors
    /// Whatever building it reports - components that cannot be read among it
    /// (`unavailable_components`); the session holds what it held then.
    pub(crate) async fn add_channel(
        self: &Arc<Self>,
        channel: &Channel,
    ) -> Result<Arc<ChannelLoop>> {
        let channel_loop = ChannelLoop::new(self.channel_runtime(channel.clone())).await?;
        self.channels.insert(
            channel.id,
            ChannelEntry::Running { channel: Arc::clone(&channel_loop), task: None },
        );
        let _ = self.start_tx.send(channel.id);
        Ok(channel_loop)
    }

    /// Holds `channel`, which the session lists, as not running, its components not to be read
    /// for `cause` (issue 463) - as it opens, and as building it again fails anew
    /// (`procedure::channel::Start`).
    pub(crate) fn hold_unavailable(&self, channel: Channel, cause: Arc<ComponentError>) {
        self.channels
            .insert(channel.id, ChannelEntry::Unavailable { channel: Arc::new(channel), cause });
    }

    /// Has the session run `channel`, which it lists, as it opens
    /// (`add_channel`) - or holds it left out, with a warning, if its
    /// components cannot be read (issue 463): a bundle it names is gone, or
    /// no longer its owner's to read. The owner of a bundle, a third party,
    /// must not keep a session from opening, as the owner of an agent's
    /// personality must not keep its channel from starting.
    ///
    /// # Errors
    /// Whatever else building it reports - a store that failed among it.
    pub(super) async fn add_listed_channel(self: &Arc<Self>, channel: Channel) -> Result<()> {
        let Err(error) = self.add_channel(&channel).await else { return Ok(()) };
        let cause = unavailable_components(error)?;
        warn!(
            "Channel {} of session {} cannot be run and is left out: {}",
            channel.id, self.id, cause
        );
        self.hold_unavailable(channel, Arc::new(cause));
        Ok(())
    }

    /// Lets `participant` go from the channel `channel_id`, which does not run (issue 463), on
    /// behalf of `context`, who let them go from the session (issue 590), for a procedure that lets
    /// them go (`procedure::participant::let_go_from_session`): in its stored document, as its
    /// controller lets one go, and in the copy the session holds, so that it is built without them.
    /// Nothing for a channel that does not list them, one that runs or one the session does not
    /// list: the procedure asks it of every channel of the session that does not run. Not while it
    /// is built again: the procedure holds the channel, which building it again holds too
    /// (`procedure::channel::Start`), so that it is neither built meanwhile nor from a copy that
    /// still lists them.
    ///
    /// # Errors
    /// Whatever storing the channel reports.
    pub(crate) async fn let_go_where_unavailable_held(
        &self,
        context: &SecurityContext,
        channel_id: ChannelId,
        participant: ActorId,
    ) -> Result<()> {
        let Some((held, ChannelRun::Unavailable(_))) = self.listed_channel(channel_id) else {
            return Ok(());
        };
        if !held.lists(participant) {
            return Ok(());
        }
        let held = Channel::clone(&held);
        let by = context.caller_id;
        let channel = self.let_go_where_not_running_held(held, &[participant], by).await?;
        if let Some(mut entry) = self.channels.get_mut(&channel_id) {
            if let ChannelEntry::Unavailable { channel: held, .. } = &mut *entry {
                *held = Arc::new(channel);
            }
        }
        Ok(())
    }

    /// `channel`, which does not run, with `participants` let go by `by` (issue 590): stored in the
    /// kernel's name - letting them go from the session was permitted - by a controller of its own,
    /// which keeps each as a former participant, lets the group of its participants follow and
    /// announces it, as for a running channel (`ChannelController::remove_participant`): one left
    /// out as the session opened (`let_go_where_unavailable_held`), or one restored - before it is
    /// built, for the document of a deleted channel cannot be changed
    /// (`procedure::channel::Restore`).
    ///
    /// # Errors
    /// Whatever storing the channel reports.
    pub(crate) async fn let_go_where_not_running_held(
        &self,
        channel: Channel,
        participants: &[ActorId],
        by: ActorId,
    ) -> Result<Channel> {
        if participants.is_empty() {
            return Ok(channel);
        }
        let controller = self.controllers.new_channel_controller(channel);
        for participant in participants {
            controller.remove_participant(&SecurityContext::system(), *participant, by).await?;
        }
        Ok(Channel::clone(&controller.load()))
    }
}

#[cfg(test)]
mod tests {
    use super::unavailable_components;
    use crate::channel::testing::under_hold;
    use crate::channel::ChannelLoop;
    use crate::error::is_forbidden;
    use crate::moderator::broadcast_policy::BROADCAST_POLICY_KIND;
    use crate::moderator::random_policy::RANDOM_POLICY_KIND;
    use crate::procedure::Key;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError, SessionLoop, SessionManager};
    use crate::view::Viewer;
    use anyhow::Error;
    use anyhow::Result;
    use caelumora_api::component::ComponentError;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::approval::ApprovalStrategy;
    use caelumora_domain::channel::moderation_components;
    use caelumora_domain::channel::testing::main_channel;
    use caelumora_domain::channel::{Channel, InternalActor, STARTED, STOPPED};
    use caelumora_domain::command::HELP_KIND;
    use caelumora_domain::component::ComponentBundle;
    use caelumora_domain::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_domain::event::EntityChange;
    use caelumora_domain::group::GroupId;
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::{Grant, SecurityAttributes, Subject};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::Session;
    use caelumora_domain::snapshot::Detail;
    use caelumora_domain::store::StoreError;
    use caelumora_domain::store::Stored;
    use caelumora_domain::template::InternalActorTemplate;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::UserId;
    use caelumora_domain::value::language::Language;
    use caelumora_domain::value::name::ScopedName;
    use caelumora_domain::value::time_zone::TimeZone;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    /// Only a bundle gone or not the owner's to read leaves a channel out as
    /// its session opens (issue 463); a store that failed among it, or any
    /// other error, keeps the session from opening.
    #[test]
    fn only_unreadable_components_leave_a_channel_out() {
        let built = |cause: StoreError| {
            Error::new(SessionError::InvalidComponents(ComponentError::Build {
                component: "policy".to_string(),
                kind: "ref".to_string(),
                cause: Box::new(ComponentError::Bundle { name: "shared".to_string(), cause }),
            }))
        };
        assert!(
            unavailable_components(built(StoreError::NotFound {
                entity: "component_bundle",
                id: "shared".to_string()
            }))
            .is_ok(),
            "gone"
        );
        assert!(
            unavailable_components(built(StoreError::Backend("unreachable".to_string()))).is_err(),
            "the store failed"
        );
        assert!(
            unavailable_components(Error::new(StoreError::Backend("unreachable".to_string())))
                .is_err(),
            "nor any other error"
        );
    }

    #[tokio::test]
    async fn a_channel_created_in_a_running_session_is_stored_announced_and_runs() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let mut rx = kernel.event_bus().subscribe_all();

        let with_approver = ChannelSpec {
            internal_actors: vec![InternalActorTemplate::Approver {
                strategy: ApprovalStrategy::Majority,
            }],
            ..spec("side", vec![other])
        };
        let channel =
            session.create_channel(&ctx, with_approver).await.expect("the channel is created");

        assert_eq!(channel.name(), "side");
        let participants = channel.participant_ids();
        assert_eq!(
            participants.iter().filter(|id| matches!(id, ActorId::User(_))).count(),
            2,
            "the creator and the other user: {participants:?}"
        );
        let Some(InternalActor::Approver(approver)) =
            channel.channel().internal_actors.first().cloned()
        else {
            panic!("the approver it names")
        };
        assert_eq!(approver.strategy, ApprovalStrategy::Majority, "deciding as named");
        assert!(
            channel.runs(ActorId::Approver(approver.id))
                && !participants.contains(&ActorId::Approver(approver.id)),
            "its actor runs, no participant (issue 420)"
        );
        let bare = session.create_channel(&ctx, spec("bare", vec![])).await.expect("created");
        assert!(
            bare.channel().internal_actors.is_empty(),
            "one that names no approver has none: no fallback"
        );
        assert!(session.channel(channel.id()).is_some());
        let names: Vec<String> = session.channels().iter().map(|c| c.name().to_string()).collect();
        assert_eq!(names, ["main", "side", "bare"], "in the order of the session");

        let stored = kernel.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
        assert!(stored.channels.contains(&channel.id()), "the session lists the channel");
        let document = kernel
            .entity_stores()
            .channels
            .get(&ctx, channel.id())
            .await
            .expect("stored as a document of its own");
        assert_eq!(document.name, "side");

        // `run` starts it asynchronously.
        time::sleep(Duration::from_millis(50)).await;
        let events = rx.drain().await;
        let created = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Created)) if cid == channel.id()));
        let started = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Custom { name: STARTED, .. })) if cid == channel.id()));
        assert!(
            created.is_some() && started.is_some() && created < started,
            "created, then started: {events:?}"
        );

        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_is_held_in_the_language_asked_for_and_in_none_otherwise() {
        use caelumora_domain::user::UserProfile;
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let language = |tag: &str| Some(Language::new(tag).unwrap());

        let owner = ctx.initiator_id.as_user_id().unwrap();
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = language("de");
        kernel.entity_stores().user_profiles.create(&ctx, &mut profile).await.unwrap();
        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert_eq!(
            unsaid.channel().language,
            None,
            "not the one the creator prefers: that is the frontend's to offer (issue 331)"
        );
        let asked = session
            .create_channel(
                &ctx,
                ChannelSpec { language: Some("en".to_string()), ..spec("asked", vec![]) },
            )
            .await
            .unwrap();
        assert_eq!(asked.channel().language, language("en"), "the one asked for");

        asked.controller().update_language(&ctx, None).await.expect("the owner changes it");
        assert_eq!(asked.channel().language, None);
        let stored = kernel.entity_stores().channels.get(&ctx, asked.id()).await.unwrap();
        assert_eq!(stored.language, None, "stored, not only in memory");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_without_a_language_of_its_own_follows_its_sessions() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let language = |tag: &str| Some(Language::new(tag).unwrap());
        let following = session.create_channel(&ctx, spec("following", vec![])).await.unwrap();
        let own = session
            .create_channel(
                &ctx,
                ChannelSpec { language: Some("fr".to_string()), ..spec("own", vec![]) },
            )
            .await
            .unwrap();
        let held_in = async |channel: &ChannelLoop| {
            let view = manager
                .views(Viewer::new(ctx.clone()))
                .channel(session.id(), channel.id())
                .await
                .unwrap();
            let snapshot = view.snapshot(Detail::empty()).await;
            assert_eq!(
                snapshot.effective_language.as_ref(),
                view.effective_language(),
                "the snapshot shows it as the view answers it"
            );
            (snapshot.language, snapshot.effective_language)
        };
        assert_eq!(held_in(&following).await, (None, None), "neither holds one");

        session.update_language(&ctx, Some("de")).await.expect("the owner changes it");
        assert_eq!(
            held_in(&following).await,
            (None, language("de")),
            "the session's, as it is now"
        );
        assert_eq!(
            held_in(&own).await,
            (language("fr"), language("fr")),
            "its own before the session's"
        );
        let stored = kernel.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
        assert_eq!(stored.language, language("de"), "stored, not only in memory");
        assert!(
            session.update_language(&ctx, Some("no language")).await.is_err(),
            "no language tag"
        );

        session.update_language(&ctx, None).await.expect("taken back");
        assert_eq!(held_in(&following).await, (None, None));
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_session_is_held_in_the_time_zone_its_owner_chooses_and_in_utc_otherwise() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let berlin = TimeZone::new("Europe/Berlin").unwrap();
        let shown = async || {
            manager
                .views(Viewer::new(ctx.clone()))
                .session(session.id())
                .await
                .unwrap()
                .snapshot(Detail::empty())
                .await
                .time_zone
        };
        assert_eq!(
            (session.session().effective_time_zone(), shown().await),
            (TimeZone::UTC, None),
            "UTC where it names none (issue 424)"
        );

        session.update_time_zone(&ctx, Some("Europe/Berlin")).await.expect("the owner changes it");
        assert_eq!(
            (session.session().effective_time_zone(), shown().await),
            (berlin, Some(berlin))
        );
        let stored = kernel.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
        assert_eq!(stored.time_zone, Some(berlin), "stored, not only in memory");
        assert!(
            session.update_time_zone(&ctx, Some("Europe/Atlantis")).await.is_err(),
            "no time zone"
        );
        session.update_time_zone(&ctx, None).await.expect("taken back");
        assert_eq!(session.session().effective_time_zone(), TimeZone::UTC);
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_uses_the_workspace_unless_its_creator_says_otherwise() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;

        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert!(unsaid.channel().uses_workspace, "by default");
        let without = session
            .create_channel(&ctx, ChannelSpec { uses_workspace: false, ..spec("without", vec![]) })
            .await
            .unwrap();
        assert!(!without.channel().uses_workspace, "as its creator says");
        let stored = kernel.entity_stores().channels.get(&ctx, without.id()).await.unwrap();
        assert!(!stored.uses_workspace, "stored, not only in memory");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_is_refused_before_anything_is_stored() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let stranger = ActorId::User(UserId::new());

        let taken = session
            .create_channel(&ctx, spec("MAIN", vec![]))
            .await
            .expect_err("its name is taken");
        assert!(
            matches!(
                taken.downcast_ref::<StoreError>(),
                Some(StoreError::NameTaken { entity: "channel" })
            ),
            "by the store, as names are compared: {taken}"
        );
        let refusal = |result: Result<Arc<ChannelLoop>>| {
            result.expect_err("refused").downcast::<SessionError>().expect("a SessionError")
        };
        assert!(matches!(
            refusal(session.create_channel(&ctx, spec("x", vec![stranger])).await),
            SessionError::NotAParticipant { .. }
        ));
        let unknown = ChannelSpec {
            components: moderation_components(None, Some("nobody")),
            ..spec("x", vec![])
        };
        assert!(matches!(
            refusal(session.create_channel(&ctx, unknown).await),
            SessionError::InvalidComponents(_)
        ));
        let two = ComponentConfig {
            component_nodes: vec![
                ComponentDescriptor::new("one", BROADCAST_POLICY_KIND),
                ComponentDescriptor::new("two", RANDOM_POLICY_KIND),
            ],
        };
        assert!(
            matches!(
                refusal(
                    session
                        .create_channel(&ctx, ChannelSpec { components: two, ..spec("x", vec![]) })
                        .await
                ),
                SessionError::InvalidComponents(_)
            ),
            "a channel has one policy"
        );
        let command =
            ComponentConfig { component_nodes: vec![ComponentDescriptor::new("help", HELP_KIND)] };
        assert!(matches!(refusal(session.create_channel(&ctx, ChannelSpec { components: command, ..spec("x", vec![]) }).await), SessionError::InvalidComponents(_)), "a channel takes no component it does not ask for, a command of the session (issue 337)");

        assert_eq!(session.channels().len(), 1);
        assert_eq!(
            kernel.entity_stores().sessions.get(&ctx, session.id()).await.unwrap().channels.len(),
            1
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_deleted_channel_is_stopped_announced_and_kept_until_it_is_restored() {
        use caelumora_domain::agent::{AgentChannelState, AgentId};
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let id = channel.id();
        let agent = AgentId::new();
        kernel
            .state_store()
            .put_agent_channel_state(agent, id, &mut AgentChannelState::default())
            .await
            .unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let mut rx = kernel.event_bus().subscribe_all();

        assert!(session.delete_channel(&ctx, id).await.expect("the owner deletes it"));

        assert!(session.channel(id).is_none());
        assert!(!kernel
            .entity_stores()
            .sessions
            .get(&ctx, session.id())
            .await
            .unwrap()
            .channels
            .contains(&id));
        assert!(kernel.entity_stores().channels.get(&ctx, id).await.is_err(), "not there");
        assert!(
            kernel.entity_stores().channels.get_deleted(&ctx, id).await.is_ok(),
            "but kept, marked (issue 164)"
        );
        assert!(kernel.state_store().get_channel_state(id).await.is_ok(), "with its state");
        let events = rx.drain().await;
        let stopped = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Custom { name: STOPPED, .. })) if cid == id));
        let deleted = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Deleted)) if cid == id));
        assert!(
            stopped.is_some() && deleted.is_some() && deleted < stopped,
            "deleted as it is marked, then stopped (issue 593): {events:?}"
        );

        assert!(!session.delete_channel(&ctx, id).await.unwrap(), "gone (issue 513)");

        // Restored: listed and running again, from the state it kept.
        let taken = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let refused = session.restore_channel(&ctx, id).await.expect_err("its name is taken");
        assert!(
            matches!(
                refused.downcast_ref::<StoreError>(),
                Some(StoreError::NameTaken { entity: "channel" })
            ),
            "{refused}"
        );
        assert!(session.delete_channel(&ctx, taken.id()).await.unwrap(), "deleted");
        session.restore_channel(&ctx, id).await.expect("restored");
        assert!(session.channel(id).is_some());
        assert!(
            kernel
                .entity_stores()
                .sessions
                .get(&ctx, session.id())
                .await
                .unwrap()
                .channels
                .contains(&id),
            "listed again"
        );
        assert!(rx.drain().await.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Restored)) if cid == id)), "and announced");
        assert!(kernel.state_store().get_channel_state(id).await.is_ok());
        let again = session.restore_channel(&ctx, id).await.expect_err("not deleted");
        assert!(
            matches!(
                again.downcast_ref::<SessionError>(),
                Some(SessionError::ChannelNotFound { .. })
            ),
            "{again}"
        );
        assert!(session.delete_channel(&ctx, id).await.unwrap(), "deleted");

        // The last channel may go too; the session keeps running without.
        let main = session.channel_named("main").unwrap().id();
        assert!(session.delete_channel(&ctx, main).await.expect("the last channel may be deleted"));
        assert!(session.channels().is_empty());
        session.create_channel(&ctx, spec("again", vec![])).await.expect("and a new one created");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_deleted_before_the_session_runs_is_never_started() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();
        let session = make_session("waiting", UserId::new());
        let channel_id = session.channels[0];
        let mut channels = [main_channel(&session)];
        let session = persist(&manager, &ctx, session, &mut channels).await;
        let session_loop =
            SessionLoop::new(session.clone(), channels.to_vec(), Arc::clone(&kernel))
                .await
                .unwrap();
        let mut rx = kernel.event_bus().subscribe_all();

        // Queued to be started, but stopped and taken out before `run` gets to it, as deleting it
        // does (`procedure::channel::Delete`) - by its building block, as the loop runs in no
        // manager that a procedure would reach it by.
        let stopping = Arc::clone(&session_loop);
        let key = Key::Channel(session.id, channel_id);
        under_hold(kernel.procedures(), key, async move {
            stopping.remove_channel(channel_id).await;
            Ok(())
        })
        .await
        .unwrap();
        let running = tokio::spawn({
            let session_loop = Arc::clone(&session_loop);
            async move { session_loop.run().await }
        });
        time::sleep(Duration::from_millis(50)).await;

        let events = rx.drain().await;
        let about = |wanted: fn(&EntityChange) -> bool| {
            events.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, id), change)) if id == channel_id && wanted(change)))
        };
        assert!(session_loop.channel(channel_id).is_none(), "taken out of the session's channels");
        assert!(
            !about(|change| matches!(change, EntityChange::Custom { name: STARTED, .. })),
            "never started: {events:?}"
        );
        assert!(
            !about(|change| matches!(change, EntityChange::Custom { name: STOPPED, .. })),
            "so never stopped: {events:?}"
        );
        running.abort();
    }

    #[tokio::test]
    async fn only_who_may_change_the_session_creates_or_deletes_its_channels() {
        let (_, manager) = owned_kernel(Default::default());

        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let as_user = |id: UserId| {
            SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
        };
        let (alice_ctx, bob_ctx) = (as_user(alice.id), as_user(bob.id));
        let mut session = make_session("hers", alice.id);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob.id)), Grant::Read);
        session.participants =
            record_users(manager.kernel().entity_stores(), &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let main = session.channel_named("main").unwrap().id();

        let refused = session
            .create_channel(&bob_ctx, spec("bobs", vec![]))
            .await
            .expect_err("a contributor may not");
        assert!(is_forbidden(&refused), "{refused}");
        let refused =
            session.delete_channel(&bob_ctx, main).await.expect_err("a contributor may not");
        assert!(is_forbidden(&refused), "{refused}");
        assert!(session.channel(main).is_some(), "a refused deletion leaves the channel");

        session.create_channel(&alice_ctx, spec("hers", vec![])).await.expect("the owner may");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_its_viewer_may_not_read_is_not_there_for_them() {
        let (_, manager) = owned_kernel(Default::default());
        let (alice, bob) = (make_user("alice"), make_user("bob"));
        let as_user = |id: UserId| {
            SecurityContext::new(ActorId::User(id), Default::default(), Default::default())
        };
        let mut session = make_session("hers", alice.id);
        session.security_attributes = session
            .security_attributes
            .clone()
            .granting(Subject::Actor(ActorId::User(bob.id)), Grant::Read);
        session.participants =
            record_users(manager.kernel().entity_stores(), &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = session.create_channel(&as_user(alice.id), spec("hers", vec![])).await.unwrap();

        let hidden =
            session.channel_for(&as_user(bob.id), hers.id()).await.expect_err("not bob's to read");
        assert!(
            matches!(
                hidden.downcast_ref::<SessionError>(),
                Some(SessionError::ChannelNotFound { .. })
            ),
            "absent, not refused: {hidden}"
        );
        assert!(
            !session.delete_channel(&as_user(bob.id), hers.id()).await.unwrap(),
            "nor there for bob to delete (issue 513)"
        );
        assert!(session.channel(hers.id()).is_some(), "and kept");
        assert!(
            session.channel_for(&as_user(alice.id), hers.id()).await.is_ok(),
            "the owner reads it"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Issue 469: a channel that cannot be listed in its session again once its group came back
    /// stays deleted with its group, both marked as they were, and tried again, comes back with
    /// its group and what lay in its scope.
    #[tokio::test]
    async fn a_channel_whose_restoring_failed_comes_back_whole_when_tried_again() {
        let (kernel, passing) = failing_kernel(Session::KIND);
        let stores = kernel.entity_stores().clone();
        let manager = SessionManager::new(kernel);
        let (ctx, system) = (SecurityContext::default(), SecurityContext::system());
        let session =
            create(&manager, &ctx, make_session("restored", UserId::new())).await.unwrap();
        let side = session.create_channel(&ctx, spec("side", vec![])).await.unwrap().id();
        let scope = Scope::ChannelScope { session_id: session.id(), channel_id: side };
        let mut bundle = ComponentBundle::new(
            ScopedName::new("tools", scope).unwrap(),
            Default::default(),
            SecurityAttributes::systems(),
        );
        stores.component_bundles.create(&system, &mut bundle).await.unwrap();
        assert!(session.delete_channel(&ctx, side).await.unwrap(), "deleted");
        let mark = stores.channels.get_deleted(&system, side).await.unwrap().removal;

        passing.store(0, Ordering::SeqCst);
        session.restore_channel(&ctx, side).await.expect_err("the session cannot list it again");
        assert_eq!(
            stores.channels.get_deleted(&system, side).await.unwrap().removal,
            mark,
            "it stays deleted, marked as it was"
        );
        assert_eq!(
            stores.groups.get_deleted(&system, GroupId::of_channel(side)).await.unwrap().removal,
            mark,
            "with its group"
        );

        passing.store(usize::MAX, Ordering::SeqCst);
        session.restore_channel(&ctx, side).await.expect("restored when tried again");
        assert!(
            stores.groups.get(&system, GroupId::of_channel(side)).await.is_ok(),
            "with its group"
        );
        assert!(
            stores.component_bundles.get(&system, bundle.id).await.is_ok(),
            "and what lay in its scope"
        );
    }
}
