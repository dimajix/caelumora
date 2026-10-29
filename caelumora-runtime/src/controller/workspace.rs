//! `WorkspaceController`: the one writer of a live workspace in this instance, with its storage.

use super::document::{LiveDocument, Writing};
use crate::backend::StorageGone;
use crate::group::{gone_groups, prune_groups, require_nameable};
use crate::terminal::{Shell, Terminal, TerminalProvider};
use crate::view::{channel_reading, ChannelReader};
use crate::workspace::reaper::scope_of;
use crate::workspace::{
    require_classification, require_write, store_error, WorkspaceError, WorkspaceResult,
};
use arc_swap::Guard;
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, Place};
use caelumora_model::event::{EventPublisher, GlobalEvent};
use caelumora_model::group::Group;
use caelumora_model::privacy::SessionPrivacy;
use caelumora_model::scope::Scope;
use caelumora_model::security::{
    AccessPolicy, AccessQuestions, AclEntry, Grant, SecurityContext, Subject,
};
use caelumora_model::session::{Channel, Session};
use caelumora_model::store::{EntityStore, EntityStores, StoreResult, StoreResultExt, Updatable};
use caelumora_model::workspace::{Workspace, WorkspaceId, WorkspaceUpdateEvent};
use std::slice;
use std::sync::Arc;
use tracing::warn;

/// The live workspace in this instance (issue 344), shared by everything
/// that uses it - a workspace of its owner's or the global scope may be
/// attached to several sessions, and is used directly too: the one writer
/// of what it is - its name, its description, its classification and its
/// access list -, and what hands out its terminal to whoever may use it,
/// directly or in a channel of a session it is attached to (issue 251,
/// decision 10). Making, deleting, restoring and purging it are the
/// `WorkspaceManager`'s.
///
/// In a channel, it is asked of the session and the channel as the caller
/// gives them - live from a running session, as stored otherwise -, and
/// read itself as stored now: an operation in a channel relies on nothing
/// its caller reached before (issue 296).
///
/// Every change reaches the workspace first as the views reach it - by the
/// store, in the caller's name -, so that whoever does not reach it learns
/// nothing else of it, and is applied to it as stored: a change made
/// elsewhere may not have reached the live copy yet, an access list never
/// does by its announcement.
#[derive(Debug)]
pub struct WorkspaceController {
    workspace: LiveDocument<Workspace>,
    /// The storage the workspace's files lie in; none if it is no longer
    /// configured.
    provider: Option<Arc<dyn TerminalProvider>>,
    groups: Arc<dyn EntityStore<Group>>,
    /// Who may change its files (`WriteWorkspace`, issue 23).
    access_policy: Arc<dyn AccessPolicy>,
    /// What may flow where, and who is cleared for what (issue 304).
    confidentiality: Arc<dyn ConfidentialityPolicy>,
}

impl WorkspaceController {
    /// The live `workspace`, whose files lie in `provider`. Made by the
    /// `ControllerManager`, which keeps one per workspace.
    pub(crate) fn new(
        workspace: Workspace,
        provider: Option<Arc<dyn TerminalProvider>>,
        event_publisher: Arc<dyn EventPublisher>,
        stores: &EntityStores,
        access_policy: Arc<dyn AccessPolicy>,
        confidentiality: Arc<dyn ConfidentialityPolicy>,
    ) -> Self {
        Self {
            workspace: LiveDocument::new(
                workspace.id,
                workspace,
                Arc::clone(&stores.workspaces),
                event_publisher,
            ),
            provider,
            groups: Arc::clone(&stores.groups),
            access_policy,
            confidentiality,
        }
    }

    pub fn load(&self) -> Guard<Arc<Workspace>> {
        self.workspace.load()
    }

    pub fn id(&self) -> WorkspaceId {
        *self.workspace.address()
    }

    /// Whether its terminals have a shell - asked of its storage, without
    /// reaching one; `false` for a storage no longer configured.
    pub fn has_shell(&self) -> bool {
        self.provider.as_ref().is_some_and(|provider| provider.has_shell())
    }

    /// Its terminal, for whoever `ctx` acts for to use directly, for what
    /// `need` asks - reading its files, or changing them too (issue 23) -,
    /// wherever they act: reached as the views reach it, then, to change
    /// its files, if its list lets them (`WriteWorkspace`).
    ///
    /// # Errors
    /// `NotFound` if they do not reach it, so that nothing can be probed; `ReadOnly` if they may
    /// not change its files where `need` asks to; `StorageGone`; a `TerminalError` of reaching its
    /// place; or whatever else the store reports.
    pub async fn terminal(
        &self,
        ctx: &SecurityContext,
        need: Grant,
    ) -> WorkspaceResult<Arc<dyn Terminal>> {
        let workspace = self.reached(ctx).await?;
        if need == Grant::Write {
            require_write(self.access_policy.as_ref(), ctx, &workspace).await?;
        }
        Ok(self.provider(&workspace)?.terminal_for(self.id()).await?)
    }

