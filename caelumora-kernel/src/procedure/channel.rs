//! The procedures of a channel's lifecycle (issue 593): creating a channel with its founders,
//! deleting and restoring it, building again the channels left out as their session started, and
//! reclassifying one.
//!
//! Each locks the channels it changes exclusively (`Key::Channel`) and the participants it takes in
//! or lets go; each but building again, which opens no session, locks its session as opening it
//! does - shared where it runs, exclusively where it starts it. Each carries the change out by the
//! services of the kernel, the session's controller and the session's loop, which runs the
//! channels: asked first, in the order of issue 304, then one step after the other, each that can
//! be set back undone by its compensation if a later one fails - not what deleting a channel
//! deleted of its scope, nor whom restoring it let go. Whoever takes someone into a channel or lets
//! them go holds it shared (`participant`), so that neither crosses a channel being made, deleted,
//! restored, built again or reclassified.

use super::participant::{let_go_where_not_running, take_in, Entry};
use super::session::{self, lock_to_open};
use super::{sessions_of, Compensation, Deferred, Key, Locks, Procedure};
use crate::channel::{ChannelLoop, ResolvedParticipant};
use crate::kernel::Kernel;
use crate::scoped::ScopeNotEmpty;
use crate::session::{
    require_classification, unavailable_components, ChannelRun, ChannelSpec, SessionError,
    SessionLoop, SessionManager,
};
use anyhow::Result;
use async_trait::async_trait;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{Channel, ChannelId, HistoryAccess, Participant, ThreadId};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::group::{Group, GroupId};
use caelumora_domain::removal::Removal;
use caelumora_domain::scope::Scope;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::{StoreError, StoreResultExt, Stored};
use caelumora_domain::template::InternalActorTemplate;
use caelumora_domain::value::language::Language;
use caelumora_domain::value::name::Name;
use caelumora_security::{ActionType, Enforcement, SecurityContext};
use caelumora_store::Deleted;
use std::iter;
use std::slice;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Creates a channel in a session on behalf of `context`, starting the session where it does not
/// run, with its founders - its participants, the creator first if in the session -, who join it by
/// the one way in once it is started (`participant::take_in`); taken back if one cannot. Holds the
/// channel, its id chosen before it is built, until it is complete, so that nobody takes someone
/// into it before its founders.
pub struct Create {
    context: SecurityContext,
    session: SessionId,
    /// Chosen before it is built, so that it is held before anyone can find it.
    id: ChannelId,
    spec: ChannelSpec,
}

impl Create {
    /// Creates the channel `spec` asks for in `session` on behalf of `context`.
    pub fn new(context: &SecurityContext, session: SessionId, spec: ChannelSpec) -> Self {
        Self { context: context.clone(), session, id: ChannelId::new(), spec }
    }
}

#[async_trait]
impl Procedure for Create {
    type Plan = ();
    type Output = Arc<ChannelLoop>;
    type Error = SessionError;

