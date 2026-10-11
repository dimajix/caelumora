//! The channels of a running session: finding, creating, deleting and restoring them.

use caelumora_model::value::name::Name;
use crate::channel::ChannelLoop;
use crate::runtime::ChannelRuntime;
use anyhow::Result;
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::Channel;
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::value::language::Language;
use caelumora_model::event::GlobalEvent;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext};
use crate::view::{channel_reading, ChannelReader};
use caelumora_model::store::{StoreError, StoreResultExt};
use tracing::{info, warn};
use caelumora_model::scope::Scope;
use caelumora_model::session::ChannelId;
use caelumora_model::group::Group;
use crate::group::keep_participants;
use crate::scoped::{delete_scoped_in, restore_scoped_in};
use std::sync::Arc;
use super::{ChannelEntry, SessionError, SessionLoop, preferred_language, purge_channel_document, require_classification};

/// What a new channel should be (issue 09), as it was asked: its name and
/// language are made from it by `create_channel` (issue 248).
#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub name: String,
    /// The channel's components (issue 174): its moderator and the policy
    /// for the moderator's slot; the defaults for what is left out, as in a
    /// template.
    pub components: ComponentConfig,
    /// Who takes part, from the session's participants. Whoever creates the
    /// channel takes part anyway, if they are a participant of the session.
    pub participants: Vec<ActorId>,
    /// The classification of the channel's contents (issue 133); `None`
    /// for the default, as in a template. Set by whoever may create the
    /// channel - its owner.
    pub classification: Option<Classification>,
    /// The language its conversation is held in (issue 56); `None` for the
    /// one its creator prefers, if they prefer one.
    pub language: Option<String>,
    /// Whether its members work on the session's workspace in it (issue
    /// 253); `None` for the default, as in a template.
    pub uses_workspace: Option<bool>,
}

impl SessionLoop {
    /// The running channels, in the order of the session: those from its
    /// template first, then those created later, oldest first.
    pub fn channels(&self) -> Vec<Arc<ChannelLoop>> {
        self.session().channels.iter().filter_map(|c| self.channel_by_id(*c)).collect()
    }

    /// Has every channel of the session follow whether its agents exist
    /// (issue 314) - `only`, or every agent if `None`: a deleted one rests in
    /// them, a restored one runs again (`ChannelLoop::follow_agents`).
    pub(crate) async fn follow_agents(&self, only: Option<AgentId>) {
        for channel in self.channels() {
            channel.follow_agents(only).await;
        }
    }

    pub fn channel_by_id(&self, channel_id: ChannelId) -> Option<Arc<ChannelLoop>> {
        self.channels.get(&channel_id).map(|entry| Arc::clone(&entry.channel))
    }