    /// The workspace, if the caller of `ctx` may use it in `channel` of
    /// `session` (issue 251, decision 10): it is the session's, the channel
    /// is the session's and uses it, the caller is one of its current
    /// participants, its access list lets `ctx` read it (`GetWorkspace`, issue
    /// 23) and the workspace and the channel flow both ways - asked as the
    /// policy asks what an actor puts into a channel
    /// (`ConfidentialityPolicy::may_publish`). `None` otherwise, also when
    /// the workspace is gone, without saying which of them failed. Whether
    /// the caller may read the session and the channel is not asked here:
    /// whoever gives them reached them (`terminal_in`, the views).
    ///
    /// # Errors
    /// Whatever reading the workspace reports, other than its absence.
    pub(crate) async fn usable_in(
        &self,
        ctx: &SecurityContext,
        session: &Session,
        channel: &Channel,
    ) -> WorkspaceResult<Option<Workspace>> {
        let takes_part = channel.lists(ctx.caller_id);
        if session.workspace != Some(self.id())
            || channel.session_id != session.id
            || !channel.uses_workspace
            || !takes_part
        {
            return Ok(None);
        }
        let Some(workspace) =
            self.workspace.reached(&SecurityContext::system()).await.optional()?
        else {
            warn!(session = %session.id, workspace = %self.id(), "The workspace attached to a session is gone");
            return Ok(None);
        };
        if !workspace.name.scope.admits(&Scope::SessionScope { session_id: session.id }) {
            warn!(session = %session.id, workspace = %self.id(), "A workspace of another session's scope is attached to a session");
            return Ok(None);
        }
        if !self.access_policy.may_read_workspace(ctx, &workspace.security_attributes).await {
            return Ok(None);
        }
        let (inside, outside) = (&workspace.classification, &channel.classification);
        let flows = self.confidentiality.may_publish(ctx, inside, outside)
            && self.confidentiality.may_publish(ctx, outside, inside);
        Ok(flows.then_some(workspace))
    }

    /// The workspace and its terminal, for what `need` asks, if whoever
    /// `ctx` acts for may use it in `channel` of `session` (issue 296), as
    /// `usable_in_channel` asks.
    ///
    /// # Errors
    /// What `usable_in_channel` reports, `StorageGone`, or a `TerminalError` of reaching its place.
    pub async fn terminal_in(
        &self,
        ctx: &SecurityContext,
        session: &Session,
        channel: &Channel,
        need: Grant,
    ) -> WorkspaceResult<(Workspace, Arc<dyn Terminal>)> {
        let workspace = self.usable_in_channel(ctx, session, channel, need).await?;
        let terminal = self.provider(&workspace)?.terminal_for(self.id()).await?;
        Ok((workspace, terminal))
    }

    /// The workspace and its shell as `channel` sees it - the commands
    /// started in that channel alone (issues 251, 296) -, if whoever `ctx`
    /// acts for may change its files there, as `terminal_in` asks; no shell
    /// where its storage gives none. The one place that makes a channel the
    /// scope of a shell.
    ///
    /// # Errors
    /// What `terminal_in` reports.
    pub async fn shell_in(
        &self,
        ctx: &SecurityContext,
        session: &Session,
        channel: &Channel,
    ) -> WorkspaceResult<(Workspace, Option<Arc<dyn Shell>>)> {
        let (workspace, terminal) = self.terminal_in(ctx, session, channel, Grant::Write).await?;
        let shell = terminal.shell(&scope_of(channel.id));
        Ok((workspace, shell))
    }

