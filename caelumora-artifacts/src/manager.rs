//! The one owner of "an artifact and its content" (issue 271): it publishes
//! versions - their content kept first, then the document, then announced -,
//! keeps artifacts in another scope, deletes, restores and purges them -
//! also with what holds their scope (`…_lying_in`, called by `scoped`) -,
//! and is the only code that holds the `ArtifactStorage`s, one per
//! configured storage.
//!
//! A version is read in one of two ways:
//!
//! - *by its owner* (`content`): `GetArtifact`, wherever it lies;
//! - *in the channel it was published in* (`published_content`): by
//!   whoever may read that channel - asked by the channel's view before it
//!   asks here -, and only a version published there.
//!
//! An artifact is published into its session's scope, made with the first
//! version of its name there and gone with the session, until its owner
//! keeps it in their own scope or the global one (`keep`). Its
//! classification is the workspace's its first version was copied from;
//! a later version from a workspace classified otherwise is refused.

use crate::artifact::{Artifact, ArtifactId, ArtifactUpdateEvent, ArtifactVersion};
use async_trait::async_trait;
use caelumora_model::actor::ActorId;
use caelumora_model::config::{BackendConfig, DEFAULT_MAX_FILE_BYTES};
use caelumora_model::confidentiality::{Classification, ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_model::digest::ContentDigest;
use caelumora_model::event::GlobalEvent;
use caelumora_model::group::{Group, GroupId};
use caelumora_model::scope::Scope;
use caelumora_model::security::{AclEntry, ActionType, Enforcement, SecurityAttributes, SecurityContext, SecurityManager, Subject, WRITE};
use caelumora_model::session::{ChannelId, SessionId};
use caelumora_model::store::{EntityKind, EntityStore, StoreError, StoreResult, StoreResultExt, Stored};
use caelumora_model::value::name::ScopedName;
use caelumora_model::value::text::Description;
use caelumora_model::value::ValueError;
use caelumora_runtime::component::{ComponentError, ComponentResult};
use caelumora_runtime::event::EventBus;
use caelumora_runtime::group::require_nameable;
use caelumora_runtime::scoped::{deleted_since, deleted_unless_refused, restored_unless_refused, within, ScopeMember};
use caelumora_runtime::terminal::ByteStream;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex as SyncMutex, PoisonError};
use tokio::sync::Mutex;
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("no artifact {0}")]
    NotFound(ArtifactId),
    #[error("no version {version} of artifact {artifact}")]
    VersionNotFound { artifact: ArtifactId, version: u32 },
    #[error("no storage of artifacts is configured")]
    NotConfigured,
    #[error("the storage '{0}' of artifacts is not configured")]
    UnknownStorage(String),
    #[error("an artifact is published by a user, or for one")]
    OwnerNotAUser,
    /// What is so classified may not be put into the storage.
    #[error("the storage '{0}' of artifacts may not hold what is so classified")]
    StorageNotCleared(String),
    /// A later version from a workspace classified otherwise than the first.
    #[error("the artifact '{0}' is classified otherwise than the workspace; publish it under another name")]
    ClassifiedOtherwise(String),
    #[error("the version is larger than the {limit} bytes an artifact may be")]
    TooLarge { limit: u64 },
    #[error("the file changed while it was published; publish it again once it is written")]
    Changed,
    /// Kept in a scope an artifact cannot lie in.
    #[error("an artifact is kept in its owner's scope or the global one")]
    InvalidScope,
    #[error(transparent)]
    Invalid(#[from] ValueError),
    /// The storage failed: what happened, never where.
    #[error("the storage of artifacts failed: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The runtime was built without the plugin, so there is no manager to
    /// ask.
    #[error("artifacts are not joined to this runtime")]
    NotJoined,
}

pub type ArtifactResult<T> = Result<T, ArtifactError>;