    /// The founders it may take in: the creator and those `spec` names.
    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.id))?;
        let founders =
            iter::once(self.context.initiator_id).chain(self.spec.participants.iter().copied());
        for founder in founders {
            locks.exclusive(Key::Participant(self.session, founder))?;
        }
        Ok(())
    }

    /// Asked before anything is stored (`asked_for`); then the group of its participants made in
    /// the kernel's name (issue 348) - first, so that whoever it lets read the channel hears it
    /// created -, the channel stored (`CreateChannel`, asked by the store) and announced as created
    /// (`SessionManager::channel_lifecycle`), listed in the session last - which asks whether
    /// `context` may change it (`UpdateSession`) - and run, each step taken back by `log`, the last
    /// first, if a later one fails: stopped, taken off the session's list, marked deleted and
    /// announced so, and purged with its group, in the kernel's name, its creation being
    /// permitted. Its founders join it by the one way in once it runs (`participant::take_in`,
    /// issue 420), so that its history records them as anyone who joins later.
    ///
    /// # Errors
    /// As `session::open` and `asked_for`; `StoreError::NameTaken` if the session has
    /// a channel of that name (issue 250), `StoreError::PermissionDenied` if `context` may not
    /// create it or change the session; whatever building it and taking a founder in report.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        let Self { context, session, id, spec } = self;
        let sessions = sessions_of(kernel)?;
        let session = session::open(kernel, &context, session).await?;
        let (mut document, founders) = asked_for(kernel, &session, &context, id, spec).await?;
        // Its group first, so that whoever it lets read the channel hears it created.
        let groups = kernel.group_manager();
        groups.keep_participants(Group::of_channel(&document)).await?;
        {
            // Its creation was permitted: taken back in the kernel's name.
            let groups = Arc::clone(groups);
            log.record("keeping a channel's group", async move {
                Ok(groups.discard_participants(GroupId::of_channel(id)).await?)
            });
        }
        sessions.channel_lifecycle().create(&context, &mut document).await?;
        {
            // Announced as deleted, as it was announced created.
            let (sessions, made) = (Arc::clone(&sessions), document.clone());
            log.record("storing a channel", async move {
                Ok(sessions.channel_lifecycle().discard(&made).await?)
            });
        }
        session.controller().add_channel(&context, id).await?;
        {
            let session = Arc::clone(&session);
            log.record("listing a channel", async move {
                Ok(session.controller().remove_channel(&SecurityContext::system(), id).await?)
            });
        }
        let channel = session.add_channel(&document).await?;
        {
            let session = Arc::clone(&session);
            log.record("starting a channel", async move {
                session.remove_channel(id).await;
                Ok(())
            });
        }
        let into = Some((&channel, HistoryAccess::None, ThreadId::line(channel.id())));
        for founder in founders {
            take_in(log, kernel, &session, &context, founder, Entry::Founding, into).await?;
        }
        info!("Channel {}/{} created in session {}", document.name, id, session.id());
        Ok(channel)
    }
}

/// The channel `spec` asks for, built as `id` and empty, with its founders - its participants, the
/// creator first if in the session -, once it is asked whether it can be and `context` may create
/// it (issue 09). The channel is a document of its own (issue 70), belonging to the session's
/// owner, like the session's others, its components built in that owner's name (issue 321). In the
/// order of issue 304: what `spec` asks for is validated - the component bundles it names read in
/// the name of `context`, who names them -, then creating the channel is asked, then its founders'
/// admission; the store asks again as it stores (issue 312). Stores nothing.
///
/// # Errors
/// - `ValueError` if its name is no name or its language no tag.
/// - [`SessionError`] if `spec` asks for something that cannot be: components that make no
///   moderator, or name bundles `context` may not read, or a participant who is not one of the
///   session's.
/// - `StoreError::PermissionDenied` if `context` may not create the channel.
/// - `ChannelError::OutOfScope` or `NotCleared` for a founder who may not take part.
async fn asked_for(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    id: ChannelId,
    spec: ChannelSpec,
) -> Result<(Channel, Vec<ActorId>)> {
    let document = session.session();
    let name = Name::new(&spec.name)?;
    let language = Language::optional(spec.language.as_deref())?;
    let classification = spec.classification;
    require_classification(kernel.confidentiality().as_ref(), &classification)?;

    // The creator first, if they take part in the session at all.
    let mut ids = vec![context.initiator_id];
    ids.extend(spec.participants);
    let mut participants = Vec::new();
    for actor in ids {
        if participants.iter().any(|p: &Participant| p.actor_id() == actor) {
            continue;
        }
        match document.participants.iter().find(|p| p.actor_id() == actor) {
            Some(participant) => participants.push(participant.clone()),
            None if actor == context.initiator_id => {}
            None => {
                let participant = actor;
                return Err(
                    SessionError::NotAParticipant { session: session.id(), participant }.into()
                );
            }
        }
    }

    // Components the channel would not take are refused before the verb (issue 337); and whoever
    // names component bundles names only what they may read themselves, though the channel builds
    // them in its owner's name (issue 321): their own request.
    kernel
        .component_registry()
        .require_acceptable(&spec.components, ChannelLoop::COMPONENT_FACETS, context)
        .await
        .map_err(SessionError::InvalidComponents)?;

    let mut channel = Channel::build(
        id,
        session.id(),
        document.security_attributes.owner_id,
        name,
        spec.components,
        classification,
        language,
    );
    channel.internal_actors =
        spec.internal_actors.iter().map(InternalActorTemplate::instantiate).collect();
    channel.uses_workspace = spec.uses_workspace;
    // Whether the actor may create it, before anything is asked of the others in it (issues 198,
    // 312): who may not learns nothing of their clearance. The store asks again as it stores.
    kernel
        .access_policy()
        .require(
            context,
            ActionType::create(Channel::KIND, &channel.security_attributes),
            StoreError::PermissionDenied,
        )
        .await?;
    let controllers = kernel.controller_manager();
    let founders = ResolvedParticipant::resolve_existing(controllers, &participants, &channel);
    kernel.participation().require_admitted(context, Some(&channel), &founders.await?).await?;
    Ok((channel, participants.iter().map(Participant::actor_id).collect()))
}

