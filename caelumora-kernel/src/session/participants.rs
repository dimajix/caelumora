//! What the session asks and stores of who takes part in it after it was built, for the procedures
//! that take them in (issue 221, `procedure::participant::take_in`) and let them go (issue 590,
//! `procedure::participant::let_go_from_session`): whose participant one to take in is, whether
//! they can be taken in, whether one may go out, and its list of participants.

use super::{SessionError, SessionLoop};
use crate::channel::{ChannelError, ChannelLoop, ResolvedParticipant};
use crate::event::SubscriptionTopic;
use crate::procedure::participant::{Entry, Exit};
use anyhow::{anyhow, Result};
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::Participant;
use caelumora_domain::session::Session;
use caelumora_domain::store::{StoreError, Stored};
use caelumora_security::{ActionType, Enforcement, SecurityContext, JOIN};
use std::sync::Arc;
use tracing::info;

impl SessionLoop {
    /// Nothing if `context` may change the session (`UpdateSession`): what taking someone into it
    /// asks, before anything else (issue 221).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may not.
    pub(crate) async fn require_changing(&self, context: &SecurityContext) -> Result<()> {
        let session = self.session();
        self.policies
            .access_policy()
            .require(
                context,
                ActionType::update(Session::KIND, &session.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        Ok(())
    }

    /// `actor`, to be taken into the session - and into `channel` if one is named - on behalf of
    /// `context` by the way `entry` says, read and resolved, once the session asked what it asks
    /// of them: that they are not in it already, unless a channel is named; by their own act,
    /// where joining, whether its access list lets them join (`JOIN`), unless they are in it. For
    /// a procedure that takes them in (`procedure::participant::take_into_session`), which asks
    /// the channel next (`ChannelLoop::require_entry`) and then anything in their name
    /// (`require_to_take_in`): whose participant it is first (issue 562). Whoever takes them in
    /// reads them first, a user to find out that they exist (issue 120) - but a founder of a
    /// channel just created, whom the session lists -, and follows one not in it from then on
    /// (`following`). Stores nothing.
    ///
    /// # Errors
    /// `SessionError::AlreadyParticipant`, `CannotTakeIn`; `ChannelError::NotInSession` for a
    /// founder of a channel the session does not list; `StoreError::PermissionDenied` if they may
    /// not join; whatever reading them reports.
    pub(crate) async fn newcomer(
        &self,
        context: &SecurityContext,
        actor: ActorId,
        channel: Option<&Arc<ChannelLoop>>,
        entry: Entry,
    ) -> Result<ResolvedParticipant> {
        let session = self.session();
        let listed = session.participants.iter().find(|p| p.actor_id() == actor).cloned();
        if listed.is_some() && channel.is_none() {
            return Err(
                SessionError::AlreadyParticipant { session: self.id, participant: actor }.into()
            );
        }
        let in_session = listed.is_some();
        let participant = match (entry, listed) {
            // A founder of a channel just created is one of the session's, named by its creator:
            // creating the channel with them was what was asked (issue 420).
            (Entry::Founding, Some(listed)) => listed,
            (Entry::Founding, None) => {
                let channel = channel.ok_or_else(|| anyhow!("{actor} founds no channel"))?;
                let outside =
                    ChannelError::NotInSession { participant: actor, channel: channel.id() };
                return Err(outside.into());
            }
            _ => {
                let participant =
                    Participant::of(actor).ok_or(SessionError::CannotTakeIn(actor))?;
                match participant {
                    Participant::User(id) => drop(self.stores.users.get(context, id).await?),
                    Participant::Agent(id) => drop(self.stores.agents.get(context, id).await?),
                }
                participant
            }
        };
        if !in_session {
            // Followed before it is resolved, so that nothing of it is missed until it is listed -
            // its deletion above all, which its actor follows once it is taken in
            // (`participant::Follow`). One refused, or taken out of the session again by
            // the compensation of taking it in (`undo_listing_held`), is followed for nothing,
            // which is harmless: its channels look only at whom they list, and a topic followed
            // already is not followed twice.
            self.events.follow(&self.following, SubscriptionTopic::of_participant(&participant));
        }
        let resolved = self.resolve_to_take_in(&participant, channel).await?;
        if !in_session && entry == Entry::Joining {
            self.participation
                .require_own_act(
                    &resolved,
                    context,
                    ActionType::custom(Session::KIND, &JOIN, &session.security_attributes),
                    StoreError::PermissionDenied,
                )
                .await?;
        }
        Ok(resolved)
    }

    /// Lists `resolved` in the session - unless it lists them already - stored in the name the way
    /// `entry` says (`Entry::storing`), for a procedure that asked what taking them in asks
    /// (`procedure::participant::take_into_session`), which had the session follow them as it read
    /// them (`newcomer`). The session announces it. Answers the session as it was before, where it
    /// listed them, which taking the listing back returns to (`undo_listing_held`); `None` for one
    /// in it already.
    ///
    /// # Errors
    /// `SessionError::CannotTakeIn` for an internal actor; what storing the session reports.
    pub(crate) async fn list_participant_held(
        &self,
        context: &SecurityContext,
        resolved: &ResolvedParticipant,
        entry: Entry,
    ) -> Result<Option<Session>> {
        let actor = resolved.actor_id();
        if self.session().takes_part(actor) {
            return Ok(None);
        }
        let participant = Participant::of(actor).ok_or(SessionError::CannotTakeIn(actor))?;
        let storing = entry.storing(context);
        let before = self.session_controller.add_participant(&storing, participant).await?;
        info!(
            session = %self.id,
            participant = %actor,
            by = %context.caller_id,
            "Taken into the session"
        );
        Ok(Some(before))
    }

    /// Takes back listing `participant` in the session, for the compensation of taking them in:
    /// the session is again as it was `before`, their invitation given back
    /// (`SessionController::roll_back_participant_addition`, issue 590).
    ///
    /// # Errors
    /// What storing the session reports.
    pub(crate) async fn undo_listing_held(
        &self,
        participant: ActorId,
        before: &Session,
    ) -> Result<()> {
        self.session_controller.roll_back_participant_addition(participant, before).await?;
        Ok(())
    }

    /// Nothing if `participant` may go out of the session on behalf of `context` by the way `exit`
    /// says, for a procedure that lets them go (`procedure::participant::let_go_from_session`),
    /// which asked whoever removes them whether they may change the session before
    /// (`require_changing`): they take part in it, or in a channel of it - one let go from the
    /// session whom a channel failed to let go -; by their own act, whether leaving
    /// (`LeaveSession`) is permitted in the participant's own context, an agent's by its owner, as
    /// leaving a channel is (`ChannelLoop::require_exit`). One that cannot be resolved, a deleted
    /// agent, cannot leave: whoever may change the session lets it go.
    ///
    /// # Errors
    /// `SessionError::NotAParticipant`; `StoreError::PermissionDenied` if they may not leave; what
    /// resolving the participant reports (`Participation::resolve`).
    pub(crate) async fn require_exit(
        &self,
        context: &SecurityContext,
        participant: ActorId,
        exit: Exit,
    ) -> Result<()> {
        let Some(leaver) = self.listed_anywhere(participant) else {
            return Err(SessionError::NotAParticipant { session: self.id, participant }.into());
        };
        if exit == Exit::Leaving {
            let attributes = self.session().security_attributes.clone();
            let leaving = ActionType::LeaveSession(&attributes);
            let Some(resolved) = self.participation.resolve(&leaver, self.id, None).await? else {
                return Err(StoreError::PermissionDenied(leaving.refused()).into());
            };
            self.participation
                .require_own_act(&resolved, context, leaving, StoreError::PermissionDenied)
                .await?;
        }
        Ok(())
    }

    /// Takes `participant` off the session's list, stored in the name of `storing`, for a procedure
    /// that lets them go (`procedure::participant::let_go_from_session`), which asked what letting
    /// them go asks; nothing for one off it already, as a letting go that failed in a channel left
    /// them. The session's controller announces it (`SessionController::remove_participant`).
    ///
    /// # Errors
    /// Whatever storing the session reports.
    pub(crate) async fn unlist_participant_held(
        &self,
        storing: &SecurityContext,
        participant: ActorId,
    ) -> Result<()> {
        match self.session_controller.remove_participant(storing, participant).await {
            Ok(_) | Err(StoreError::NotFound { .. }) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// `participant` as the session lists it, or else a channel of it - one let go from the session
    /// whom a channel failed to let go (`procedure::participant::let_go_from_session`); `None` if
    /// neither does.
    fn listed_anywhere(&self, participant: ActorId) -> Option<Participant> {
        let listed = |participants: &[Participant]| {
            participants.iter().find(|p| p.actor_id() == participant).cloned()
        };
        listed(&self.session().participants).or_else(|| {
            self.listed_channels().iter().find_map(|(channel, _)| listed(&channel.participants))
        })
    }

    /// `participant` with what its actor is built from, to be taken into the session and into
    /// `channel` too, where one is named (`Participation::resolve`). Asks nothing.
    ///
    /// # Errors
    /// `SessionError::CannotTakeIn` for a user or agent that no longer exists; whatever reading
    /// it reports.
    pub(crate) async fn resolve_to_take_in(
        &self,
        participant: &Participant,
        channel: Option<&Arc<ChannelLoop>>,
    ) -> Result<ResolvedParticipant> {
        self.participation
            .resolve(participant, self.id, channel.map(|channel| channel.id()))
            .await?
            .ok_or_else(|| SessionError::CannotTakeIn(participant.actor_id()).into())
    }

    /// Nothing if `resolved` can be taken in: it can run (`Participation::require_runnable`), and
    /// may take part in `channel` too, where one is named, as the channel answers it
    /// (`ChannelLoop::require_admissible`). What is asked before anyone is taken in,
    /// and what says whom one cannot take in (`SessionView::candidates`).
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` for an agent that may not read its personality, and cannot
    /// run; `ChannelError::OutOfScope` or `NotCleared`.
    pub(crate) async fn require_to_take_in(
        &self,
        context: &SecurityContext,
        resolved: &ResolvedParticipant,
        channel: Option<&Arc<ChannelLoop>>,
    ) -> Result<()> {
        self.participation.require_runnable(resolved).await?;
        if let Some(channel) = channel {
            channel.require_admissible(context, resolved)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::channel::ChannelLoop;
    use crate::component::BUNDLE_REF_KIND;
    use crate::event::SubscriptionTopic;
    use crate::kernel::Kernel;
    use crate::procedure::channel::Start;
    use crate::procedure::follower::AgentFollower;
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::session::testing::*;
    use crate::session::ChannelSpec;
    use crate::session::{SessionError, SessionLoop, SessionManager};
    use crate::testing::{attributes, World};
    use crate::view::Viewer;
    use anyhow::Error;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::agent::AgentId;
    use caelumora_domain::chain::Position;
    use caelumora_domain::channel::testing::main_channel;
    use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, Participant};
    use caelumora_domain::component::{ComponentBundle, ComponentConfig, ComponentDescriptor};
    use caelumora_domain::event::{Announcing, GlobalEvent};
    use caelumora_domain::group::{Group, GroupId};
    use caelumora_domain::history::{HistoryEntry, Reader};
    use caelumora_domain::scope::Scope;
    use caelumora_domain::security::{AclEntry, SecurityAttributes};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::session::{Session, SessionUpdateEvent};
    use caelumora_domain::store::StoreError;
    use caelumora_domain::user::testing::make_user;
    use caelumora_domain::user::{User, UserId};
    use caelumora_domain::value::name::ScopedName;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::record_users;
    use caelumora_store::testing::stored_agent;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time;

    fn as_user(user: &User) -> SecurityContext {
        SecurityContext::new(ActorId::User(user.id), Default::default(), Default::default())
    }

    fn refused_by_the_store(error: &Error) -> bool {
        matches!(error.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_)))
    }

    #[tokio::test]
    async fn whoever_may_change_the_session_takes_a_user_into_it_and_a_channel_at_once() {
        let (kernel, manager) = owned_kernel(Default::default());
        let stores = kernel.entity_stores();
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(stores, &[&alice, &carol]).await;
        record_users(stores, &[&bob]).await;
        let session = create(&manager, &as_user(&alice), session).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        let bob_id = ActorId::User(bob.id);
        assert!(
            manager.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_err(),
            "bob reads nothing of it before"
        );
        let mut events = kernel.event_bus().subscribe_all();

        let refused = session
            .add_participant(&as_user(&carol), bob_id, Some((main.id(), HistoryAccess::All)))
            .await
            .expect_err("a participant may not");
        assert!(refused_by_the_store(&refused), "{refused}");
        assert!(
            !session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "nothing was stored"
        );

        session
            .add_participant(&as_user(&alice), bob_id, Some((main.id(), HistoryAccess::All)))
            .await
            .expect("the owner may");
        assert!(
            session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "in the session"
        );
        assert!(main.participants().iter().any(|m| m.id == bob_id), "and in the channel");
        let members = stores
            .groups
            .get(&SecurityContext::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(members.contains(&bob_id), "the group of its participants follows");
        assert!(
            manager.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_ok(),
            "so he reads it now"
        );
        assert!(events.drain().await.iter().any(|e| matches!(e.about::<Session>(), Some((id, change)) if id == session.id()
            && matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::ParticipantJoined(p)) if p.actor_id() == bob_id))), "announced");

        let again = session
            .add_participant(&as_user(&alice), bob_id, None)
            .await
            .expect_err("in it already");
        assert!(
            matches!(
                again.downcast_ref::<SessionError>(),
                Some(SessionError::AlreadyParticipant { .. })
            ),
            "{again}"
        );
        let approver = session
            .add_participant(&as_user(&alice), ActorId::System, None)
            .await
            .expect_err("only users and agents");
        assert!(
            matches!(
                approver.downcast_ref::<SessionError>(),
                Some(SessionError::CannotTakeIn(ActorId::System))
            ),
            "{approver}"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Alice's session with carol in its channel "main" and in two that do not run once it is
    /// opened again: "unavailable", whose policy bob shared with alice and withdrew, and "deleted";
    /// her joins recorded in both (`out_of_service`).
    struct OutOfService {
        session: Arc<SessionLoop>,
        unavailable: ChannelId,
        deleted: ChannelId,
        /// Bob's policy, as withdrawn.
        withdrawn: ComponentBundle,
    }

    /// Issue 593: building again the channels left out holds each of them exclusively, so that
    /// nobody lets someone go from one meanwhile - letting go holds them shared.
    #[tokio::test]
    async fn building_again_the_channels_left_out_holds_them() {
        let world = World::new().await;
        let OutOfService { session, unavailable, .. } = out_of_service(&world).await;
        let executor = Arc::clone(world.kernel.procedures());
        let held = Held::key(&executor, Key::Channel(session.id(), unavailable), false).await;
        let starting = Start::left_out(&world.manager, session.id());
        waits_for(held, &executor, starting).await.expect("built again once it is let go");
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    async fn out_of_service(world: &World) -> OutOfService {
        let bundles = &world.kernel.entity_stores().component_bundles;
        let bobs = World::as_user(world.bob);
        let policy = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("policy", "broadcast_policy")],
        };
        let name = ScopedName::new("bobs-policy", Scope::GlobalScope).unwrap();
        let mut shared =
            ComponentBundle::new(name, policy, attributes(Some(world.bob), &[world.alice]));
        bundles.create(&bobs, &mut shared).await.expect("bob shares a policy with alice");
        let session = world.session_of(&[world.bob, world.carol], &[], &[]).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        let with_carol = |name| ChannelSpec { participants: vec![carol], ..ChannelSpec::new(name) };
        let by_bundle = ComponentConfig {
            component_nodes: vec![ComponentDescriptor {
                params: json!({ "name": "bobs-policy" }),
                ..ComponentDescriptor::new("policy", BUNDLE_REF_KIND)
            }],
        };
        let spec = ChannelSpec { components: by_bundle, ..with_carol("unavailable") };
        let unavailable = session.create_channel(&alice, spec).await.unwrap().id();
        let deleted = session.create_channel(&alice, with_carol("deleted")).await.unwrap().id();
        for channel in [unavailable, deleted] {
            assert!(carol_attending(world, channel, true).await, "her join is recorded");
        }
        assert!(session.delete_channel(&alice, deleted).await.unwrap(), "deleted");
        let unshared = attributes(Some(world.bob), &[]);
        let mut withdrawn = ComponentBundle { security_attributes: unshared, ..shared };
        bundles.update(&bobs, &mut withdrawn).await.expect("bob withdraws it");
        world.manager.stop(&alice, session.id()).await.unwrap();
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        assert!(session.channel(unavailable).is_none(), "does not run");
        OutOfService { session, unavailable, deleted, withdrawn }
    }

    /// Whether carol takes part in the history of `channel` (`HistoryStore::attending`), once she
    /// does as `open` says.
    async fn carol_attending(world: &World, channel: ChannelId, open: bool) -> bool {
        let carol = ActorId::User(world.carol);
        let history = world.kernel.history_store();
        time::timeout(Duration::from_secs(5), async {
            loop {
                if history.attending(channel).await.unwrap().contains_key(&carol) == open {
                    break;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// Issue 590: one let go from the session is let go from every channel of it - a running one,
    /// one that does not run, in its document and in the copy the session holds, and a deleted one
    /// as it is restored -, the leave is announced, and they are not taken into a channel again but
    /// through the session.
    #[tokio::test]
    async fn one_let_go_from_the_session_is_let_go_from_every_channel_of_it() {
        let world = World::new().await;
        let OutOfService { session, unavailable, deleted, .. } = out_of_service(&world).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        let main = Arc::clone(&session.channels()[0]);
        let topics = vec![SubscriptionTopic::All];
        let mut hers = world.kernel.event_bus().subscribe(World::as_user(world.carol), topics);

        session.remove_participant(&alice, carol).await.expect("the owner lets her go");

        assert!(!session.session().takes_part(carol), "out of the session");
        assert!(!main.channel().lists(carol), "out of a running channel");
        let channels = world.entity_store::<Channel>();
        let stored = channels.get(&World::system(), unavailable).await.unwrap();
        assert!(!stored.lists(carol), "out of the document of one that does not run");
        let (held, _) = session.listed_channel(unavailable).expect("listed");
        assert!(!held.lists(carol), "and of the copy the session holds");
        let members = world
            .entity_store::<Group>()
            .get(&World::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(!members.contains(&carol), "the group of its participants follows");
        let left = |event: &GlobalEvent| {
            let update = event.about::<Session>().and_then(|(_, c)| c.update());
            matches!(update, Some(SessionUpdateEvent::ParticipantLeft(id)) if id == carol)
        };
        assert!(hers.drain().await.iter().any(left), "announced, to her as well");

        let restored = session.restore_channel(&alice, deleted).await.expect("restored").channel();
        assert!(!restored.lists(carol), "let go from a deleted one as it is restored");
        let former = restored.former.iter().find(|f| f.participant.actor_id() == carol);
        let former = former.expect("kept as a former participant there");
        assert_eq!(former.left.by, ActorId::System, "removed by the kernel, as it is restored");
        let again = session.remove_participant(&alice, carol).await.expect_err("let go already");
        assert!(
            matches!(again.downcast_ref(), Some(SessionError::NotAParticipant { .. })),
            "{again}"
        );
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: a channel that did not run as someone was let go from its session records their
    /// leave as it is built (reconciling), once and in the kernel's name - one that comes back as
    /// its components can be read again, and one restored -, so that their attendance ends there.
    #[tokio::test]
    async fn a_channel_that_did_not_run_records_the_leave_as_it_is_built() {
        let world = World::new().await;
        let OutOfService { session, unavailable, deleted, withdrawn } =
            out_of_service(&world).await;
        let alice = World::as_user(world.alice);
        let carol = ActorId::User(world.carol);
        session.remove_participant(&alice, carol).await.expect("the owner lets her go");

        let reshared = attributes(Some(world.bob), &[world.alice]);
        let mut shared = ComponentBundle { security_attributes: reshared, ..withdrawn };
        let bundles = &world.kernel.entity_stores().component_bundles;
        bundles.update(&World::as_user(world.bob), &mut shared).await.expect("shared again");
        let session = world.manager.open(&alice, session.id()).await.unwrap();
        assert!(session.channel(unavailable).is_some(), "runs again");
        session.restore_channel(&alice, deleted).await.expect("restored");

        let history = world.kernel.history_store();
        for channel in [unavailable, deleted] {
            assert!(carol_attending(&world, channel, false).await, "her attendance ends");
            let page = history.page(channel, &Reader::Channel, Position::START, None, 1000, None);
            let page = page.await.unwrap();
            let left: Vec<ActorId> = page
                .entries()
                .filter_map(|entry| match entry {
                    HistoryEntry::ParticipantLeft { participant, by, .. }
                        if *participant == carol =>
                    {
                        Some(*by)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(left, [ActorId::System], "once, in the kernel's name");
        }
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: one an owner takes into the session and a channel, whose actor cannot start -
    /// no agent runs in this world -, is rolled back out of the session as well, the group of its
    /// participants following, and the leave announced.
    #[tokio::test]
    async fn one_the_channel_cannot_take_in_is_rolled_back_out_of_the_session() {
        let world = World::new().await;
        let hers = world.personality("hers", Some(world.alice), &[]).await;
        let helper = ActorId::Agent(world.agent("helper", world.alice, &hers).await);
        let session = world.session_of(&[], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let alice = World::as_user(world.alice);
        let mut events = world.kernel.event_bus().subscribe_all();

        let into = Some((main.id(), HistoryAccess::All));
        session.add_participant(&alice, helper, into).await.expect_err("it cannot start");

        assert!(!session.session().takes_part(helper), "rolled back out of the session");
        assert!(!main.channel().lists(helper), "and out of the channel");
        let members = world
            .entity_store::<Group>()
            .get(&World::system(), GroupId::of_session(session.id()))
            .await
            .unwrap()
            .members;
        assert!(!members.contains(&helper), "the group of its participants follows");
        let left = |event: &GlobalEvent| {
            let update = event.about::<Session>().and_then(|(_, c)| c.update());
            matches!(update, Some(SessionUpdateEvent::ParticipantLeft(id)) if id == helper)
        };
        assert!(events.drain().await.iter().any(left), "announced");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: a session hears none of its own announcements - its controller holds every
    /// change of it live already, and whom it takes in it follows where it takes them in -, so
    /// that a join announced after the leave that followed it does not list the one let go again.
    #[tokio::test]
    async fn a_join_heard_of_after_the_leave_does_not_list_the_one_let_go_again() {
        let world = World::new().await;
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let alice = World::as_user(world.alice);
        let bob = ActorId::User(world.bob);
        session.remove_participant(&alice, bob).await.expect("the owner lets him go");

        let joined = SessionUpdateEvent::ParticipantJoined(Participant::User(world.bob));
        let joined = GlobalEvent::updated::<Session>(&session.id(), &joined);
        world.kernel.event_bus().announce(joined, &*session.session());
        // Time for the session to hear of it.
        time::sleep(Duration::from_millis(100)).await;

        assert!(!session.session().takes_part(bob), "still let go");
        world.manager.stop(&alice, session.id()).await.unwrap();
    }

    /// Issue 590: one let go from the session whom a channel failed to let go - nothing rolls them
    /// back into the session - is let go from it by asking again: by whoever may change it, since
    /// by their own act they no longer reach the session, as letting go opens it (issue 593).
    #[tokio::test]
    async fn asking_again_finishes_letting_go_where_a_channel_failed() {
        let world = World::new().await;
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let main = Arc::clone(&session.channels()[0]);
        let bob = ActorId::User(world.bob);
        // Out of the session, as letting go leaves one whose channel failed to store.
        session.session_controller.remove_participant(&World::system(), bob).await.unwrap();
        assert!(main.channel().lists(bob), "still in its channel");

        let unreached = session.leave(&World::as_user(world.bob), bob).await.expect_err("unread");
        assert!(matches!(unreached.downcast_ref(), Some(SessionError::NotFound(_))), "{unreached}");
        let alice = World::as_user(world.alice);
        session.remove_participant(&alice, bob).await.expect("she finishes it");
        assert!(!main.channel().lists(bob), "out of the channel too");
        let again = session.remove_participant(&alice, bob).await.expect_err("nowhere");
        assert!(
            matches!(again.downcast_ref(), Some(SessionError::NotAParticipant { .. })),
            "{again}"
        );
        world.manager.stop(&World::as_user(world.alice), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn whom_the_access_list_lets_join_takes_themselves_in_and_uses_up_an_invitation() {
        let (_, manager) = owned_kernel(Default::default());
        let (alice, bob, carol) = (make_user("alice"), make_user("bob"), make_user("carol"));
        let (bob_id, carol_id) = (ActorId::User(bob.id), ActorId::User(carol.id));
        let invited = |mut attributes: SecurityAttributes| {
            attributes.acl.push(AclEntry::invitation(bob_id));
            attributes
        };
        let mut session = make_session("hers", alice.id);
        session.participants = record_users(manager.kernel().entity_stores(), &[&alice]).await;
        record_users(manager.kernel().entity_stores(), &[&bob, &carol]).await;
        session.security_attributes = invited(session.security_attributes);
        let channel = Channel {
            security_attributes: invited(main_channel(&session).security_attributes),
            ..main_channel(&session)
        };
        let session = manager.create(&as_user(&alice), session, vec![channel]).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);

        let uninvited = session
            .join(&as_user(&carol), carol_id, Some(main.id()))
            .await
            .expect_err("carol is not let in");
        // Neither invited nor reading it, she does not find it, as joining opens it (issue 593).
        assert!(matches!(uninvited.downcast_ref(), Some(SessionError::NotFound(_))), "{uninvited}");
        let for_another = session
            .join(&as_user(&bob), carol_id, None)
            .await
            .expect_err("bob joins as himself alone");
        assert!(refused_by_the_store(&for_another), "{for_another}");

        session.join(&as_user(&bob), bob_id, Some(main.id())).await.expect("bob is invited");
        assert!(
            session.session().participants.iter().any(|p| p.actor_id() == bob_id),
            "in the session"
        );
        assert!(main.participants().iter().any(|m| m.id == bob_id), "and in the channel");
        assert!(
            !session.session().security_attributes.acl.contains(&AclEntry::invitation(bob_id)),
            "his invitation is used up"
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    /// Deletes `agent` and waits until its actor stopped in `channel`: the
    /// kernel's follower heard of the deletion.
    async fn deleted_stops_in(
        kernel: &Kernel,
        ctx: &SecurityContext,
        agent: AgentId,
        channel: &ChannelLoop,
    ) {
        assert!(kernel.agent_manager().delete(ctx, agent).await.expect("deleted"));
        let stopped = async {
            while channel.participant_ids().contains(&ActorId::Agent(agent)) {
                time::sleep(Duration::from_millis(10)).await
            }
        };
        time::timeout(Duration::from_secs(2), stopped)
            .await
            .expect("its actor stops: the follower heard of it");
    }

    #[tokio::test]
    async fn an_agent_taken_in_later_is_followed_as_one_taken_in_from_the_start() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        // Followed as the programs follow them (issue 593).
        let _followed = AgentFollower::spawn(&manager);
        let helper = stored_agent(
            kernel.entity_stores(),
            "helper",
            make_personality("plain"),
            ActorId::User(UserId::new()),
        )
        .await;
        let actor = ActorId::Agent(helper.id);
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let main = Arc::clone(&session.channels()[0]);
        let mut events = kernel.event_bus().subscribe_all();
        // Running, with the participants it had as it started.
        time::sleep(Duration::from_millis(50)).await;

        session
            .add_participant(&ctx, actor, Some((main.id(), HistoryAccess::All)))
            .await
            .expect("taken in");
        assert!(main.participant_ids().contains(&actor), "it runs in the channel");
        // Announced, and followed by the follower of the agents as any agent.
        let announced = |events: &[GlobalEvent]| {
            events.iter().any(|e| matches!(e.about::<Session>().and_then(|(_, change)| change.update::<SessionUpdateEvent>()),
            Some(SessionUpdateEvent::ParticipantJoined(p)) if p.actor_id() == actor))
        };
        assert!(announced(&events.drain().await), "announced");
        time::sleep(Duration::from_millis(50)).await;

        deleted_stops_in(&kernel, &ctx, helper.id, &main).await;
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