/// The section `artifacts` of the configuration: the storages artifacts
/// are kept in (issue 271), each by the name an artifact refers to it with
/// and a backend (`dyn ArtifactStorage`), which of them new artifacts go
/// to, and how large a version may be. No storage unless configured: then
/// no artifact can be published.
#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactsConfig {
    pub max_bytes: u64,
    /// The storage new artifacts are made in; needed where there are
    /// several, the one there is where there is one.
    pub publish_to: Option<String>,
    pub storage: BTreeMap<String, BackendConfig>,
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self { max_bytes: DEFAULT_MAX_FILE_BYTES, publish_to: None, storage: BTreeMap::new() }
    }
}

impl ArtifactsConfig {
    /// The storage new artifacts are made in: `publish_to`, or the one
    /// configured; `None` where there is none.
    pub fn publish_to(&self) -> Option<&str> {
        match (&self.publish_to, self.storage.len()) {
            (Some(name), _) => Some(name.as_str()),
            (None, 1) => self.storage.keys().next().map(String::as_str),
            (None, _) => None,
        }
    }

    /// Whether it fits together: `publish_to` names a configured storage,
    /// and is given where there are several.
    ///
    /// # Errors
    /// What does not fit.
    pub fn check(&self) -> Result<(), String> {
        match &self.publish_to {
            Some(name) if !self.storage.contains_key(name) => Err(format!("publish_to names '{name}', which is no storage of artifacts")),
            None if self.storage.len() > 1 => Err("publish_to is needed where there are several storages of artifacts".to_string()),
            _ => Ok(()),
        }
    }
}

/// `NotFound` for artifact `id` where the store found nothing.
fn store_error(id: ArtifactId) -> impl FnOnce(StoreError) -> ArtifactError {
    move |e| match e {
        StoreError::NotFound { .. } => ArtifactError::NotFound(id),
        e => ArtifactError::Store(e),
    }
}

/// Where the content of artifacts lies (issue 271): one piece per version,
/// by artifact and number, without permissions - which are the manager's.
/// The service a storage of artifacts is a backend of (issues 324, 328):
/// the plugin brings `local`, another crate may register one of its own
/// (`Plugin::backends`).
#[async_trait]
pub trait ArtifactStorage: Send + Sync + fmt::Debug {
    /// Keeps `content` as version `version` of `artifact`, at most `limit`
    /// bytes, or nothing; answers how many bytes were kept.
    async fn put(&self, artifact: ArtifactId, version: u32, content: ByteStream, limit: u64) -> ArtifactResult<u64>;
    /// The content of version `version` of `artifact`, with its size.
    async fn get(&self, artifact: ArtifactId, version: u32) -> ArtifactResult<(u64, ByteStream)>;
    /// Removes version `version` of `artifact`; nothing there is no error.
    async fn delete(&self, artifact: ArtifactId, version: u32) -> ArtifactResult<()>;
    /// Removes every version of `artifact`; nothing there is no error.
    async fn remove(&self, artifact: ArtifactId) -> ArtifactResult<()>;
    /// What may be put into it: the classification an artifact in it may
    /// have at most; `None` for no bound.
    fn destination(&self) -> Option<&Classification>;
}

pub struct ArtifactManager {
    artifacts: Arc<dyn EntityStore<Artifact>>,
    groups: Arc<dyn EntityStore<Group>>,
    events: Arc<EventBus>,
    security: Arc<dyn SecurityManager>,
    /// What may flow where (issue 304): into a storage that keeps it.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    storages: BTreeMap<String, Arc<dyn ArtifactStorage>>,
    /// The storage new artifacts are made in.
    publish_to: Option<String>,
    max_bytes: u64,
    /// Held while a document and its content are changed together, so that
    /// two versions of one artifact are not numbered alike.
    writing: Mutex<()>,
}

impl fmt::Debug for ArtifactManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArtifactManager").field("storages", &self.storages.keys().collect::<Vec<_>>()).field("publish_to", &self.publish_to).finish()
    }
}

