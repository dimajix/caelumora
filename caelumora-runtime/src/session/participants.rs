//! Taking someone into a running session after it was built (issue 221): by whoever may change it,
//! or by their own act where its access list lets them join - into the session and, if one is
//! named, into one of its channels at once.

use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::channel::{Channel, ChannelId, HistoryAccess, Participant};
use caelumora_model::security::{ActionType, Enforcement, SecurityContext};
use caelumora_model::session::Session;
use caelumora_model::store::{StoreError, Stored};
use std::sync::Arc;
use tracing::info;
use crate::channel::{ChannelLoop, ActorSource};
use super::{SessionError, SessionLoop};

impl SessionLoop {
    /// Takes `participant` - a user, or an agent `context` may read (issue 14: nobody brings in
    /// an agent whose tools they may not use) - into the session on behalf of `context`, who may
    /// change it (`UpdateSession`, asked before anything else), and with `into` into that channel
    /// at once, seeing as much of its history as granted there - as whoever may change the
    /// channel (`UpdateChannel`). One in the session already is only added to the channel, which
    /// needs no right to change the session (issue 46). Everything is asked before anything is
    /// stored - their clearance for the channel too; the session is stored first, then the
    /// channel: one the channel refuses after all stays in the session.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if `context` may not change the session or the channel;
    /// `SessionError::ChannelNotFound` for a channel they may not read;
    /// `SessionError::AlreadyParticipant` for one in the session already and no channel named;
    /// as `take_in` otherwise, and as `ChannelLoop::add_participant`.
    pub async fn add_participant(&self, context: &SecurityContext, participant: ActorId, into: Option<(ChannelId, HistoryAccess)>) -> Result<()> {
        let session = self.session();
        if into.is_none() || !session.participants.iter().any(|p| p.actor_id() == participant) {
            self.access_policy.require(context, ActionType::update(Session::KIND, &session.security_attributes), StoreError::PermissionDenied).await?;
        }
        let channel = match into {
            Some((id, history)) => {
                let channel = self.channel_for(context, id).await?;
                self.access_policy.require(context, ActionType::update(Channel::KIND, &channel.security_attributes()), StoreError::PermissionDenied).await?;
                Some((channel, history))
            }
            None => None,
        };
        self.take_in(context, participant, channel.as_ref().map(|(channel, _)| channel), false).await?;
        if let Some((channel, history)) = channel {
            channel.add_participant(context, participant, history).await?;
        }
        Ok(())
    }

    /// Takes `participant` - whom `context` acts for: themselves, or their own agent - into the
    /// session by their own act, where an entry of its access list lets them join (`JOIN`), and
    /// with `into` into that channel at once, where the channel's list lets them join as well
    /// (`ChannelLoop::join`). Asked and stored as `add_participant`, but stored in the
    /// runtime's name once joining was permitted.
    ///
    /// # Errors
    /// `StoreError::PermissionDenied` if they may not join the session or the channel;
    /// `SessionError::ChannelNotFound` if there is no such channel; as `add_participant`
    /// otherwise.
    pub async fn join(&self, context: &SecurityContext, participant: ActorId, into: Option<ChannelId>) -> Result<()> {
        let channel = match into {
            Some(id) => Some(self.channel(id).ok_or(SessionError::ChannelNotFound { session: self.id, channel: id })?),
            None => None,
        };
        self.take_in(context, participant, channel.as_ref(), true).await?;
        if let Some(channel) = channel {
            channel.join(context, participant).await?;
        }
        Ok(())
    }