/// Deletes a channel of a session on behalf of `context`, starting the session where it does not
/// run: it is marked, its actors leave, its task ends.
pub struct Delete {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
}

impl Delete {
    /// Deletes channel `channel` of `session` on behalf of `context`.
    pub fn new(context: &SecurityContext, session: SessionId, channel: ChannelId) -> Self {
        Self { context: context.clone(), session, channel }
    }
}

#[async_trait]
impl Procedure for Delete {
    type Plan = ();
    type Output = bool;
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))
    }

    /// Asked first, in the order of issue 304, whether `context` reaches it as the views reach it
    /// (`SessionLoop::listed_channel_for`) - also one that does not run (issue 463), which its
    /// owner removes so -, then whether they may delete it (`DeleteChannel`); then taken off the
    /// session's list - which asks whether `context` may change it (`UpdateSession`), so that a
    /// refused deletion leaves the channel running and listed -, marked (`mark_deleted`), what lies
    /// in its scope deleted as far as `context` may (issue 320) - not put back if a later step
    /// fails -, and the group of its participants, in the kernel's name (issue 348); all of it
    /// carries the channel's mark and comes back with it (issues 178, 469). If any of it fails, the
    /// channel is marked no more and listed again, in the kernel's name - without what was deleted
    /// of its scope, but with its group -, and runs on. Its deletion is announced as it is marked,
    /// and as it is taken back its restoring. Then it is stopped. `false` if there is no channel
    /// the caller reaches, as every deletion answers (issue 513).
    ///
    /// # Errors
    /// As `session::open` and `mark_deleted`; `StoreError::PermissionDenied` if
    /// `context` may not delete the channel or change the session; `SessionError::ScopeNotEmpty` if
    /// something of its scope is left that `context` may not delete; whatever else the stores
    /// report.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<bool, SessionError> {
        let Self { context, session, channel: id } = self;
        let sessions = sessions_of(kernel)?;
        let session = session::open(kernel, &context, session).await?;
        let Some((channel, _)) = session.listed_channel_for(&context, id).await else {
            return Ok(false);
        };
        let deleting = ActionType::delete(Channel::KIND, &channel.security_attributes);
        kernel.access_policy().require(&context, deleting, StoreError::PermissionDenied).await?;
        session.controller().remove_channel(&context, id).await?;
        {
            // Its deletion was permitted: put back in the kernel's name.
            let session = Arc::clone(&session);
            log.record("taking a channel off its session's list", async move {
                Ok(session.controller().add_channel(&SecurityContext::system(), id).await?)
            });
        }
        let removal = Removal::now(context.caller_id);
        mark_deleted(&sessions, &context, &channel, &removal).await?;
        {
            let sessions = Arc::clone(&sessions);
            log.record("marking a channel deleted", async move {
                let system = SecurityContext::system();
                Ok(sessions.channel_lifecycle().restore(&system, id).await.map(drop)?)
            });
        }
        let scope = Scope::ChannelScope { session_id: session.id(), channel_id: id };
        let holders = slice::from_ref(&scope);
        let left = kernel.scoped_entities().delete_lying_in(&context, holders, &removal).await?;
        ScopeNotEmpty::require_empty(&scope, left)?;
        kernel.group_manager().delete_participants(holders, &removal).await?;
        session.remove_channel(id).await;
        info!(
            "Channel {}/{} deleted from session {} by {}",
            channel.name,
            id,
            session.id(),
            context.caller_id
        );
        Ok(true)
    }
}