    /// The channel `channel_id`, if the user `context` acts for may read it,
    /// as `SessionManager::open_session` opens a session (issues 70, 102):
    /// reached as the views reach a channel for a user (`channel_reading`,
    /// issue 304), by `GetChannel` and their clearance for it as a whole.
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` if there is no such channel, and
    /// equally if `context` may not read it, so that channels cannot be
    /// probed.
    pub async fn channel_for(&self, context: &SecurityContext, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let channel = self.channel_by_id(channel_id).ok_or_else(not_found)?;
        let at = Place { channel: channel_id, classification: channel.classification(), session: self.privacy() };
        channel_reading(self.security.as_ref(), self.confidentiality.as_ref(), context, &channel.security_attributes(), &at, ChannelReader::Cleared).await
            .ok_or_else(not_found)?;
        Ok(channel)
    }

    pub fn channel_by_name(&self, name: &str) -> Option<Arc<ChannelLoop>> {
        self.channels.iter().find(|entry| entry.channel.name() == name).map(|entry| Arc::clone(&entry.channel))
    }

    /// Creates a channel in the running session on behalf of `context` and
    /// starts it (issue 09). The channel is a document of its own (issue 70),
    /// stored first (`CreateChannel`), and then listed in the session, which
    /// takes the permission to change the session (`UpdateSession`); the
    /// channel belongs to the session's owner, like the session's others.
    ///
    /// # Errors
    /// - `ValueError` if its name is no name or its language no tag.
    /// - `StoreError::NameTaken` if the session has a channel of that name
    ///   (issue 250).
    /// - [`SessionError`] if `spec` asks for something that cannot be:
    ///   components that make no moderator, or a participant who is not one
    ///   of the session's.
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session; nothing is changed then.
    pub async fn create_channel(self: &Arc<Self>, context: &SecurityContext, spec: ChannelSpec) -> Result<Arc<ChannelLoop>> {
        let session = Arc::clone(&self.session());
        // Whether the actor may change the session at all, before anything
        // is asked of the others in the channel (issue 198, F2): who may not
        // learns nothing of their clearance.
        self.security
            .require(context, ActionType::UpdateSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        let name = Name::new(&spec.name)?;
        let language = Language::optional(spec.language.as_deref())?;
        let classification = spec.classification.unwrap_or_default();
        require_classification(&self.runtime_manager, &classification)?;
        let language = match language {
            Some(language) => Some(language),
            None => preferred_language(self.entities.as_ref(), context).await?,
        };

        // The creator first, if they take part in the session at all.
        let mut ids = vec![context.initiator_id];
        ids.extend(spec.participants);
        let mut participants = Vec::new();
        for id in ids {
            if participants.iter().any(|p: &caelumora_model::channel::Participant| p.actor_id() == id) {
                continue;
            }
            match session.participants.iter().find(|p| p.actor_id() == id) {
                Some(participant) => participants.push(participant.clone()),
                None if id == context.initiator_id => {}
                None => return Err(SessionError::NotAParticipant { session: self.id, participant: id }.into()),
            }
        }

        let mut channel = Channel::build(self.id, session.security_attributes.owner_id, name, spec.components, classification, language, participants);
        if let Some(uses) = spec.uses_workspace {
            channel.uses_workspace = uses;
        }
        crate::channel::require_admitted(&self.runtime_manager, context, &channel).await?;
        let store = &self.entities;
        store.create_channel(context, &channel).await?;
        let added = match keep_participants(store.as_ref(), Group::of_channel(&channel)).await {
            Ok(()) => self.session_controller.add_channel(context, channel.id).await,
            Err(e) => Err(e),
        };
        if let Err(error) = added {
            self.discard_channel(channel.id).await;
            return Err(error.into());
        }

        let channel_loop = match self.add_channel_loop(&channel).await {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Stored, but it cannot run: taken back out, so that the
                // session does not keep a channel that fails every start.
                if let Err(undo) = self.session_controller.remove_channel(context, channel.id).await {
                    warn!("Channel {} of session {} could not be built and not be removed again: {}", channel.id, self.id, undo);
                }
                self.discard_channel(channel.id).await;
                return Err(error);
            }
        };
        self.events.publish(GlobalEvent::ChannelCreated(self.id, channel.id), channel.security_attributes.clone());
        info!("Channel {}/{} created in session {}", channel.name, channel.id, self.id);
        Ok(channel_loop)
    }

    /// Builds the loop of `channel`, which the session lists, and has the
    /// session run it.
    async fn add_channel_loop(self: &Arc<Self>, channel: &Channel) -> Result<Arc<ChannelLoop>> {
        let runtime = ChannelRuntime::new(Arc::clone(&self.runtime_manager), Arc::clone(&self.commands), Arc::clone(&self.session_controller));
        let channel_loop = ChannelLoop::new(channel.clone(), runtime).await?;
        channel_loop.attach(self);
        self.channels.insert(channel.id, ChannelEntry { channel: Arc::clone(&channel_loop), task: None });
        let _ = self.start_tx.send(channel.id);
        Ok(channel_loop)
    }

    /// Stops and deletes the channel `channel_id` on behalf of `context`.
    /// Deleting marks (issue 164): the channel is taken out of the session
    /// and its document marked, but it and its state stay stored - it can
    /// be restored (`restore_channel`) - until it is purged with its session
    /// (`SessionManager::purge_session`). Asked about first, in the order
    /// of issue 304 - whether `context` may read the channel (`GetChannel`),
    /// then delete it (`DeleteChannel`), then, as it is taken out of the
    /// session, change the session (`UpdateSession`, by the store) - so that
    /// a refused deletion leaves the channel running and listed.
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such channel,
    ///   and equally if `context` may not read it.
    /// - `StoreError::PermissionDenied` if `context` may not delete the
    ///   channel or change the session; nothing is changed then.
    pub async fn delete_channel(&self, context: &SecurityContext, channel_id: ChannelId) -> Result<()> {
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let running = self.channel_by_id(channel_id).ok_or_else(not_found)?;
        // Not the guard itself: it would be held across the awaits below.
        let channel = Arc::clone(&running.channel());
        self.security.require(context, ActionType::GetChannel(&channel.security_attributes), not_found).await?;
        self.security.require(context, ActionType::DeleteChannel(&channel.security_attributes), || StoreError::PermissionDenied).await?;
        self.session_controller.remove_channel(context, channel_id).await?;
        // Permitted, so only the store itself can fail here: the channel is
        // out of the session, and stays so.
        if let Err(e) = self.entities.delete_channel(context, channel_id).await {
            warn!("Channel {} was taken out of session {} but not marked deleted: {}", channel_id, self.id, e);
        }

        let task = self.channels.remove(&channel_id).and_then(|(_, entry)| entry.task);
        // Given the time to end by themselves before the channel's task -
        // and with it whoever did not - is aborted (issue 187): they store
        // their state on their way out, which stays.
        running.stop_actors().await;
        let event_bus = &self.events;
        if let Some(task) = task {
            task.abort();
            event_bus.publish(GlobalEvent::ChannelStopped(self.id, channel_id), channel.security_attributes.clone());
        }
        event_bus.publish(GlobalEvent::ChannelDeleted(self.id, channel_id), channel.security_attributes.clone());
        // What lies in its scope goes with it (issue 178); an agent of it
        // takes part in no other channel (`Scope::admits`).
        if let Err(e) = delete_scoped_in(self.entities.as_ref(), event_bus, &Scope::ChannelScope { session_id: self.id, channel_id }).await {
            warn!("Channel {} was deleted from session {}, but not what lies in its scope: {}", channel_id, self.id, e);
        }
        info!("Channel {}/{} deleted from session {} by {}", channel.name, channel_id, self.id, context.caller_id);
        Ok(())
    }

    /// Restores the deleted channel `channel_id` of this session on behalf
    /// of `context` (issue 164): its mark taken away, listed in the session
    /// again and started from its stored state. Asks to change the session
    /// (`UpdateSession`) and its participants' clearance, as creating one
    /// does, and the store asks to restore it (`Restore`).
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such deleted
    ///   channel, or `context` may not see it.
    /// - `StoreError::NameTaken` if its name was taken since (issue 250).
    /// - `StoreError::PermissionDenied` if `context` may not change the
    ///   session or restore the channel; nothing is changed then.
    pub async fn restore_channel(self: &Arc<Self>, context: &SecurityContext, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
        let session = Arc::clone(&self.session());
        self.security
            .require(context, ActionType::UpdateSession(&session.security_attributes), || StoreError::PermissionDenied).await?;
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let store = &self.entities;
        let channel = match store.get_deleted_channel(context, channel_id).await.optional()? {
            Some(channel) if channel.session_id == self.id => channel,
            _ => return Err(not_found().into()),
        };
        // As when it was created: its participants' clearance may have
        // changed while it was deleted.
        crate::channel::require_admitted(&self.runtime_manager, context, &channel).await?;
        let deleted_at = channel.deleted.as_ref().map(|removal| removal.at);
        let channel = store.restore_channel(context, channel_id).await?;
        let listed = match self.session_controller.add_channel(context, channel_id).await {
            Ok(()) => self.add_channel_loop(&channel).await,
            Err(error) => Err(error.into()),
        };
        let channel_loop = match listed {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Deleted again, so that it is neither half back nor lost.
                let _ = self.session_controller.remove_channel(context, channel_id).await;
                if let Err(undo) = store.delete_channel(&SecurityContext::system(), channel_id).await {
                    warn!("Channel {} of session {} could not be restored and not be marked deleted again: {}", channel_id, self.id, undo);
                }
                return Err(error);
            }
        };
        if let Some(since) = deleted_at {
            // What was deleted with it comes back with it (issue 178).
            if let Err(e) = restore_scoped_in(self.entities.as_ref(), &self.events, &Scope::ChannelScope { session_id: self.id, channel_id }, since).await {
                warn!("Channel {} of session {} was restored, but not what lies in its scope: {}", channel_id, self.id, e);
            }
        }
        self.events.publish(GlobalEvent::ChannelRestored(self.id, channel_id), channel.security_attributes.clone());
        info!("Channel {}/{} restored in session {} by {}", channel.name, channel_id, self.id, context.caller_id);
        Ok(channel_loop)
    }

    /// Deletes the stored channel `channel_id` again after creating it failed
    /// half-way - in the runtime's name: its creation was permitted.
    async fn discard_channel(&self, channel_id: ChannelId) {
        if let Err(e) = purge_channel_document(self.entities.as_ref(), channel_id).await {
            warn!("Channel {} of session {} could not be deleted again: {}", channel_id, self.id, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use caelumora_model::actor::ActorId;

    use crate::channel::ChannelLoop;

    use anyhow::Result;
    use caelumora_model::value::language::Language;
    use caelumora_model::event::GlobalEvent;
    use caelumora_model::security::SecurityContext;
    use caelumora_model::store::StoreError;
    use caelumora_model::session::ChannelId;
    use std::sync::Arc;

    
    use crate::session::{ChannelSpec, SessionError, SessionLoop, SessionManager};
    use crate::session::testing::*;

    #[tokio::test]
    async fn a_channel_created_in_a_running_session_is_stored_announced_and_runs() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let mut rx = runtime.event_bus().subscribe_all();

        let channel = session.create_channel(&ctx, spec("side", vec![other])).await.expect("the channel is created");

        assert_eq!(channel.name(), "side");
        let kinds: Vec<String> = channel.members().into_iter().map(|m| m.kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "user").count(), 2, "the creator and the other user: {kinds:?}");
        assert!(kinds.iter().any(|k| k == "approver"), "an approver is added: {kinds:?}");
        assert!(session.channel_by_id(channel.id()).is_some());
        let names: Vec<String> = session.channels().iter().map(|c| c.name().to_string()).collect();
        assert_eq!(names, ["main", "side"], "in the order of the session");

        let stored = runtime.entity_store().get_session(&ctx, session.id()).await.unwrap();
        assert!(stored.channels.contains(&channel.id()), "the session lists the channel");
        let document = runtime.entity_store().get_channel(&ctx, channel.id()).await.expect("stored as a document of its own");
        assert_eq!(document.name, "side");

        // `run` starts it asynchronously.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let events = drain(&mut rx);
        let created = events.iter().position(|e| matches!(e, GlobalEvent::ChannelCreated(_, cid) if *cid == channel.id()));
        let started = events.iter().position(|e| matches!(e, GlobalEvent::ChannelStarted(_, cid) if *cid == channel.id()));
        assert!(created.is_some() && started.is_some() && created < started, "created, then started: {events:?}");

        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_is_held_in_the_language_its_creator_prefers_unless_asked_otherwise() {
        use caelumora_model::user::UserProfile;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let language = |tag: &str| Some(Language::new(tag).unwrap());

        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert_eq!(unsaid.channel().language, None, "no profile, no language");

        let owner = ctx.initiator_id.as_user_id().unwrap();
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = language("de");
        runtime.entity_store().create_user_profile(&ctx, &profile).await.unwrap();
        let preferred = session.create_channel(&ctx, spec("preferred", vec![])).await.unwrap();
        assert_eq!(preferred.channel().language, language("de"), "the one the creator prefers");
        let asked = session.create_channel(&ctx, ChannelSpec { language: Some("en".to_string()), ..spec("asked", vec![]) }).await.unwrap();
        assert_eq!(asked.channel().language, language("en"), "unless another is asked for");

        asked.update_language(&ctx, None).await.expect("the owner changes it");
        assert_eq!(asked.channel().language, None);
        let stored = runtime.entity_store().get_channel(&ctx, asked.id()).await.unwrap();
        assert_eq!(stored.language, None, "stored, not only in memory");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_uses_the_workspace_unless_its_creator_says_otherwise() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;

        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert!(unsaid.channel().uses_workspace, "by default");
        let without = session.create_channel(&ctx, ChannelSpec { uses_workspace: Some(false), ..spec("without", vec![]) }).await.unwrap();
        assert!(!without.channel().uses_workspace, "as its creator says");
        let stored = runtime.entity_store().get_channel(&ctx, without.id()).await.unwrap();
        assert!(!stored.uses_workspace, "stored, not only in memory");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_is_refused_before_anything_is_stored() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let stranger = caelumora_model::actor::ActorId::User(caelumora_model::user::UserId::new());

        let taken = session.create_channel(&ctx, spec("MAIN", vec![])).await.expect_err("its name is taken");
        assert!(matches!(taken.downcast_ref::<StoreError>(), Some(StoreError::NameTaken { entity: "channel" })), "by the store, as names are compared: {taken}");
        let refusal = |result: Result<Arc<ChannelLoop>>| result.expect_err("refused").downcast::<SessionError>().expect("a SessionError");
        assert!(matches!(refusal(session.create_channel(&ctx, spec("x", vec![stranger])).await), SessionError::NotAParticipant { .. }));
        let unknown = ChannelSpec { components: caelumora_model::channel::moderation_components(None, Some("nobody")), ..spec("x", vec![]) };
        assert!(matches!(refusal(session.create_channel(&ctx, unknown).await), SessionError::InvalidModerator(_)));
        let two = caelumora_model::component::ComponentConfig { component_nodes: vec![
            caelumora_model::component::ComponentDescriptor::new("one", crate::moderator::broadcast_policy::BROADCAST_POLICY_KIND),
            caelumora_model::component::ComponentDescriptor::new("two", crate::moderator::random_policy::RANDOM_POLICY_KIND),
        ] };
        assert!(matches!(refusal(session.create_channel(&ctx, ChannelSpec { components: two, ..spec("x", vec![]) }).await), SessionError::InvalidModerator(_)), "a channel has one policy");

        assert_eq!(session.channels().len(), 1);
        assert_eq!(runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.len(), 1);
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_deleted_channel_is_stopped_announced_and_kept_until_it_is_restored() {
        use caelumora_model::agent::{AgentChannelState, AgentId};
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let channel = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let id = channel.id();
        let agent = AgentId::new();
        runtime.state_store().put_agent_channel_state(agent, id, &mut AgentChannelState::default()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut rx = runtime.event_bus().subscribe_all();

        session.delete_channel(&ctx, id).await.expect("the owner deletes it");

        assert!(session.channel_by_id(id).is_none());
        assert!(!runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.contains(&id));
        assert!(runtime.entity_store().get_channel(&ctx, id).await.is_err(), "not there");
        assert!(runtime.entity_store().get_deleted_channel(&ctx, id).await.is_ok(), "but kept, marked (issue 164)");
        assert!(runtime.state_store().get_channel_state(id).await.is_ok(), "with its state");
        let events = drain(&mut rx);
        let stopped = events.iter().position(|e| matches!(e, GlobalEvent::ChannelStopped(_, cid) if *cid == id));
        let deleted = events.iter().position(|e| matches!(e, GlobalEvent::ChannelDeleted(_, cid) if *cid == id));
        assert!(stopped.is_some() && deleted.is_some() && stopped < deleted, "stopped, then deleted: {events:?}");

        let missing = session.delete_channel(&ctx, id).await.expect_err("gone");
        assert!(matches!(missing.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })));

        // Restored: listed and running again, from the state it kept.
        let taken = session.create_channel(&ctx, spec("side", vec![])).await.unwrap();
        let refused = session.restore_channel(&ctx, id).await.expect_err("its name is taken");
        assert!(matches!(refused.downcast_ref::<StoreError>(), Some(StoreError::NameTaken { entity: "channel" })), "{refused}");
        session.delete_channel(&ctx, taken.id()).await.unwrap();
        session.restore_channel(&ctx, id).await.expect("restored");
        assert!(session.channel_by_id(id).is_some());
        assert!(runtime.entity_store().get_session(&ctx, session.id()).await.unwrap().channels.contains(&id), "listed again");
        assert!(drain(&mut rx).iter().any(|e| matches!(e, GlobalEvent::ChannelRestored(_, cid) if *cid == id)), "and announced");
        assert!(runtime.state_store().get_channel_state(id).await.is_ok());
        let again = session.restore_channel(&ctx, id).await.expect_err("not deleted");
        assert!(matches!(again.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })), "{again}");
        session.delete_channel(&ctx, id).await.unwrap();

        // The last channel may go too; the session keeps running without.
        let main = session.channel_by_name("main").unwrap().id();
        session.delete_channel(&ctx, main).await.expect("the last channel may be deleted");
        assert!(session.channels().is_empty());
        session.create_channel(&ctx, spec("again", vec![])).await.expect("and a new one created");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_deleted_before_the_session_runs_is_never_started() {
        let runtime = test_runtime();
        let ctx = SecurityContext::default();
        let session = make_session("waiting", ActorId::None);
        let channel_id = session.channels[0];
        persist(&runtime, &ctx, &session).await;
        let session_loop = SessionLoop::new(session.clone(), vec![main_channel(&session)], Arc::clone(&runtime)).await.unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        // Queued to be started, but deleted before `run` gets to it.
        session_loop.delete_channel(&ctx, channel_id).await.expect("deleted");
        let running = tokio::spawn({ let session_loop = Arc::clone(&session_loop); async move { session_loop.run().await } });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let events = drain(&mut rx);
        let about = |wanted: fn(&GlobalEvent) -> Option<ChannelId>| events.iter().any(|e| wanted(e) == Some(channel_id));
        assert!(about(|e| match e { GlobalEvent::ChannelDeleted(_, id) => Some(*id), _ => None }), "{events:?}");
        assert!(!about(|e| match e { GlobalEvent::ChannelStarted(_, id) => Some(*id), _ => None }), "never started: {events:?}");
        assert!(!about(|e| match e { GlobalEvent::ChannelStopped(_, id) => Some(*id), _ => None }), "so never stopped: {events:?}");
        running.abort();
    }

    #[tokio::test]
    async fn only_who_may_change_the_session_creates_or_deletes_its_channels() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserId;

        let (_, manager) = owned_runtime(Default::default());

        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let (alice_ctx, bob_ctx) = (as_user(alice.id), as_user(bob.id));
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(ActorId::User(bob.id)), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &alice_ctx, session).await.unwrap();
        let main = session.channel_by_name("main").unwrap().id();

        let refused = session.create_channel(&bob_ctx, spec("bobs", vec![])).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        let refused = session.delete_channel(&bob_ctx, main).await.expect_err("a contributor may not");
        assert!(crate::error::is_forbidden(&refused), "{refused}");
        assert!(session.channel_by_id(main).is_some(), "a refused deletion leaves the channel");

        session.create_channel(&alice_ctx, spec("hers", vec![])).await.expect("the owner may");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_its_viewer_may_not_read_is_not_there_for_them() {
        use caelumora_model::actor::ActorId;
        use caelumora_model::user::UserId;

        let (_, manager) = owned_runtime(Default::default());
        let (alice, bob) = (user("alice"), user("bob"));
        let as_user = |id: UserId| SecurityContext::new(ActorId::User(id), Default::default(), Default::default());
        let mut session = make_session("hers", ActorId::User(alice.id));
        session.security_attributes = session.security_attributes.clone().granting(caelumora_model::security::Subject::Actor(ActorId::User(bob.id)), caelumora_model::security::Grant::Read);
        session.participants = record_users(&manager, &[&alice, &bob]).await;
        let session = create(&manager, &as_user(alice.id), session).await.unwrap();
        // Alice's alone: bob takes part in the session, not in this channel.
        let hers = session.create_channel(&as_user(alice.id), spec("hers", vec![])).await.unwrap();

        let hidden = session.channel_for(&as_user(bob.id), hers.id()).await.expect_err("not bob's to read");
        assert!(matches!(hidden.downcast_ref::<SessionError>(), Some(SessionError::ChannelNotFound { .. })), "absent, not refused: {hidden}");
        assert!(session.channel_for(&as_user(alice.id), hers.id()).await.is_ok(), "the owner reads it");
        manager.stop_session(&SecurityContext::system(), session.id()).await.unwrap();
    }
}