    /// Takes `actor` into the session on behalf of `context` - by their own act if `joining` -,
    /// once it is asked whether they may take part in `channel` as well, if one is named
    /// (`source_to_take_in`), and whether they may join it by themselves.
    /// Whoever takes them in reads them first, a user to find out that they exist (issue 120).
    /// The session announces it, and follows an agent taken in (`run`).
    ///
    /// # Errors
    /// `SessionError::AlreadyParticipant`, `CannotTakeIn`; `StoreError::PermissionDenied` if they
    /// may not join; `ChannelError::OutOfScope` or `NotCleared`; whatever reading them or
    /// storing the session reports.
    async fn take_in(&self, context: &SecurityContext, actor: ActorId, channel: Option<&Arc<ChannelLoop>>, joining: bool) -> Result<()> {
        let session = self.session();
        let in_session = session.participants.iter().any(|p| p.actor_id() == actor);
        if in_session && channel.is_none() {
            return Err(SessionError::AlreadyParticipant { session: self.id, participant: actor }.into());
        }
        let participant = match actor {
            ActorId::User(id) => {
                self.stores.users.get(context, id).await?;
                Participant::User(id)
            }
            ActorId::Agent(id) => {
                self.stores.agents.get(context, id).await?;
                Participant::Agent(id)
            }
            other => return Err(SessionError::CannotTakeIn(other).into()),
        };
        let source = self.source_to_take_in(context, &participant, channel).await?;
        // Joining was permitted; storing is the runtime's.
        let storing = match (in_session, joining) {
            (true, _) => None,
            (false, true) => {
                self.participation.require_joinable(&source, context, Session::KIND, &session.security_attributes, StoreError::PermissionDenied).await?;
                Some(SecurityContext::system())
            }
            (false, false) => Some(context.clone()),
        };
        if let (Some(channel), true) = (channel, joining) {
            self.participation.require_joinable(&source, context, Channel::KIND, &channel.security_attributes(), StoreError::PermissionDenied).await?;
        }
        if let Some(storing) = storing {
            self.session_controller.add_participant(&storing, participant).await?;
            info!(session = %self.id, participant = %actor, by = %context.caller_id, "Taken into the session");
        }
        Ok(())
    }