/// Marks `channel` deleted on behalf of `context` (issue 164), kept as removed as `removal` says,
/// and announced so (`SessionManager::channel_lifecycle`): it and its state stay stored - it can be
/// restored - until it is purged with its session (`procedure::session::purge`). Marked before
/// what lies in its scope, so that nothing of it goes unless the channel's deletion was granted,
/// and nothing is put into its scope afterwards (`ScopeHolders`, issue 458) - as it is deleted, and
/// for the compensation of restoring it.
///
/// # Errors
/// `SessionError::ChannelNotFound` if the store finds nothing `context` could delete - gone
/// meanwhile, by another process; `StoreError::PermissionDenied` if they may not delete it;
/// whatever else the store reports.
async fn mark_deleted(
    sessions: &SessionManager,
    context: &SecurityContext,
    channel: &Channel,
    removal: &Removal,
) -> Result<(), SessionError> {
    if !sessions.channel_lifecycle().delete(context, channel, removal).await? {
        let (session, channel) = (channel.session_id, channel.id);
        return Err(SessionError::ChannelNotFound { session, channel });
    }
    Ok(())
}

/// Restores a deleted channel of a session on behalf of `context`, starting the session where it
/// does not run, and starts the channel, letting go whom the session let go meanwhile. Holds its
/// participants, so that nobody takes one in or lets one go while it is listed again.
pub struct Restore {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
}

impl Restore {
    /// Restores the deleted channel `channel` of `session` on behalf of `context`.
    pub fn new(context: &SecurityContext, session: SessionId, channel: ChannelId) -> Self {
        Self { context: context.clone(), session, channel }
    }
}

#[async_trait]
impl Procedure for Restore {
    /// What reading the deleted channel's participants failed at, answered as it is carried out.
    type Plan = Result<(), SessionError>;
    type Output = Arc<ChannelLoop>;
    type Error = SessionError;