impl ArtifactManager {
    /// The manager of the artifacts of `artifacts`, naming the groups of
    /// `groups` in their lists, whose content lies in `storages`, by their
    /// names - the storages `config` names, made as backends (issue 324) -,
    /// new ones in the storage `config` says.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` where `config` does not fit
    /// together.
    #[allow(clippy::too_many_arguments)]
    pub fn new(artifacts: Arc<dyn EntityStore<Artifact>>, groups: Arc<dyn EntityStore<Group>>, events: Arc<EventBus>, security: Arc<dyn SecurityManager>, confidentiality: Arc<dyn ConfidentialityPolicy>, storages: BTreeMap<String, Arc<dyn ArtifactStorage>>, config: &ArtifactsConfig) -> ComponentResult<Self> {
        config.check().map_err(|why| ComponentError::InvalidConfig(format!("artifacts: {why}")))?;
        Ok(Self { artifacts, groups, events, security, confidentiality, storages, publish_to: config.publish_to().map(String::from), max_bytes: config.max_bytes, writing: Mutex::new(()) })
    }

    /// The store of artifacts, which the views read in the viewer's name.
    pub(crate) fn store(&self) -> &Arc<dyn EntityStore<Artifact>> {
        &self.artifacts
    }

    /// Whether artifacts can be published: a storage is configured.
    pub fn is_configured(&self) -> bool {
        self.publish_to.is_some()
    }

    fn storage(&self, name: &str) -> ArtifactResult<&Arc<dyn ArtifactStorage>> {
        self.storages.get(name).ok_or_else(|| ArtifactError::UnknownStorage(name.to_string()))
    }

    /// Publishes `content` - its size and its bytes, as a file of a
    /// workspace is read -, classified as `classification`, as a version
    /// of the artifact `name` in the session `session`'s scope, published in
    /// `channel` by the agent `ctx` names as its caller, for the user it
    /// acts for: the artifact of that name there gains a version, or is
    /// made with its first. Its owner may change it, or make it
    /// (`UpdateArtifact`, `CreateArtifact`), asked before anything is kept;
    /// its storage may hold what is so classified. Where the content comes
    /// from is the caller's (issue 296): read before the artifact's own
    /// permission is asked, it gives nothing away to one who could read it.
    /// The caller vouches for `classification`: it is that of the workspace
    /// the content was read from (`artifact_tool` reads it through
    /// `WorkspaceManager::terminal_in`), never one a client names.
    /// The content is kept first, its digest taken as it is; a document that
    /// cannot be stored leaves no content. Answers the artifact as it is
    /// now, and the number of the new version.
    ///
    /// # Errors
    /// `NotConfigured`, `OwnerNotAUser`, `Invalid` for a name that is
    /// none, `ClassifiedOtherwise`, `StorageNotCleared`, `TooLarge`,
    /// `Changed`, `Unavailable`, or whatever the store reports -
    /// `PermissionDenied` and `NameTaken` among it.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish(&self, ctx: &SecurityContext, (size, content): (u64, ByteStream), classification: &Classification, name: &str, description: Option<&str>, session: SessionId, channel: ChannelId) -> ArtifactResult<(Artifact, u32)> {
        let ActorId::User(owner) = ctx.principal_id else { return Err(ArtifactError::OwnerNotAUser) };
        let name = ScopedName::new(name, Scope::SessionScope { session_id: session })?;
        let description = Description::optional(description)?;
        let _writing = self.writing.lock().await;
        // Found whoever's it is, so that one not the caller's to add to is
        // refused as such rather than taken for a name to make anew.
        let existing = self.artifacts.find_by_name(&SecurityContext::system(), &name).await.optional()?;
        let (id, number, storage_name) = match &existing {
            Some(artifact) => {
                self.security.require(ctx, ActionType::custom(Artifact::KIND, &WRITE, &artifact.security_attributes), StoreError::PermissionDenied).await?;
                if artifact.classification != *classification {
                    return Err(ArtifactError::ClassifiedOtherwise(artifact.name.name.to_string()));
                }
                (artifact.id, artifact.latest().map_or(1, |latest| latest.number + 1), artifact.storage.clone())
            }
            None => {
                let attributes = SecurityAttributes::owned_by(ActorId::User(owner));
                self.security.require(ctx, ActionType::create(Artifact::KIND, &attributes), StoreError::PermissionDenied).await?;
                (ArtifactId::new(), 1, self.publish_to.clone().ok_or(ArtifactError::NotConfigured)?)
            }
        };
        let storage = self.storage(&storage_name)?;
        if let Some(destination) = storage.destination() {
            self.confidentiality.require_publish(ctx, classification, destination, || ArtifactError::StorageNotCleared(storage_name.clone()))?;
        }