    /// `participant` with what its actor is built from, if they may take part in `channel` too, where one is named: it
    /// lies where it may and is cleared for it, asked in `context`, whoever takes them in. What
    /// is asked before anyone is taken in, and what says whom one cannot take in
    /// (`SessionView::candidates`).
    ///
    /// # Errors
    /// `SessionError::CannotTakeIn` for a user or agent that no longer exists;
    /// `ChannelError::OutOfScope` or `NotCleared`; `StoreError::PermissionDenied` for an agent
    /// that may not read its personality, and cannot run; whatever reading it reports.
    pub(crate) async fn source_to_take_in(&self, context: &SecurityContext, participant: &Participant, channel: Option<&Arc<ChannelLoop>>) -> Result<ActorSource> {
        let source = self.participation.source_of(participant, self.id, channel.map(|channel| channel.id())).await?
            .ok_or(SessionError::CannotTakeIn(participant.actor_id()))?;
        if let Some(channel) = channel {
            self.participation.require_admissible(&source, context, &channel.classification())?;
        }
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::actor::ActorId;
    use caelumora_model::agent::{Agent, Personality, PersonalityId};
    use caelumora_model::channel::{Channel, HistoryAccess};
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::group::GroupId;
    use caelumora_model::scope::Scope;
    use caelumora_model::security::{AclEntry, Grant, SecurityAttributes, SecurityContext, Subject};
    use caelumora_model::session::{Session, SessionUpdateEvent};
    use caelumora_model::store::StoreError;
    use caelumora_model::user::User;
    use caelumora_model::value::name::ScopedName;
    use std::sync::Arc;
    use std::time::Duration;
    use crate::session::{SessionError, SessionManager};
    use crate::session::testing::*;
    use crate::view::Viewer;

    fn as_user(user: &User) -> SecurityContext {
        SecurityContext::new(ActorId::User(user.id), Default::default(), Default::default())
    }

    fn refused_by_the_store(error: &anyhow::Error) -> bool {
        matches!(error.downcast_ref::<StoreError>(), Some(StoreError::PermissionDenied(_)))
    }

    #[tokio::test]
    async fn whoever_may_change_the_session_takes_a_user_into_it_and_a_channel_at_once() {
        let (store, manager) = owned_runtime(Default::default());
        let runtime = Arc::clone(runtime_of(&manager));
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice, &carol]).await;
        record_users(&manager, &[&bob]).await;
        let session = create(&manager, &as_user(&alice), session).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);
        let bob_id = ActorId::User(bob.id);
        assert!(manager.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_err(), "bob reads nothing of it before");
        let mut events = runtime.event_bus().subscribe_all();

        let refused = session.add_participant(&as_user(&carol), bob_id, Some((main.id(), HistoryAccess::All))).await.expect_err("a participant may not");
        assert!(refused_by_the_store(&refused), "{refused}");
        assert!(!session.session().participants.iter().any(|p| p.actor_id() == bob_id), "nothing was stored");

        session.add_participant(&as_user(&alice), bob_id, Some((main.id(), HistoryAccess::All))).await.expect("the owner may");
        assert!(session.session().participants.iter().any(|p| p.actor_id() == bob_id), "in the session");
        assert!(main.members().iter().any(|m| m.id == bob_id), "and in the channel");
        let members = store.entity_stores().groups.get(&SecurityContext::system(), GroupId::of_session(session.id())).await.unwrap().members;
        assert!(members.contains(&bob_id), "the group of its participants follows");
        assert!(manager.views(Viewer::new(as_user(&bob))).session(session.id()).await.is_ok(), "so he reads it now");
        assert!(drain(&mut events).iter().any(|e| matches!(e.about::<Session>(), Some((id, change)) if id == session.id()
            && matches!(change.update::<SessionUpdateEvent>(), Some(SessionUpdateEvent::ParticipantAdded(p)) if p.actor_id() == bob_id))), "announced");

        let again = session.add_participant(&as_user(&alice), bob_id, None).await.expect_err("in it already");
        assert!(matches!(again.downcast_ref::<SessionError>(), Some(SessionError::AlreadyParticipant { .. })), "{again}");
        let approver = session.add_participant(&as_user(&alice), ActorId::System, None).await.expect_err("only users and agents");
        assert!(matches!(approver.downcast_ref::<SessionError>(), Some(SessionError::CannotTakeIn(ActorId::System))), "{approver}");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn whom_the_access_list_lets_join_takes_themselves_in_and_uses_up_an_invitation() {
        let (_, manager) = owned_runtime(Default::default());
        let (alice, bob, carol) = (user("alice"), user("bob"), user("carol"));
        let (bob_id, carol_id) = (ActorId::User(bob.id), ActorId::User(carol.id));
        let invited = |attributes: SecurityAttributes| attributes.granting(Subject::Actor(bob_id), Grant::Join);
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.participants = record_users(&manager, &[&alice]).await;
        record_users(&manager, &[&bob, &carol]).await;
        session.security_attributes = invited(session.security_attributes);
        let channel = Channel { security_attributes: invited(main_channel(&session).security_attributes), ..main_channel(&session) };
        let session = manager.create(&as_user(&alice), session, vec![channel]).await.unwrap();
        let main = Arc::clone(&session.channels()[0]);

        let uninvited = session.join(&as_user(&carol), carol_id, Some(main.id())).await.expect_err("carol is not let in");
        assert!(refused_by_the_store(&uninvited), "{uninvited}");
        let for_another = session.join(&as_user(&bob), carol_id, None).await.expect_err("bob joins as himself alone");
        assert!(refused_by_the_store(&for_another), "{for_another}");

        session.join(&as_user(&bob), bob_id, Some(main.id())).await.expect("bob is invited");
        assert!(session.session().participants.iter().any(|p| p.actor_id() == bob_id), "in the session");
        assert!(main.members().iter().any(|m| m.id == bob_id), "and in the channel");
        let invitation = AclEntry { subject: Subject::Actor(bob_id), grant: Grant::Join };
        assert!(!session.session().security_attributes.acl.contains(&invitation), "his invitation is used up");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn an_agent_taken_in_later_is_followed_as_one_taken_in_from_the_start() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let system = SecurityContext::system();
        let personality = Personality {
            id: PersonalityId::new(), name: ScopedName::new("plain", Scope::GlobalScope).unwrap(), kind: Default::default(),
            roles: None, description: None, introduction: None, components: Default::default(),
            security_attributes: Default::default(), reasoning_visibility: Default::default(), deleted: None,
        };
        runtime.entity_stores().personalities.create(&system, &personality).await.unwrap();
        let helper = Agent::new(ScopedName::new("helper", Scope::GlobalScope).unwrap(), personality.id, ActorId::None);
        runtime.entity_stores().agents.create(&system, &helper).await.unwrap();
        let actor = ActorId::Agent(helper.id);
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let main = Arc::clone(&session.channels()[0]);
        let mut events = runtime.event_bus().subscribe_all();
        // Running, with the participants it had as it started.
        tokio::time::sleep(Duration::from_millis(50)).await;

        session.add_participant(&ctx, actor, Some((main.id(), HistoryAccess::All))).await.expect("taken in");
        assert!(main.member_ids().contains(&actor), "it runs in the channel");
        // Once the session took in its own announcement, it follows the agent.
        let announced = |events: &[GlobalEvent]| events.iter().any(|e| matches!(e.about::<Session>().and_then(|(_, change)| change.update::<SessionUpdateEvent>()),
            Some(SessionUpdateEvent::ParticipantAdded(p)) if p.actor_id() == actor));
        assert!(announced(&drain(&mut events)), "announced");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(runtime.agent_manager().delete(&ctx, helper.id).await.expect("deleted"));
        let stopped = async { while main.member_ids().contains(&actor) { tokio::time::sleep(Duration::from_millis(10)).await } };
        tokio::time::timeout(Duration::from_secs(2), stopped).await.expect("its actor stops: the session heard of it");
        manager.stop(&system, session.id()).await.unwrap();
    }
}