    /// Its participants as the deleted channel lists them, read in the kernel's name, in a session
    /// that runs - none for one that is no deleted channel of the session; one it starts it holds
    /// exclusively, the participants with it.
    async fn enumerate(
        &self,
        kernel: &Kernel,
        locks: &mut Locks<'_>,
    ) -> Result<Self::Plan, Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))?;
        let running = kernel.sessions().and_then(|sessions| sessions.running(self.session));
        if running.is_none() {
            return Ok(Ok(()));
        }
        let system = SecurityContext::system();
        let read = kernel.entity_stores().channels.get_deleted(&system, self.channel).await;
        let deleted = match read.optional() {
            Ok(deleted) => deleted.filter(|deleted| deleted.entity.session_id == self.session),
            Err(error) => return Ok(Err(error.into())),
        };
        for participant in deleted.iter().flat_map(|deleted| &deleted.entity.participants) {
            locks.exclusive(Key::Participant(self.session, participant.actor_id()))?;
        }
        Ok(Ok(()))
    }

    /// Asked first (`to_restore`); then the group of its participants restored in the kernel's name
    /// (issue 348) - first, so that whoever it lets read the channel hears it restored, and once it
    /// runs, its participants reach it, whatever of its scope comes back -, its mark taken away
    /// (`Restore`, asked by the store) and announced (`SessionManager::channel_lifecycle`), who was
    /// let go from the session while it was deleted (issue 590) let go from it, in the kernel's
    /// name and removed by it, before it is built, listed in the session again - which asks whether
    /// `context` may change it - and run from its stored state. If any of that fails, it is deleted
    /// again with its group, marked as it was, in the kernel's name, so that it is neither half
    /// back nor lost, and a restoring tried again finds what went with it. What was deleted with it
    /// comes back last (issue 178) - what cannot is logged, and the channel stays restored.
    ///
    /// # Errors
    /// As `session::open` and `to_restore`; `StoreError::NameTaken` if its name was
    /// taken since (issue 250), `StoreError::PermissionDenied` if `context` may not restore it or
    /// change the session; whatever building it reports.
    async fn carry_out(
        self,
        read: Result<(), SessionError>,
        kernel: &Kernel,
        log: &mut Compensation,
    ) -> Result<Arc<ChannelLoop>, SessionError> {
        read?;
        let Self { context, session, channel: id } = self;
        let sessions = sessions_of(kernel)?;
        let session = session::open(kernel, &context, session).await?;
        let (Deleted { removal, .. }, gone) = to_restore(kernel, &session, &context, id).await?;
        // Its group first, so that whoever it lets read the channel hears it restored; its
        // restoring was permitted: deleted again in the kernel's name.
        let scope = Scope::ChannelScope { session_id: session.id(), channel_id: id };
        let groups = kernel.group_manager();
        groups.restore_participants(slice::from_ref(&scope), &removal).await?;
        {
            let (groups, scope, removal) = (Arc::clone(groups), scope.clone(), removal.clone());
            log.record("restoring a channel's group", async move {
                Ok(groups.delete_participants(slice::from_ref(&scope), &removal).await?)
            });
        }
        let document = sessions.channel_lifecycle().restore(&context, id).await?;
        {
            let (sessions, restored) = (Arc::clone(&sessions), document.clone());
            let removal = removal.clone();
            log.record("taking a channel's mark away", async move {
                let system = SecurityContext::system();
                Ok(mark_deleted(&sessions, &system, &restored, &removal).await?)
            });
        }
        let document = let_go_where_not_running(kernel, document, &gone, ActorId::System).await?;
        session.controller().add_channel(&context, id).await?;
        {
            let session = Arc::clone(&session);
            log.record("listing a channel", async move {
                Ok(session.controller().remove_channel(&SecurityContext::system(), id).await?)
            });
        }
        let channel = session.add_channel(&document).await?;
        let holders = slice::from_ref(&scope);
        if let Err(e) = kernel.scoped_entities().restore_lying_in(&context, holders, &removal).await
        {
            warn!(
                "Channel {} of session {} was restored, but not what lies in its scope: {}",
                id,
                session.id(),
                e
            );
        }
        info!(
            "Channel {}/{} restored in session {} by {}",
            document.name,
            id,
            session.id(),
            context.caller_id
        );
        Ok(channel)
    }
}

/// The deleted channel `id` of `session` as `context` may restore it, with whom the session let go
/// while it was deleted (issue 590). In the order of issue 304: it is read as deleted
/// (`ReadDeleted`, as the session lists it no more), then restoring it is asked (`Restore`), then
/// whether those it lists that are still in the session may take part in it again - asked before
/// anything is stored, as of a new channel's founders (`Participation::require_admitted`); the
/// store asks again as it restores it. Stores nothing.
///
/// # Errors
/// `SessionError::ChannelNotFound` if the session has no such deleted channel, or `context` may
/// not read it; `StoreError::PermissionDenied` if they may not restore it;
/// `ChannelError::OutOfScope` or `NotCleared` for one who may no longer take part; whatever
/// reading a participant reports.
async fn to_restore(
    kernel: &Kernel,
    session: &SessionLoop,
    context: &SecurityContext,
    id: ChannelId,
) -> Result<(Deleted<Channel>, Vec<ActorId>)> {
    // What went with it carries its mark (issue 469).
    let of_this = kernel
        .entity_stores()
        .channels
        .get_deleted(context, id)
        .await
        .optional()?
        .filter(|deleted| deleted.entity.session_id == session.id());
    let Some(deleted) = of_this else {
        return Err(SessionError::ChannelNotFound { session: session.id(), channel: id }.into());
    };
    let channel = &deleted.entity;
    let restoring = ActionType::restore(Channel::KIND, &channel.security_attributes);
    kernel.access_policy().require(context, restoring, StoreError::PermissionDenied).await?;
    let document = session.session();
    let (staying, gone): (Vec<Participant>, Vec<Participant>) =
        channel.participants.iter().cloned().partition(|p| document.takes_part(p.actor_id()));
    let controllers = kernel.controller_manager();
    let staying = ResolvedParticipant::resolve_existing(controllers, &staying, channel).await?;
    kernel.participation().require_admitted(context, Some(channel), &staying).await?;
    let gone = gone.iter().map(Participant::actor_id).collect();
    Ok((deleted, gone))
}