        if size > self.max_bytes {
            return Err(ArtifactError::TooLarge { limit: self.max_bytes });
        }
        let digest = Arc::new(SyncMutex::new(ContentDigest::new(size)));
        let digesting = Arc::clone(&digest);
        let content: ByteStream = Box::pin(content.inspect(move |chunk| {
            if let Ok(chunk) = chunk {
                digesting.lock().unwrap_or_else(PoisonError::into_inner).update(chunk);
            }
        }));
        let kept = storage.put(id, number, content, self.max_bytes).await?;
        let digest = match Arc::try_unwrap(digest) {
            Ok(digest) if kept == size => digest.into_inner().unwrap_or_else(PoisonError::into_inner).finish(),
            _ => {
                self.forget(storage, id, number, existing.is_none()).await;
                return Err(ArtifactError::Changed);
            }
        };

        // Read by the members of the channel it is published in (issues 23,
        // 316) - the group of its participants (issue 286) -, who read the
        // artifact for it: a draft published where few are reaches nobody
        // who reads a later version published where many are.
        let members = Subject::Group(GroupId::of_channel(channel));
        let mut version = ArtifactVersion { number, size, digest, media_type: None, description, session_id: session, channel_id: channel, published_by: ctx.caller_id, published_at: Utc::now(), acl: Vec::new() };
        version.grant_reading(members);
        let stored = match existing {
            Some(mut artifact) => {
                artifact.add_version(version);
                artifact.grant_reading(members);
                // Changed in the runtime's name: whoever may add a version
                // need not own the artifact (`WriteArtifact`, asked above),
                // and lets the channel they publish it in read it.
                self.artifacts.update(&SecurityContext::system(), &artifact).await.map(|()| artifact)
            }
            None => {
                let mut artifact = Artifact::new(id, name, storage_name, classification.clone(), owner, version);
                artifact.grant_reading(members);
                self.artifacts.create(ctx, &artifact).await.map(|()| artifact)
            }
        };
        let artifact = match stored {
            Ok(artifact) => artifact,
            Err(e) => {
                self.forget(storage, id, number, number == 1).await;
                return Err(e.into());
            }
        };
        // Heard by whoever reads the artifact and the version.
        let event = if number == 1 { GlobalEvent::created::<Artifact>(&id) } else { GlobalEvent::updated::<Artifact>(&id, &ArtifactUpdateEvent::VersionPublished(number)) };
        if let Some(version) = artifact.version(number) {
            self.events.publish_classified_part(event, artifact.security_attributes.clone(), artifact.version_attributes(version), std::slice::from_ref(&artifact.classification));
        }
        info!(artifact = %id, version = number, by = %ctx.caller_id, "Artifact published");
        Ok((artifact, number))
    }

    /// Removes what was kept of version `number` of artifact `id` for a
    /// publication that failed after - all of it for an artifact that was
    /// to be made by it. Logged where it fails: the failure that led here
    /// is what the caller is told.
    async fn forget(&self, storage: &Arc<dyn ArtifactStorage>, id: ArtifactId, number: u32, made: bool) {
        let removed = if made { storage.remove(id).await } else { storage.delete(id, number).await };
        if let Err(e) = removed {
            warn!(artifact = %id, version = number, "What was kept of a publication that failed stays: {e}");
        }
    }

    /// The content of version `number` of `artifact`, with its size -
    /// asking nothing: whoever reaches the artifact decided who may
    /// (`ArtifactView::content`, issue 292).
    ///
    /// # Errors
    /// `VersionNotFound`, `UnknownStorage`, or `Unavailable`.
    pub(crate) async fn content_of(&self, artifact: &Artifact, number: u32) -> ArtifactResult<(u64, ByteStream)> {
        if artifact.version(number).is_none() {
            return Err(ArtifactError::VersionNotFound { artifact: artifact.id, version: number });
        }
        self.storage(&artifact.storage)?.get(artifact.id, number).await
    }

    /// Gives artifact `id` the access list `acl` (issues 23, 316) on behalf
    /// of `ctx`: reached by the store, each group the list names newly
    /// read, since who names one must read it (issue 286), then stored,
    /// which is where its owner's act is asked (`UpdateArtifact`): whether
    /// the caller reads a group tells them nothing of others (issue 312).
    /// With `versions`, whom the list newly names reads every version there
    /// is as well, as `setfacl -R` hands an entry to a directory's files;
    /// otherwise the lists of its versions stay as they are. Whom the list
    /// no longer names reads no version, since that needs the artifact's
    /// list too.
    ///
    /// # Errors
    /// `NotFound` if there is none the caller may reach, or what the store
    /// reports - `PermissionDenied`, `Validation` for an entry that names
    /// an agent, the system or nobody, or a group the caller cannot read.
    pub async fn set_access(&self, ctx: &SecurityContext, id: ArtifactId, acl: Vec<AclEntry>, versions: bool) -> ArtifactResult<Artifact> {
        let _writing = self.writing.lock().await;
        let mut artifact = self.artifacts.get(ctx, id).await.map_err(store_error(id))?;
        require_nameable(self.groups.as_ref(), ctx, &artifact.security_attributes.acl, &acl).await?;
        if versions {
            let named: Vec<Subject> = acl.iter().map(|entry| entry.subject).filter(|subject| !artifact.security_attributes.acl.iter().any(|entry| entry.subject == *subject)).collect();
            for version in &mut artifact.versions {
                for subject in &named {
                    version.grant_reading(*subject);
                }
            }
        }
        artifact.security_attributes.acl = acl;
        artifact.updated_at = Utc::now();
        self.artifacts.update(ctx, &artifact).await?;
        self.events.publish_classified(GlobalEvent::updated::<Artifact>(&id, &ArtifactUpdateEvent::AccessChanged), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
        info!(artifact = %id, by = %ctx.caller_id, "Artifact's access changed");
        Ok(artifact)
    }

    /// Keeps artifact `id` in `scope` - its owner's or the global one -,
    /// beyond the session it was published in (`UpdateArtifact`, its
    /// owner's act, asked by the store). The scope is compared with the
    /// artifact's owner before: that tells the caller nothing of others
    /// (issue 312).
    ///
    /// # Errors
    /// `NotFound`, `InvalidScope` for another scope, or whatever the store
    /// reports - `PermissionDenied` and `NameTaken` among it.
    pub async fn keep(&self, ctx: &SecurityContext, id: ArtifactId, scope: Scope) -> ArtifactResult<Artifact> {
        let _writing = self.writing.lock().await;
        let mut artifact = self.artifacts.get(ctx, id).await.map_err(store_error(id))?;
        match (&scope, artifact.owner()) {
            (Scope::GlobalScope, _) => {}
            (Scope::UserScope { user_id }, Some(owner)) if *user_id == owner => {}
            _ => return Err(ArtifactError::InvalidScope),
        }
        artifact.name.scope = scope;
        artifact.updated_at = Utc::now();
        self.artifacts.update(ctx, &artifact).await?;
        self.events.publish_classified(GlobalEvent::updated::<Artifact>(&id, &ArtifactUpdateEvent::ScopeChanged(artifact.name.scope.clone())), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
        info!(artifact = %id, by = %ctx.caller_id, "Artifact kept");
        Ok(artifact)
    }

    /// Says what artifact `id` is; `None` or empty takes it away
    /// (`UpdateArtifact`, its owner's act, asked by the store).
    ///
    /// # Errors
    /// `NotFound`, `Invalid` for a description that does not fit, or
    /// whatever the store reports - `PermissionDenied` among it.
    pub async fn describe(&self, ctx: &SecurityContext, id: ArtifactId, description: Option<&str>) -> ArtifactResult<Artifact> {
        let _writing = self.writing.lock().await;
        let mut artifact = self.artifacts.get(ctx, id).await.map_err(store_error(id))?;
        artifact.description = Description::optional(description)?;
        artifact.updated_at = Utc::now();
        self.artifacts.update(ctx, &artifact).await?;
        self.events.publish_classified(GlobalEvent::updated::<Artifact>(&id, &ArtifactUpdateEvent::DescriptionChanged(artifact.description.as_ref().map(ToString::to_string))), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
        Ok(artifact)
    }

    /// Deletes artifact `id` (`DeleteArtifact`), its content kept until it
    /// is purged; `false` if there was none the caller could reach, by the store.
    ///
    /// # Errors
    /// Whatever the store reports, `PermissionDenied` included.
    pub async fn delete(&self, ctx: &SecurityContext, id: ArtifactId) -> ArtifactResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(artifact) = self.artifacts.get(ctx, id).await.optional()? else { return Ok(false) };
        if !self.artifacts.delete(ctx, id).await? {
            return Ok(false);
        }
        self.events.publish_classified(GlobalEvent::deleted::<Artifact>(&id), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
        info!(artifact = %id, by = %ctx.caller_id, "Artifact deleted");
        Ok(true)
    }

    /// Takes the deletion of artifact `id` back (`Restore`). Returns the
    /// artifact as it is restored.
    ///
    /// # Errors
    /// `NotFound` if it is not deleted or not the caller's to restore, or
    /// whatever else the store reports - `NameTaken` among it.
    pub async fn restore(&self, ctx: &SecurityContext, id: ArtifactId) -> ArtifactResult<Artifact> {
        let _writing = self.writing.lock().await;
        let artifact = self.artifacts.restore(ctx, id).await.map_err(store_error(id))?;
        self.events.publish_classified(GlobalEvent::restored::<Artifact>(&id), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
        info!(artifact = %id, by = %ctx.caller_id, "Artifact restored");
        Ok(artifact)
    }

    /// Removes the deleted artifact `id` for good, its content first
    /// (`Purge`, asked before anything is removed). Content that cannot be
    /// removed leaves the document, so that the next purge tries again.
    /// `false` if there was nothing deleted the caller could see.
    ///
    /// # Errors
    /// `PermissionDenied`, `UnknownStorage`, `Unavailable`, or whatever
    /// the store reports.
    pub async fn purge(&self, ctx: &SecurityContext, id: ArtifactId) -> ArtifactResult<bool> {
        let _writing = self.writing.lock().await;
        let Some(artifact) = self.artifacts.get_deleted(ctx, id).await.optional()? else { return Ok(false) };
        self.security.require(ctx, ActionType::purge(Artifact::KIND, &artifact.security_attributes), StoreError::PermissionDenied).await?;
        self.storage(&artifact.storage)?.remove(id).await?;
        let purged = self.artifacts.purge(ctx, id).await?;
        info!(artifact = %id, by = %ctx.caller_id, "Artifact purged");
        Ok(purged)
    }
}

#[async_trait]
impl ScopeMember for ArtifactManager {
    fn kind(&self) -> EntityKind {
        Artifact::KIND
    }

    /// Deletes the artifacts that lie in `holder` or within it, their
    /// content kept, on behalf of `ctx` (`scoped`), as far as they may, and
    /// announces them as `delete` does; answers how many are left.
    ///
    /// # Errors
    /// Whatever else the store reports; what was deleted before stays
    /// deleted.
    async fn delete_lying_in(&self, ctx: &SecurityContext, holder: &Scope) -> StoreResult<usize> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        let mut left = 0;
        for artifact in self.artifacts.list(&system, &within(holder)).await?.items {
            if deleted_unless_refused(self.artifacts.delete(ctx, artifact.id).await)? {
                self.events.publish_classified(GlobalEvent::deleted::<Artifact>(&artifact.id), artifact.security_attributes.clone(), std::slice::from_ref(&artifact.classification));
            } else {
                left += 1;
            }
        }
        Ok(left)
    }

    /// Restores the artifacts that lie in `holder` or within it and were
    /// deleted at `since` or later on behalf of `ctx` (`scoped`), and
    /// announces them as `restore` does. One whose name was taken meanwhile
    /// stays deleted, as does one `ctx` may not restore.
    ///
    /// # Errors
    /// Whatever else the store reports; what was restored before stays
    /// restored.
    async fn restore_lying_in(&self, ctx: &SecurityContext, holder: &Scope, since: DateTime<Utc>) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for artifact in deleted_since(self.artifacts.list_deleted(&system, &within(holder)).await?.items, since) {
            if let Some(restored) = restored_unless_refused(self.artifacts.restore(ctx, artifact.id).await, &artifact)? {
                self.events.publish_classified(GlobalEvent::restored::<Artifact>(&restored.id), restored.security_attributes.clone(), std::slice::from_ref(&restored.classification));
            }
        }
        Ok(())
    }

    /// Purges the deleted artifacts that lie in `holder` or within it,
    /// their content first.
    ///
    /// # Errors
    /// `UnknownStorage`, `Unavailable`, or whatever the store reports;
    /// what was purged before stays purged.
    async fn purge_lying_in(&self, holder: &Scope) -> StoreResult<()> {
        let system = SecurityContext::system();
        let _writing = self.writing.lock().await;
        for artifact in self.artifacts.list_deleted(&system, &within(holder)).await?.items {
            // Its content is its storage too.
            let unremoved = |e: ArtifactError| StoreError::Backend(format!("the content of artifact {} cannot be removed: {e}", artifact.id));
            self.storage(&artifact.storage).map_err(unremoved)?.remove(artifact.id).await.map_err(unremoved)?;
            self.artifacts.purge(&system, artifact.id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::config::parse_yaml;

    #[test]
    fn artifacts_are_kept_in_the_storage_publish_to_names_or_the_one_there_is() {
        let read = |yaml: &str| parse_yaml::<ArtifactsConfig>(yaml).expect("a section of artifacts");
        let one = read("storage:\n  files:\n    type: local\n    root: local/artifacts\n");
        assert_eq!(one.publish_to(), Some("files"));
        assert_eq!(one.max_bytes, DEFAULT_MAX_FILE_BYTES);
        assert_eq!(read("{}").publish_to(), None, "none configured");
        let several = "storage:\n  a:\n    type: local\n    root: a\n  b:\n    type: local\n    root: b\n";
        assert!(read(several).check().is_err(), "which one, among several, must be said");
        let chosen = read(&format!("{several}publish_to: b\n"));
        assert!(chosen.check().is_ok());
        assert_eq!(chosen.publish_to(), Some("b"));
        assert!(read(&format!("{several}publish_to: c\n")).check().is_err(), "not one that is not there");
    }
}
