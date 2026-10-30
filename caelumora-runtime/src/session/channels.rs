//! The channels of a running session: finding, creating, deleting and restoring them.

use super::{
    purge_channel_document, require_classification, ChannelEntry, ChannelRun, Listed, SessionError,
    SessionLoop,
};
use crate::channel::ChannelLoop;
use crate::component::ComponentError;
use crate::group::keep_participants;
use crate::scoped::ScopeNotEmpty;
use crate::view::{channel_reading, ChannelReader};
use anyhow::{Error, Result};
use caelumora_model::actor::ActorId;
use caelumora_model::agent::AgentId;
use caelumora_model::channel::{self, Channel, Participant};
use caelumora_model::component::ComponentConfig;
use caelumora_model::confidentiality::{Classification, Place};
use caelumora_model::event::{Announcing, GlobalEvent};
use caelumora_model::group::Group;
use caelumora_model::removal::Removal;
use caelumora_model::scope::Scope;
use caelumora_model::security::{ActionType, Enforcement, SecurityContext};
use caelumora_model::session::ChannelId;
use caelumora_model::store::{Deleted, StoreError, StoreResultExt, Stored};
use caelumora_model::template::InternalActorTemplate;
use caelumora_model::value::language::Language;
use caelumora_model::value::name::Name;
use futures::future;
use std::slice;
use std::sync::Arc;
use tracing::{debug, info, warn};

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
    /// The language its conversation is held in (issue 56); `None` for none
    /// in particular. Offering the one its creator prefers is the
    /// frontend's (issue 331).
    pub language: Option<String>,
    /// Whether its participants work on the session's workspace in it (issue
    /// 253); `None` for the default, as in a template.
    pub uses_workspace: Option<bool>,
    /// The actors that belong to the channel itself (issue 420): its
    /// approver, if one is named - none that is not.
    pub internal_actors: Vec<InternalActorTemplate>,
}