/// Builds again the channels of a running session left out as it started, their components not
/// to be read (issue 463); nothing for a session that does not run. Asked by whoever opens a
/// session, after opening it - the REST server, as every request reaches the session
/// (`ensure_session`).
pub struct Start {
    session: SessionId,
}

impl Start {
    /// Builds again the channels of `session` left out as it started.
    pub fn left_out(session: SessionId) -> Self {
        Self { session }
    }
}

#[async_trait]
impl Procedure for Start {
    /// Whether there is anything to build again.
    type Plan = bool;
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<bool, Deferred> {
        let Some(running) = kernel.sessions().and_then(|sessions| sessions.running(self.session))
        else {
            return Ok(false);
        };
        let left_out: Vec<ChannelId> = running
            .listed_channels()
            .iter()
            .filter(|(_, run)| matches!(run, ChannelRun::Unavailable(_)))
            .map(|(channel, _)| channel.id)
            .collect();
        for channel in &left_out {
            locks.exclusive(Key::Channel(self.session, *channel))?;
        }
        Ok(!left_out.is_empty())
    }

    /// Holding them - so that nobody lets someone go from one of them meanwhile (issue 590) -,
    /// those whose components can be read again run from now on (`SessionLoop::add_channel`), the
    /// others stay left out, with why they still cannot run - also one whose store failed, for the
    /// session runs already. Asked as the running session is opened, so logged at `debug!`.
    async fn carry_out(
        self,
        any: bool,
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let running = kernel.sessions().and_then(|sessions| sessions.running(self.session));
        let (true, Some(running)) = (any, running) else { return Ok(()) };
        let session = running.id();
        for (channel, run) in running.listed_channels() {
            let ChannelRun::Unavailable(_) = run else { continue };
            match running.add_channel(&channel).await.map_err(unavailable_components) {
                Ok(_) => info!("Channel {} of session {} can be run again", channel.id, session),
                Err(Ok(now)) => {
                    debug!(
                        "Channel {} of session {} still cannot be run: {}",
                        channel.id, session, now
                    );
                    running.hold_unavailable(Channel::clone(&channel), Arc::new(now));
                }
                Err(Err(error)) => debug!(
                    "Channel {} of session {} could not be built again: {:#}",
                    channel.id, session, error
                ),
            }
        }
        Ok(())
    }
}

/// Gives a channel of a session another classification on behalf of `context`, starting the
/// session where it does not run (`ChannelLoop::reclassify`): who takes someone in is asked
/// whether they are cleared for the classification, and raising it whether every participant is -
/// holding the channel, neither crosses the other.
pub struct Reclassify {
    context: SecurityContext,
    session: SessionId,
    channel: ChannelId,
    classification: Classification,
}

impl Reclassify {
    /// Gives channel `channel` of `session` `classification`, on behalf of `context`.
    pub fn new(
        context: &SecurityContext,
        session: SessionId,
        channel: ChannelId,
        classification: Classification,
    ) -> Self {
        Self { context: context.clone(), session, channel, classification }
    }
}

#[async_trait]
impl Procedure for Reclassify {
    type Plan = ();
    type Output = ();
    type Error = SessionError;

    async fn enumerate(&self, kernel: &Kernel, locks: &mut Locks<'_>) -> Result<(), Deferred> {
        lock_to_open(kernel, locks, self.session)?;
        locks.exclusive(Key::Channel(self.session, self.channel))
    }