    /// The workspace, if whoever `ctx` acts for may use it in `channel` of
    /// `session` for what `need` asks: if they may read the session
    /// (`GetSession`) and the channel (`view::channel_reading` - an agent
    /// from the channel it acts in, a user by their roles), it is usable
    /// there (`usable_in`), and - to change its files - its list lets them
    /// (`WriteWorkspace`, issue 23).
    ///
    /// # Errors
    /// `NotUsableInChannel` if they may not use it there, `ReadOnly` if they
    /// may not change its files where `need` asks to, or what the store
    /// reports.
    pub(crate) async fn usable_in_channel(
        &self,
        ctx: &SecurityContext,
        session: &Session,
        channel: &Channel,
        need: Grant,
    ) -> WorkspaceResult<Workspace> {
        if !self.access_policy.may_read_session(ctx, &session.security_attributes).await {
            return Err(WorkspaceError::NotUsableInChannel);
        }
        let at = Place::of(channel, SessionPrivacy::of(session));
        // An agent uses the workspace only in the channel it acts in (`ActorRuntime`), and so reads
        // it from there.
        let agent = matches!(ctx.caller_id, ActorId::Agent(_));
        let reader = ChannelReader::of(agent.then_some(&at));
        if channel_reading(
            self.access_policy.as_ref(),
            self.confidentiality.as_ref(),
            ctx,
            &channel.security_attributes,
            &at,
            reader,
        )
        .await
        .is_none()
        {
            return Err(WorkspaceError::NotUsableInChannel);
        }
        let workspace = self
            .usable_in(ctx, session, channel)
            .await?
            .ok_or(WorkspaceError::NotUsableInChannel)?;
        if need == Grant::Write {
            require_write(self.access_policy.as_ref(), ctx, &workspace).await?;
        }
        Ok(workspace)
    }

    /// Renames the workspace on behalf of `ctx` (`UpdateWorkspace`).
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn update_name(&self, ctx: &SecurityContext, name: &str) -> WorkspaceResult<()> {
        self.update(ctx, WorkspaceUpdateEvent::NameChanged(name.to_string())).await
    }