/// What `error` of building a channel's loop says if its components cannot be read
/// (`ComponentError::is_unavailable`, issue 463); `error` itself otherwise - a store that failed
/// among it.
fn unavailable_components(error: Error) -> Result<ComponentError, Error> {
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

    /// Has every channel of the session follow whether its agents exist
    /// (issue 314) - `only`, or every agent if `None`: a deleted one rests in
    /// them, a restored one runs again (`ChannelLoop::follow_agents`).
    pub(crate) async fn follow_agents(&self, only: Option<AgentId>) {
        for channel in self.channels() {
            channel.follow_agents(only).await;
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
    /// as `SessionManager::open` opens a session (issues 70, 102):
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
        let (_, run) = self.listed_channel_for(context, channel_id).await?;
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
    /// it - asked as `channel_for` asks it.
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` if there is no such channel, and
    /// equally if `context` may not read it.
    async fn listed_channel_for(
        &self,
        context: &SecurityContext,
        channel_id: ChannelId,
    ) -> Result<Listed> {
        let not_found = || SessionError::ChannelNotFound { session: self.id, channel: channel_id };
        let (channel, run) = self.listed_channel(channel_id).ok_or_else(not_found)?;
        let at = Place::of(&channel, self.privacy());
        channel_reading(
            self.access_policy.as_ref(),
            self.confidentiality.as_ref(),
            context,
            &channel.security_attributes,
            &at,
            // Reached for a user alone, who reads by their clearance.
            ChannelReader::of(None),
        )
        .await
        .ok_or_else(not_found)?;
        Ok((channel, run))
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
    /// whether they may read it - joining it decides (`join`).
    ///
    /// # Errors
    /// `SessionError::ChannelNotFound` if the session lists no such channel,
    /// `SessionError::ChannelUnavailable` if it does not run (issue 463).
    pub(super) fn channel_to_join(&self, channel_id: ChannelId) -> Result<Arc<ChannelLoop>> {
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

    /// Creates a channel in the running session on behalf of `context` and
    /// starts it (issue 09). The channel is a document of its own (issue 70),
    /// stored first (`CreateChannel`), and then listed in the session, which
    /// takes the permission to change the session (`UpdateSession`, asked
    /// by the store); the channel belongs to the session's owner, like the
    /// session's others, and its components are built in that owner's name
    /// (issue 321). In the order of issue 304: what `spec` asks for is
    /// validated - the component bundles it names read in the name of
    /// `context`, who names them -, then creating the channel is asked,
    /// then its participants' admission, and the store asks again as it stores
    /// (issue 312). It is built and stored empty and started; then its
    /// participants join it as its founders, by the one way in
    /// (`ChannelLoop::found`, issue 420) - so that its history records them
    /// as anyone who joins later. If one cannot, the channel is taken back.
    /// It is announced as created once they joined.
    ///
    /// # Errors
    /// - `ValueError` if its name is no name or its language no tag.
    /// - `StoreError::NameTaken` if the session has a channel of that name
    ///   (issue 250).
    /// - [`SessionError`] if `spec` asks for something that cannot be: components that make no
    ///   moderator, or name bundles `context` may not read, or a participant who is not one of the
    ///   session's.
    /// - `StoreError::PermissionDenied` if `context` may not create the
    ///   channel or change the session; nothing is changed then.
    /// - What taking a founder in reports (`ChannelLoop::found`); the channel
    ///   is taken back then.
    pub async fn create_channel(
        self: &Arc<Self>,
        context: &SecurityContext,
        spec: ChannelSpec,
    ) -> Result<Arc<ChannelLoop>> {
        let session = self.session();
        let name = Name::new(&spec.name)?;
        let language = Language::optional(spec.language.as_deref())?;
        let classification = spec.classification.unwrap_or_default();
        require_classification(self.confidentiality.as_ref(), &classification)?;

        // The creator first, if they take part in the session at all.
        let mut ids = vec![context.initiator_id];
        ids.extend(spec.participants);
        let mut participants = Vec::new();
        for id in ids {
            if participants.iter().any(|p: &Participant| p.actor_id() == id) {
                continue;
            }
            match session.participants.iter().find(|p| p.actor_id() == id) {
                Some(participant) => participants.push(participant.clone()),
                None if id == context.initiator_id => {}
                None => {
                    return Err(
                        SessionError::NotAParticipant { session: self.id, participant: id }.into()
                    )
                }
            }
        }

        // Components the channel would not take are refused before the verb
        // (issue 337); and whoever names component bundles names only what
        // they may read themselves, though the channel builds them in
        // its owner's name (issue 321): their own request.
        self.components
            .require_acceptable(&spec.components, ChannelLoop::COMPONENT_FACETS, context)
            .await
            .map_err(SessionError::InvalidComponents)?;

        let mut channel = Channel::build(
            self.id,
            session.security_attributes.owner_id,
            name,
            spec.components,
            classification,
            language,
        );
        channel.internal_actors =
            spec.internal_actors.iter().map(InternalActorTemplate::instantiate).collect();
        if let Some(uses) = spec.uses_workspace {
            channel.uses_workspace = uses;
        }
        // Whether the actor may create it, before anything is asked of the
        // others in it (issues 198, 312): who may not learns nothing of their
        // clearance. The store asks again as it stores.
        self.access_policy
            .require(
                context,
                ActionType::create(Channel::KIND, &channel.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        self.participation.require_admitted(context, &channel, &participants).await?;
        self.stores.channels.create(context, &mut channel).await?;
        let added =
            match keep_participants(self.stores.groups.as_ref(), Group::of_channel(&channel)).await
            {
                Ok(()) => self.session_controller.add_channel(context, channel.id).await,
                Err(e) => Err(e),
            };
        if let Err(error) = added {
            self.roll_back_channel_creation(channel.id).await;
            return Err(error.into());
        }

        let channel_loop = match self.add_channel_loop(&channel).await {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Stored, but it cannot run: rolled back, so that the
                // session does not keep a channel that fails every start.
                if let Err(undo) = self.session_controller.remove_channel(context, channel.id).await
                {
                    warn!(
                        "Channel {} of session {} could not be built and not be removed again: {}",
                        channel.id, self.id, undo
                    );
                }
                self.roll_back_channel_creation(channel.id).await;
                return Err(error);
            }
        };
        for participant in &participants {
            if let Err(error) = channel_loop.found(context, participant.actor_id()).await {
                self.abandon_channel(context, &channel_loop).await;
                return Err(error);
            }
        }
        self.events.announce(
            GlobalEvent::created::<Channel>(&(self.id, channel.id)),
            &*channel_loop.channel(),
        );
        info!("Channel {}/{} created in session {}", channel.name, channel.id, self.id);
        Ok(channel_loop)
    }

    /// Takes back `channel_loop`, created on behalf of `context` and started,
    /// whose founders could not all join (issue 420): its actors leave, its
    /// task ends, and it is taken out of the session and its document
    /// purged - as if it had never been made.
    async fn abandon_channel(&self, context: &SecurityContext, channel_loop: &Arc<ChannelLoop>) {
        let id = channel_loop.id();
        let task = match self.channels.remove(&id) {
            Some((_, ChannelEntry::Running { task, .. })) => task,
            _ => None,
        };
        channel_loop.leave_actors().await;
        if let Some(task) = task {
            task.abort();
        }
        if let Err(undo) = self.session_controller.remove_channel(context, id).await {
            warn!("Channel {} of session {} could not take in its founders and not be removed again: {}", id, self.id, undo);
        }
        self.roll_back_channel_creation(id).await;
    }

    /// Builds the loop of `channel`, which the session lists, and has the
    /// session run it.
    pub(super) async fn add_channel_loop(
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

    /// Has the session run `channel`, which it lists, as it opens
    /// (`add_channel_loop`) - or holds it left out, with a warning, if its
    /// components cannot be read (issue 463): a bundle it names is gone, or
    /// no longer its owner's to read. The owner of a bundle, a third party,
    /// must not keep a session from opening, as the owner of an agent's
    /// personality must not keep its channel from starting.
    ///
    /// # Errors
    /// Whatever else building it reports - a store that failed among it.
    pub(super) async fn add_listed_channel(self: &Arc<Self>, channel: Channel) -> Result<()> {
        let Err(error) = self.add_channel_loop(&channel).await else { return Ok(()) };
        let cause = unavailable_components(error)?;
        warn!(
            "Channel {} of session {} cannot be run and is left out: {}",
            channel.id, self.id, cause
        );
        self.channels.insert(
            channel.id,
            ChannelEntry::Unavailable { channel: Arc::new(channel), cause: Arc::new(cause) },
        );
        Ok(())
    }

    /// Builds again the channels left out as the session opened (issue 463),
    /// as someone opens it while it runs: those whose components can be read
    /// again run from now on, the others stay left out, with why they still
    /// cannot run - also one whose store failed, for the session runs
    /// already. Asked on every request in the session, so logged at
    /// `debug!`. One deleted while it was built is not started.
    pub(crate) async fn retry_unavailable(self: &Arc<Self>) {
        let unavailable: Vec<Arc<Channel>> = self
            .channels
            .iter()
            .filter_map(|entry| match entry.value() {
                ChannelEntry::Unavailable { channel, .. } => Some(Arc::clone(channel)),
                ChannelEntry::Running { .. } => None,
            })
            .collect();
        for channel in unavailable {
            let built = ChannelLoop::new(self.channel_runtime(Channel::clone(&channel))).await;
            let Some(mut entry) = self.channels.get_mut(&channel.id) else { continue };
            let ChannelEntry::Unavailable { cause, .. } = &mut *entry else { continue };
            match built.map_err(unavailable_components) {
                Ok(channel_loop) => {
                    *entry = ChannelEntry::Running { channel: channel_loop, task: None };
                    drop(entry);
                    info!("Channel {} of session {} can be run again", channel.id, self.id);
                    let _ = self.start_tx.send(channel.id);
                }
                Err(Ok(now)) => {
                    debug!(
                        "Channel {} of session {} still cannot be run: {}",
                        channel.id, self.id, now
                    );
                    *cause = Arc::new(now);
                }
                Err(Err(error)) => debug!(
                    "Channel {} of session {} could not be built again: {:#}",
                    channel.id, self.id, error
                ),
            }
        }
    }

    /// Stops and deletes the channel `channel_id` on behalf of `context`.
    /// Deleting marks (issue 164): the channel is taken out of the session
    /// and its document marked, but it and its state stay stored - it can
    /// be restored (`restore_channel`) - until it is purged with its session
    /// (`SessionManager::purge`). Asked about first, in the order
    /// of issue 304 - whether `context` reaches the channel as the views
    /// reach it (`listed_channel_for`), then may delete it (`DeleteChannel`), then,
    /// as it is taken out of the session, change the session
    /// (`UpdateSession`, by the store) - so that a refused deletion leaves
    /// the channel running and listed. A channel that does not run, its
    /// components unreadable (issue 463), is deleted alike.
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such channel,
    ///   and equally if `context` may not read it or is not cleared for it.
    /// - `StoreError::PermissionDenied` if `context` may not delete the
    ///   channel or change the session; nothing is changed then.
    /// - `SessionError::ScopeNotEmpty` if something of its scope is left
    ///   that `context` may not delete (issue 320): what they may is
    ///   deleted, the channel is kept and runs on.
    /// - Whatever else the store reports, the group of its participants
    ///   failing to go among it (issue 348); the channel is kept then too,
    ///   with its group.
    pub async fn delete_channel(
        &self,
        context: &SecurityContext,
        channel_id: ChannelId,
    ) -> Result<()> {
        // Also one that does not run (issue 463): its owner removes it so.
        let (channel, _) = self.listed_channel_for(context, channel_id).await?;
        self.access_policy
            .require(
                context,
                ActionType::delete(Channel::KIND, &channel.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        let scope = Scope::ChannelScope { session_id: self.id, channel_id };
        self.session_controller.remove_channel(context, channel_id).await?;
        // Marked before what lies in its scope, so that nothing of it goes unless the channel's
        // deletion was granted, and nothing is put into its scope afterwards (`ScopeHolders`, issue
        // 458). Then what lies in its scope goes as far as `context` may delete it
        // (issue 320); an agent of it takes part in no other channel (`Scope::admits`). Then the
        // group of its participants, in the runtime's name (issue 348). All of it carries the
        // channel's mark and comes back with it (`restore_channel`, issues 178, 469). If any of it
        // fails, the deletion is rolled back - in the runtime's name: its deletion was permitted -
        // and runs on, without what was deleted but with its group.
        let removal = Removal::now(context.caller_id);
        let deleted = async {
            self.stores.channels.delete(context, channel_id, &removal).await?;
            let left =
                self.scoped.delete_lying_in(context, slice::from_ref(&scope), &removal).await?;
            ScopeNotEmpty::require_empty(&scope, left).map_err(SessionError::from)?;
            self.groups.delete_participants(slice::from_ref(&scope), &removal).await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(error) = deleted {
            self.roll_back_channel_deletion(channel_id).await;
            return Err(error);
        }

        // Whether it runs as it is taken out, not as it was reached: it may
        // have been built again meanwhile (`retry_unavailable`).
        let task = match self.channels.remove(&channel_id) {
            Some((_, ChannelEntry::Running { channel: running, task })) => {
                // Given the time to end by themselves before the channel's
                // task - and with it whoever did not - is aborted (issue
                // 187): they store their state on their way out, which stays.
                running.leave_actors().await;
                task
            }
            _ => None,
        };
        let event_bus = &self.events;
        if let Some(task) = task {
            task.abort();
            event_bus.announce(
                GlobalEvent::custom::<Channel>(&(self.id, channel_id), channel::STOPPED, &()),
                &*channel,
            );
        }
        event_bus.announce(GlobalEvent::deleted::<Channel>(&(self.id, channel_id)), &*channel);
        info!(
            "Channel {}/{} deleted from session {} by {}",
            channel.name, channel_id, self.id, context.caller_id
        );
        Ok(())
    }

    /// Restores the deleted channel `channel_id` of this session on behalf
    /// of `context` (issue 164): its mark taken away, the group of its
    /// participants restored with it in the runtime's name (issue 348),
    /// listed in the session again and started from its stored state. In
    /// the order of issue 304: the deleted channel is read as such (`ReadDeleted`, as the session
    /// lists its deleted channels), then restoring it is asked (`Restore`), then its participants'
    /// clearance, as creating one does; the store asks again as it restores it, and to change the
    /// session that lists it again (`UpdateSession`).
    ///
    /// # Errors
    /// - `SessionError::ChannelNotFound` if the session has no such deleted
    ///   channel, or `context` may not see it.
    /// - `StoreError::NameTaken` if its name was taken since (issue 250).
    /// - `StoreError::PermissionDenied` if `context` may not restore the
    ///   channel or change the session; nothing is changed then.
    /// - Whatever else the store reports, the group of its participants
    ///   failing to come back among it (issue 348); the channel stays
    ///   deleted then.
    pub async fn restore_channel(
        self: &Arc<Self>,
        context: &SecurityContext,
        channel_id: ChannelId,
    ) -> Result<Arc<ChannelLoop>> {
        // What went with it carries its mark (issue 469).
        let of_this = self
            .stores
            .channels
            .get_deleted(context, channel_id)
            .await
            .optional()?
            .filter(|deleted| deleted.entity.session_id == self.id);
        let Some(Deleted { entity: channel, removal }) = of_this else {
            return Err(
                SessionError::ChannelNotFound { session: self.id, channel: channel_id }.into()
            );
        };
        self.access_policy
            .require(
                context,
                ActionType::restore(Channel::KIND, &channel.security_attributes),
                StoreError::PermissionDenied,
            )
            .await?;
        // As when it was created: its participants' clearance may have
        // changed while it was deleted.
        self.participation.require_admitted(context, &channel, &channel.participants).await?;
        let scope = Scope::ChannelScope { session_id: self.id, channel_id };
        let channel = self.stores.channels.restore(context, channel_id).await?;
        // The group of its participants first, with it, in the runtime's
        // name (issue 348): whatever fails later, its participants reach it.
        let listed = async {
            self.groups.restore_participants(slice::from_ref(&scope), &removal).await?;
            self.session_controller.add_channel(context, channel_id).await?;
            self.add_channel_loop(&channel).await
        }
        .await;
        let channel_loop = match listed {
            Ok(channel_loop) => channel_loop,
            Err(error) => {
                // Rolled back: deleted again with its group, marked as it was, so that it is
                // neither half back nor lost, and a restoring tried again finds what went with it.
                let _ = self.session_controller.remove_channel(context, channel_id).await;
                if let Err(undo) =
                    self.groups.delete_participants(slice::from_ref(&scope), &removal).await
                {
                    warn!("Channel {} of session {} could not be restored and its group not be deleted again: {}", channel_id, self.id, undo);
                }
                if let Err(undo) = self
                    .stores
                    .channels
                    .delete(&SecurityContext::system(), channel_id, &removal)
                    .await
                {
                    warn!("Channel {} of session {} could not be restored and not be marked deleted again: {}", channel_id, self.id, undo);
                }
                return Err(error);
            }
        };
        // What was deleted with it comes back with it (issue 178).
        if let Err(e) =
            self.scoped.restore_lying_in(context, slice::from_ref(&scope), &removal).await
        {
            warn!(
                "Channel {} of session {} was restored, but not what lies in its scope: {}",
                channel_id, self.id, e
            );
        }
        self.events.announce(GlobalEvent::restored::<Channel>(&(self.id, channel_id)), &channel);
        info!(
            "Channel {}/{} restored in session {} by {}",
            channel.name, channel_id, self.id, context.caller_id
        );
        Ok(channel_loop)
    }

    /// Rolls back deleting the channel `channel_id`, which failed half-way -
    /// in the runtime's name: its deletion was permitted. Listed in the
    /// session again, last, and its mark taken away, if it was marked. What
    /// cannot be put back is logged.
    async fn roll_back_channel_deletion(&self, channel_id: ChannelId) {
        let system = SecurityContext::system();
        if let Err(e) = self.stores.channels.restore(&system, channel_id).await.optional() {
            warn!(
                "Channel {} of session {} could not be deleted and not be unmarked again: {}",
                channel_id, self.id, e
            );
        }
        if let Err(e) = self.session_controller.add_channel(&system, channel_id).await {
            warn!(
                "Channel {} of session {} could not be deleted and not be listed again: {}",
                channel_id, self.id, e
            );
        }
    }

    /// Rolls back creating the channel `channel_id`, which failed half-way -
    /// in the runtime's name: its creation was permitted. Its document is
    /// purged; if that fails, it is logged.
    async fn roll_back_channel_creation(&self, channel_id: ChannelId) {
        if let Err(e) = purge_channel_document(&self.stores, channel_id).await {
            warn!(
                "Channel {} of session {} could not be deleted again: {}",
                channel_id, self.id, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::moderator::broadcast_policy::BROADCAST_POLICY_KIND;
    use crate::moderator::random_policy::RANDOM_POLICY_KIND;
    use caelumora_model::actor::ActorId;
    use caelumora_model::channel::moderation_components;
    use caelumora_model::channel::testing::main_channel;
    use caelumora_model::command::HELP_COMMAND_KIND;
    use caelumora_model::component::{ComponentConfig, ComponentDescriptor};
    use caelumora_model::session::testing::make_session;
    use caelumora_model::user::testing::{make_user, record_users};

    use crate::channel::ChannelLoop;

    use crate::view::Viewer;
    use anyhow::Result;
    use caelumora_model::approval::ApprovalStrategy;
    use caelumora_model::channel::{Channel, InternalActor, STARTED, STOPPED};
    use caelumora_model::event::EntityChange;
    use caelumora_model::security::{Grant, SecurityAttributes, SecurityContext, Subject};
    use caelumora_model::snapshot::Detail;
    use caelumora_model::store::StoreError;
    use caelumora_model::template::InternalActorTemplate;
    use caelumora_model::value::language::Language;
    use caelumora_model::value::time_zone::TimeZone;
    use std::sync::Arc;

    use super::unavailable_components;
    use crate::component::ComponentError;
    use crate::error::is_forbidden;
    use crate::session::testing::*;
    use crate::session::{ChannelSpec, SessionError, SessionLoop, SessionManager};
    use anyhow::Error;
    use caelumora_model::component::ComponentBundle;
    use caelumora_model::group::GroupId;
    use caelumora_model::scope::Scope;
    use caelumora_model::session::Session;
    use caelumora_model::store::Stored;
    use caelumora_model::user::UserId;
    use caelumora_model::value::name::ScopedName;
    use std::sync::atomic::Ordering;
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
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, other) = running_session_of_a_user(&manager).await;
        let mut rx = runtime.event_bus().subscribe_all();

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
            channel.actor(ActorId::Approver(approver.id)).is_some()
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

        let stored = runtime.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
        assert!(stored.channels.contains(&channel.id()), "the session lists the channel");
        let document = runtime
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
        use caelumora_model::user::UserProfile;
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;
        let language = |tag: &str| Some(Language::new(tag).unwrap());

        let owner = ctx.initiator_id.as_user_id().unwrap();
        let mut profile = UserProfile::new(owner, None);
        profile.preferred_language = language("de");
        runtime.entity_stores().user_profiles.create(&ctx, &mut profile).await.unwrap();
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

        asked.update_language(&ctx, None).await.expect("the owner changes it");
        assert_eq!(asked.channel().language, None);
        let stored = runtime.entity_stores().channels.get(&ctx, asked.id()).await.unwrap();
        assert_eq!(stored.language, None, "stored, not only in memory");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_without_a_language_of_its_own_follows_its_sessions() {
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
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
                .session(session.id())
                .await
                .unwrap()
                .channel(channel.id())
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
        let stored = runtime.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
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
        let runtime = test_runtime();
        let manager = Arc::new(SessionManager::new(Arc::clone(&runtime)));
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
        let stored = runtime.entity_stores().sessions.get(&ctx, session.id()).await.unwrap();
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
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let (session, ctx, _) = running_session_of_a_user(&manager).await;

        let unsaid = session.create_channel(&ctx, spec("unsaid", vec![])).await.unwrap();
        assert!(unsaid.channel().uses_workspace, "by default");
        let without = session
            .create_channel(
                &ctx,
                ChannelSpec { uses_workspace: Some(false), ..spec("without", vec![]) },
            )
            .await
            .unwrap();
        assert!(!without.channel().uses_workspace, "as its creator says");
        let stored = runtime.entity_stores().channels.get(&ctx, without.id()).await.unwrap();
        assert!(!stored.uses_workspace, "stored, not only in memory");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_that_cannot_be_is_refused_before_anything_is_stored() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
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
        let command = ComponentConfig {
            component_nodes: vec![ComponentDescriptor::new("help", HELP_COMMAND_KIND)],
        };
        assert!(matches!(refusal(session.create_channel(&ctx, ChannelSpec { components: command, ..spec("x", vec![]) }).await), SessionError::InvalidComponents(_)), "a channel takes no component it does not ask for, a command of the session (issue 337)");

        assert_eq!(session.channels().len(), 1);
        assert_eq!(
            runtime.entity_stores().sessions.get(&ctx, session.id()).await.unwrap().channels.len(),
            1
        );
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
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
        runtime
            .state_store()
            .put_agent_channel_state(agent, id, &mut AgentChannelState::default())
            .await
            .unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let mut rx = runtime.event_bus().subscribe_all();

        session.delete_channel(&ctx, id).await.expect("the owner deletes it");

        assert!(session.channel(id).is_none());
        assert!(!runtime
            .entity_stores()
            .sessions
            .get(&ctx, session.id())
            .await
            .unwrap()
            .channels
            .contains(&id));
        assert!(runtime.entity_stores().channels.get(&ctx, id).await.is_err(), "not there");
        assert!(
            runtime.entity_stores().channels.get_deleted(&ctx, id).await.is_ok(),
            "but kept, marked (issue 164)"
        );
        assert!(runtime.state_store().get_channel_state(id).await.is_ok(), "with its state");
        let events = rx.drain().await;
        let stopped = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Custom { name: STOPPED, .. })) if cid == id));
        let deleted = events.iter().position(|e| matches!(e.about::<Channel>(), Some(((_, cid), EntityChange::Deleted)) if cid == id));
        assert!(
            stopped.is_some() && deleted.is_some() && stopped < deleted,
            "stopped, then deleted: {events:?}"
        );

        let missing = session.delete_channel(&ctx, id).await.expect_err("gone");
        assert!(matches!(
            missing.downcast_ref::<SessionError>(),
            Some(SessionError::ChannelNotFound { .. })
        ));

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
        session.delete_channel(&ctx, taken.id()).await.unwrap();
        session.restore_channel(&ctx, id).await.expect("restored");
        assert!(session.channel(id).is_some());
        assert!(
            runtime
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
        assert!(runtime.state_store().get_channel_state(id).await.is_ok());
        let again = session.restore_channel(&ctx, id).await.expect_err("not deleted");
        assert!(
            matches!(
                again.downcast_ref::<SessionError>(),
                Some(SessionError::ChannelNotFound { .. })
            ),
            "{again}"
        );
        session.delete_channel(&ctx, id).await.unwrap();

        // The last channel may go too; the session keeps running without.
        let main = session.channel_named("main").unwrap().id();
        session.delete_channel(&ctx, main).await.expect("the last channel may be deleted");
        assert!(session.channels().is_empty());
        session.create_channel(&ctx, spec("again", vec![])).await.expect("and a new one created");
        manager.stop(&SecurityContext::system(), session.id()).await.unwrap();
    }

    #[tokio::test]
    async fn a_channel_deleted_before_the_session_runs_is_never_started() {
        let runtime = test_runtime();
        let manager = SessionManager::new(Arc::clone(&runtime));
        let ctx = SecurityContext::default();
        let session = make_session("waiting", UserId::new());
        let channel_id = session.channels[0];
        let mut channels = [main_channel(&session)];
        let session = persist(&manager, &ctx, session, &mut channels).await;
        let session_loop =
            SessionLoop::new(session.clone(), channels.to_vec(), Arc::clone(&runtime))
                .await
                .unwrap();
        let mut rx = runtime.event_bus().subscribe_all();

        // Queued to be started, but deleted before `run` gets to it.
        session_loop.delete_channel(&ctx, channel_id).await.expect("deleted");
        let running = tokio::spawn({
            let session_loop = Arc::clone(&session_loop);
            async move { session_loop.run().await }
        });
        time::sleep(Duration::from_millis(50)).await;

        let events = rx.drain().await;
        let about = |wanted: fn(&EntityChange) -> bool| {
            events.iter().any(|e| matches!(e.about::<Channel>(), Some(((_, id), change)) if id == channel_id && wanted(change)))
        };
        assert!(about(|change| *change == EntityChange::Deleted), "{events:?}");
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
        let (_, manager) = owned_runtime(Default::default());

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
            record_users(manager.runtime().entity_stores(), &[&alice, &bob]).await;
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
        let (_, manager) = owned_runtime(Default::default());
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
            record_users(manager.runtime().entity_stores(), &[&alice, &bob]).await;
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
        let (runtime, passing) = failing_runtime(Session::KIND);
        let stores = runtime.entity_stores().clone();
        let manager = SessionManager::new(runtime);
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
        session.delete_channel(&ctx, side).await.unwrap();
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