    /// # Errors
    /// As `session::open`, `SessionLoop::channel_for` and
    /// `ChannelLoop::reclassify`.
    async fn carry_out(
        self,
        (): (),
        kernel: &Kernel,
        _: &mut Compensation,
    ) -> Result<(), SessionError> {
        let Self { context, session, channel, classification } = self;
        let session = session::open(kernel, &context, session).await?;
        let channel = session.channel_for(&context, channel).await?;
        Ok(channel.reclassify(&context, classification).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{Create, Delete, Reclassify, Restore};
    use crate::procedure::testing::{waits_for, Held};
    use crate::procedure::Key;
    use crate::session::testing::{create, spec, test_kernel};
    use crate::session::SessionManager;
    use crate::testing::World;
    use caelumora_domain::actor::ActorId;
    use caelumora_domain::agent::testing::make_personality;
    use caelumora_domain::channel::Participant;
    use caelumora_domain::confidentiality::{Classification, Confidentiality};
    use caelumora_domain::session::testing::make_session;
    use caelumora_domain::user::UserId;
    use caelumora_security::SecurityContext;
    use caelumora_store::testing::stored_agent;
    use std::sync::Arc;

    /// Reclassifying, deleting and restoring a channel hold it exclusively - each waits for whoever
    /// takes someone into it or lets them go, who holds it shared -; restoring holds its
    /// participants, and creating one its founders.
    #[tokio::test]
    async fn the_procedures_of_a_channel_hold_it_and_the_participants_they_change() {
        let world = World::new().await;
        let executor = Arc::clone(world.kernel.procedures());
        let alice = World::as_user(world.alice);
        let session = world.session_of(&[world.bob], &[], &[]).await;
        let (id, main) = (session.id(), session.channels()[0].id());
        let bob = ActorId::User(world.bob);

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let internal = Classification::general(Confidentiality::Internal);
        let reclassifying = Reclassify::new(&alice, id, main, internal);
        waits_for(held, &executor, reclassifying).await.expect("reclassified once let go");

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let deleting = Delete::new(&alice, id, main);
        assert!(waits_for(held, &executor, deleting).await.unwrap(), "deleted once let go");

        let held = Held::key(&executor, Key::Channel(id, main), false).await;
        let restoring = Restore::new(&alice, id, main);
        waits_for(held, &executor, restoring).await.expect("restored once it is let go");
        executor.run(Delete::new(&alice, id, main)).await.unwrap();

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let restoring = Restore::new(&alice, id, main);
        waits_for(held, &executor, restoring).await.expect("restored once bob is let go");

        let held = Held::key(&executor, Key::Participant(id, bob), true).await;
        let creating = Create::new(&alice, id, spec("side", vec![bob]));
        waits_for(held, &executor, creating).await.expect("created once its founder is let go");
        world.manager.stop(&alice, id).await.unwrap();
    }

    /// A channel whose participant no longer exists is restored all the same: whoever cannot be
    /// read is not asked whether they may take part (`ResolvedParticipant::resolve_existing`), and
    /// rests in it.
    #[tokio::test]
    async fn a_channel_is_restored_without_asking_of_a_participant_that_no_longer_exists() {
        let kernel = test_kernel();
        let manager = SessionManager::new(Arc::clone(&kernel));
        let ctx = SecurityContext::default();
        let owner = ActorId::User(UserId::new());
        let helper =
            stored_agent(kernel.entity_stores(), "helper", make_personality("plain"), owner).await;
        let agent = ActorId::Agent(helper.id);
        let mut session = make_session("hers", UserId::new());
        session.participants = vec![Participant::Agent(helper.id)];
        let session = create(&manager, &ctx, session).await.unwrap();
        let id = session.create_channel(&ctx, spec("side", vec![agent])).await.unwrap().id();
        assert!(session.delete_channel(&ctx, id).await.unwrap());
        assert!(kernel.agent_manager().delete(&ctx, helper.id).await.unwrap());

        let restored = session.restore_channel(&ctx, id).await.expect("restored all the same");
        assert!(restored.channel().lists(agent), "it rests in it");
        assert!(!restored.participant_ids().contains(&agent), "not run");
    }
}