    /// Says what the workspace holds; `None` or empty takes it away.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn update_description(
        &self,
        ctx: &SecurityContext,
        description: Option<&str>,
    ) -> WorkspaceResult<()> {
        self.update(ctx, WorkspaceUpdateEvent::DescriptionChanged(description.map(String::from)))
            .await
    }

    /// Reclassifies the workspace as a whole: lowering releases every file
    /// in it. Its owner must be cleared for the new classification; which
    /// channels may use it follows at their next use.
    ///
    /// # Errors
    /// As [`Self::update`].
    pub async fn update_classification(
        &self,
        ctx: &SecurityContext,
        classification: Classification,
    ) -> WorkspaceResult<()> {
        self.update(ctx, WorkspaceUpdateEvent::ClassificationChanged(classification)).await
    }

    /// Gives the workspace the access list `acl` (issue 23) on behalf of
    /// `ctx`: reached as the views reach it, each group the list newly names
    /// read - who names one must read it (issue 286) -, then stored, which
    /// is where its owner's act is asked (`UpdateWorkspace`): whether the
    /// caller reads a group tells them nothing of others (issue 312).
    /// Announced without the list, which only its owner sees.
    ///
    /// # Errors
    /// `NotFound` if the caller does not reach it, a `StoreError` -
    /// `PermissionDenied` for anyone but its owner, `Validation` for an
    /// entry that names an agent, the system or nobody, or a group the
    /// caller cannot read.
    pub async fn update_access(
        &self,
        ctx: &SecurityContext,
        acl: Vec<AclEntry>,
    ) -> WorkspaceResult<()> {
        let mut writing = self.workspace.write().await;
        let workspace = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        require_nameable(self.groups.as_ref(), ctx, &workspace.security_attributes.acl, &acl)
            .await?;
        self.change_access(ctx, &mut writing, |list| *list = acl.clone()).await
    }

    /// Shares the workspace with `subject` as far as `grant` reaches, on
    /// behalf of `ctx`: an entry added to its access list, unless one
    /// already grants `subject` as much - one that grants less is widened.
    ///
    /// # Errors
    /// As [`Self::update_access`].
    pub async fn share(
        &self,
        ctx: &SecurityContext,
        subject: Subject,
        grant: Grant,
    ) -> WorkspaceResult<()> {
        let mut writing = self.workspace.write().await;
        let workspace = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        let acl = &workspace.security_attributes.acl;
        if acl.iter().any(|entry| entry.subject == subject && entry.grant.covers(grant)) {
            return Ok(());
        }
        let entry = AclEntry { subject, grant };
        require_nameable(self.groups.as_ref(), ctx, acl, slice::from_ref(&entry)).await?;
        self.change_access(ctx, &mut writing, |list| {
            list.retain(|entry| entry.subject != subject);
            list.push(entry);
        })
        .await
    }

    /// Takes over a change made elsewhere - already stored there - once it
    /// is announced: read again as stored, in the runtime's name, since the
    /// announcement of a new access list does not carry it.
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    pub async fn process_global_event(&self, event: &GlobalEvent) -> StoreResult<()> {
        if event.about::<Workspace>().is_some_and(|(id, change)| {
            id == self.id() && change.update::<WorkspaceUpdateEvent>().is_some()
        }) {
            self.reload().await?;
        }
        Ok(())
    }

    /// Takes the workspace over as it is stored now (`LiveDocument::take_stored`): for a change
    /// made elsewhere, announced or lost (issue 502).
    ///
    /// # Errors
    /// Whatever the store reports; the live copy is then unchanged.
    pub(super) async fn reload(&self) -> StoreResult<()> {
        self.workspace.take_stored().await
    }

    /// The workspace as `ctx` reaches it, as stored now: read and cleared
    /// for, by the store (issues 304, 322).
    ///
    /// # Errors
    /// `NotFound` if they do not reach it, or whatever else the store
    /// reports.
    async fn reached(&self, ctx: &SecurityContext) -> WorkspaceResult<Workspace> {
        self.workspace.reached(ctx).await.map_err(store_error(self.id()))
    }

    /// The storage `workspace` - this one, as reached - lies in.
    ///
    /// # Errors
    /// `StorageGone` if it is no longer configured.
    fn provider(&self, workspace: &Workspace) -> WorkspaceResult<&Arc<dyn TerminalProvider>> {
        Ok(self.provider.as_ref().ok_or_else(|| StorageGone(workspace.storage.clone()))?)
    }

    /// Applies `event` to the workspace on behalf of `ctx`: reached as the
    /// views reach it, stored, which is where the permission is checked
    /// (`UpdateWorkspace`), then taken over live and announced. A new
    /// classification is validated and its flow asked before: it tells the
    /// caller nothing of others (issue 312).
    ///
    /// # Errors
    /// `NotFound` if the caller does not reach it, `Invalid` if what `event` says does not fit,
    /// `StorageGone` if its storage is no longer configured, `ClassificationRefused`,
    /// `StorageNotCleared` or `NotCleared` for a classification, or whatever the store reports.
    async fn update(
        &self,
        ctx: &SecurityContext,
        event: WorkspaceUpdateEvent,
    ) -> WorkspaceResult<()> {
        let mut writing = self.workspace.write().await;
        let workspace = writing.reached(ctx).await.map_err(store_error(self.id()))?;
        if let WorkspaceUpdateEvent::ClassificationChanged(classification) = &event {
            let provider = self.provider(&workspace)?;
            require_classification(
                self.confidentiality.as_ref(),
                ctx,
                &workspace.storage,
                provider.as_ref(),
                classification,
            )?;
        }
        let workspace =
            self.change(ctx, &mut writing, |workspace| Ok(workspace.apply(&event)?)).await?;
        writing.announce_update(&event, &workspace);
        Ok(())
    }

    /// Changes the access list of the reached workspace under `writing` as
    /// `change` says, on behalf of `ctx`, as `Self::change` changes it, and
    /// announces it without the list (`WorkspaceUpdateEvent::AccessChanged`).
    async fn change_access(
        &self,
        ctx: &SecurityContext,
        writing: &mut Writing<'_, Workspace>,
        mut change: impl FnMut(&mut Vec<AclEntry>),
    ) -> WorkspaceResult<()> {
        let workspace = self
            .change(ctx, writing, |workspace| {
                change(&mut workspace.security_attributes.acl);
                Ok(())
            })
            .await?;
        writing.announce_update(&WorkspaceUpdateEvent::AccessChanged, &workspace);
        Ok(())
    }

    /// Changes the reached workspace under `writing` as `change` says, on
    /// behalf of `ctx`, and takes it over live (`Writing::change`) - its
    /// access list without the entries naming a group that is gone (issue
    /// 347), told apart from the list as the change begins.
    async fn change(
        &self,
        ctx: &SecurityContext,
        writing: &mut Writing<'_, Workspace>,
        mut change: impl FnMut(&mut Workspace) -> StoreResult<()>,
    ) -> WorkspaceResult<Arc<Workspace>> {
        let begun = writing.current();
        let gone = gone_groups(self.groups.as_ref(), [&begun.security_attributes.acl]).await;
        writing
            .change(ctx, |workspace| {
                change(workspace)?;
                prune_groups([&mut workspace.security_attributes.acl], &gone);
                Ok(())
            })
            .await
            .map(|((), workspace)| workspace)
            .map_err(store_error(self.id()))
    }
}
