//! The `AttachmentManager`: what is uploaded or copied for a message - checked, kept and waiting
//! for the message that claims it - or given a model by a tool, what a message may carry as its
//! channel admits it, and the content of an attachment for a download or for a model given it
//! along.

use super::cache::ContentCache;
use crate::backend::NamedBackends;
use crate::content::{keep_digested, ContentStorage};
use bytes::{Bytes, BytesMut};
use caelumora_api::component::{ComponentError, ComponentResult};
use caelumora_api::error::ContentError;
use caelumora_api::error::{AttachmentError, AttachmentResult};
use caelumora_api::terminal::ByteStream;
use caelumora_domain::actor::ActorId;
use caelumora_domain::channel::{Channel, ChannelId};
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::config::AttachmentsConfig;
use caelumora_domain::content::{self, ContentId, ContentRef};
use caelumora_domain::context::ModelProfile;
use caelumora_domain::digest;
use caelumora_domain::message::Message;
use caelumora_domain::tool::ToolContent;
use caelumora_domain::value::text::FileName;
use caelumora_security::confidentiality::{ConfidentialityPolicy, ConfidentialityQuestions};
use caelumora_security::SecurityContext;
use futures::{stream, StreamExt};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, warn};

/// How many bytes of an upload tell what it is (`content::media_type`).
const HEAD: usize = 512;

/// The key space of attachments in the storages of content.
const KEY_SPACE: &str = "attachments";

/// The key every attachment of channel `channel` lies under.
fn channel_key(channel: ChannelId) -> String {
    format!("{KEY_SPACE}/{channel}")
}

/// Who keeps content for a message in `channel`: the caller of `ctx`, who takes part in it.
///
/// # Errors
/// `NotAParticipant` for a caller who does not.
fn uploader(ctx: &SecurityContext, channel: &Channel) -> AttachmentResult<ActorId> {
    let by = ctx.caller_id;
    channel.lists(by).then_some(by).ok_or(AttachmentError::NotAParticipant(by))
}

/// A new id for content of channel `channel`, and the key its bytes lie under.
fn new_key(channel: ChannelId) -> (ContentId, String) {
    let id = ContentId::new();
    (id, format!("{}/{id}", channel_key(channel)))
}

/// Content kept for a message yet to come: who it was kept by, for which channel, and when.
#[derive(Debug)]
struct Pending {
    by: ActorId,
    channel: ChannelId,
    content: ContentRef,
    at: Instant,
}

/// The one owner of the content of attachments (issue 282): what is uploaded for a message, kept
/// and waiting for it, claimed by it as its channel admits it, read for a download or for a model
/// given it along, and removed with its channel - the only code that keeps content in the key
/// space `attachments`.
pub struct AttachmentManager {
    /// What may flow where (issue 304): into the channel, into the storage.
    confidentiality: Arc<dyn ConfidentialityPolicy>,
    storages: Arc<NamedBackends<dyn ContentStorage>>,
    /// The storage uploads are kept in, with its name.
    keep_in: Option<(String, Arc<dyn ContentStorage>)>,
    max_bytes: u64,
    max_per_message: usize,
    /// How long an upload waits for its message.
    unclaimed_expire: Duration,
    /// The uploads no message claimed yet, in memory: what a restart loses has never been part of
    /// anything, and lies in its channel's key space until the channel is purged.
    uploads: Mutex<HashMap<ContentId, Pending>>,
    /// What a tool gave its model, for as long as an upload waits: how the turn that made the call
    /// reaches it before its channel has recorded the message of the result (`kept_content`). Only
    /// remembered, in memory: nothing here is removed from a storage, since files wait for no
    /// message, and a call waiting for its approval may take longer than this.
    kept: Mutex<HashMap<ContentId, Pending>>,
    /// The content models are given along, held for their next calls.
    cache: ContentCache,
}

impl fmt::Debug for AttachmentManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentManager")
            .field("keep_in", &self.keep_in.as_ref().map(|(name, _)| name))
            .finish_non_exhaustive()
    }
}

impl AttachmentManager {
    /// The manager of the attachments kept in `storages` by their names, uploads in the storage
    /// `config` says.
    ///
    /// # Errors
    /// `ComponentError::InvalidConfig` where `config` names no storage of `storages`, or none among
    /// several.
    pub fn new(
        confidentiality: Arc<dyn ConfidentialityPolicy>,
        storages: Arc<NamedBackends<dyn ContentStorage>>,
        config: &AttachmentsConfig,
    ) -> ComponentResult<Self> {
        let keep_in = storages
            .chosen(config.keep_in.as_deref())
            .map_err(|why| ComponentError::InvalidConfig(format!("attachments.keep_in {why}")))?
            .map(|(name, storage)| (name.to_string(), Arc::clone(storage)));
        Ok(Self {
            confidentiality,
            storages,
            keep_in,
            max_bytes: config.max_bytes,
            max_per_message: config.max_per_message,
            unclaimed_expire: Duration::from_secs(config.unclaimed_expire_secs),
            uploads: Mutex::new(HashMap::new()),
            kept: Mutex::new(HashMap::new()),
            cache: ContentCache::new(config.cache_bytes),
        })
    }

    /// Keeps `content` - its size and its bytes - as the file `name`, uploaded by the caller of
    /// `ctx` for a message they will say in `channel` - the channel as it stands, handed over by
    /// whoever reached it in the caller's name - (issue 282): classified `classification` - the
    /// channel's if `None` -, which may flow into the channel and into the storage it is kept in,
    /// by a caller taking part in the channel. What it is, is told by its first bytes, never by
    /// the name. It waits for the message that claims it (`named`, `claim`) as long as configured,
    /// and is removed once it waited longer, as the next upload is kept.
    ///
    /// # Errors
    /// `NotConfigured`, `NotAParticipant`, `Invalid` for a name that is none,
    /// `ClassificationRefused`, `StorageNotCleared`, `Content` for content too large, other than
    /// said, or a storage that failed.
    pub async fn create(
        &self,
        ctx: &SecurityContext,
        channel: &Channel,
        name: &str,
        classification: Option<Classification>,
        (size, content): (u64, ByteStream),
    ) -> AttachmentResult<ContentRef> {
        let by = uploader(ctx, channel)?;
        let name = FileName::new(name)?;
        let classification = classification.unwrap_or_else(|| channel.classification.clone());
        let storage = self.storage_for(ctx, channel, &classification)?;
        self.forget_expired().await;

        let (head, content) = head_of(content).await;
        let media_type = content::media_type(&head);
        let content = self
            .keep(storage, channel.id, name, classification, media_type, (size, content))
            .await?;
        self.wait(by, channel.id, content.clone());
        debug!(attachment = %content.id, channel = %channel.id, by = %by, size, "Attachment uploaded");
        Ok(content)
    }

    /// Keeps `files`, what a tool gave its model for the result of a call the caller of `ctx` makes
    /// in `channel`, handed over as for `create` (issue 512): classified `classification`, as the
    /// result, kept as an upload is, but waiting for no message - the result refers to them from
    /// the moment they are kept, and the channel admits what lies in its key space (`lies_in`);
    /// so it is a way of creating of its own, apart from `create`, whose uploads wait. Each is kept
    /// byte for byte, whatever it is, labelled as its first bytes say (`content::media_type`), and
    /// there are at most as many as a message may carry attachments - checked before any is kept.
    /// All are kept or none: what was kept before one failed is removed again
    /// (`roll_back_content`). What is kept is remembered for its caller (`kept_content`).
    ///
    /// # Errors
    /// As `create`; `TooMany` for more than a message may carry.
    pub async fn create_content(
        &self,
        ctx: &SecurityContext,
        channel: &Channel,
        classification: Classification,
        files: Vec<ToolContent>,
    ) -> AttachmentResult<Vec<ContentRef>> {
        let by = uploader(ctx, channel)?;
        if files.len() > self.max_per_message {
            return Err(AttachmentError::TooMany(self.max_per_message));
        }
        let files = files
            .into_iter()
            .map(|ToolContent { name, bytes }| {
                let name = FileName::new(&name)?;
                let media_type = content::media_type(&bytes[..bytes.len().min(HEAD)]);
                Ok((name, media_type, bytes))
            })
            .collect::<AttachmentResult<Vec<_>>>()?;
        let storage = self.storage_for(ctx, channel, &classification)?;
        self.forget_expired().await;

        let mut kept = Vec::with_capacity(files.len());
        for (name, media_type, bytes) in files {
            let size = bytes.len() as u64;
            let stream: ByteStream = Box::pin(stream::iter([Ok(bytes)]));
            let classification = classification.clone();
            let given =
                self.keep(storage, channel.id, name, classification, media_type, (size, stream));
            match given.await {
                Ok(given) => kept.push(given),
                Err(e) => {
                    let (_, kept_in) = storage;
                    self.roll_back_content(kept_in.as_ref(), &kept).await;
                    return Err(e);
                }
            }
        }
        let mut remembered = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        for content in &kept {
            let pending =
                Pending { by, channel: channel.id, at: Instant::now(), content: content.clone() };
            remembered.insert(content.id, pending);
        }
        drop(remembered);
        debug!(channel = %channel.id, by = %by, count = kept.len(), "Content of a tool kept");
        Ok(kept)
    }

    /// What `create_content` kept as `id` for `by` in `channel`: the reference the kernel made, for
    /// the turn that made the call, whose channel may not have recorded the message of the result
    /// when the turn's next step reads it (issue 547). `None` for anybody else, in any other
    /// channel, and once it was remembered longer than an upload waits.
    pub(crate) fn kept_content(
        &self,
        by: ActorId,
        channel: ChannelId,
        id: ContentId,
    ) -> Option<ContentRef> {
        let remembered = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let kept =
            remembered.get(&id).filter(|kept| kept.is_for(by, channel, self.unclaimed_expire))?;
        Some(kept.content.clone())
    }

    /// The storage content classified `classification` is kept in for the caller of `ctx` in
    /// `channel`, with its name: where it may flow into the channel and into the storage.
    ///
    /// # Errors
    /// `ClassificationRefused`, `NotConfigured`, `StorageNotCleared`.
    fn storage_for(
        &self,
        ctx: &SecurityContext,
        channel: &Channel,
        classification: &Classification,
    ) -> AttachmentResult<(&str, &Arc<dyn ContentStorage>)> {
        self.confidentiality
            .require_classifiable(classification, || AttachmentError::ClassificationRefused)?;
        self.confidentiality.require_publish(
            ctx,
            classification,
            &channel.classification,
            || AttachmentError::ClassificationRefused,
        )?;
        let (storage_name, storage) =
            self.keep_in.as_ref().ok_or(AttachmentError::NotConfigured)?;
        self.confidentiality.require_storable(
            ctx,
            classification,
            storage.destination(),
            || AttachmentError::StorageNotCleared(storage_name.to_string()),
        )?;
        Ok((storage_name, storage))
    }

    /// Keeps `content` - its size and its bytes, of `media_type` as they say - as the file `name`
    /// of channel `channel` in `storage`, classified `classification`, under an id and a key of its
    /// own.
    ///
    /// # Errors
    /// `Content` for content too large, other than said, or a storage that failed.
    async fn keep(
        &self,
        (storage_name, storage): (&str, &Arc<dyn ContentStorage>),
        channel: ChannelId,
        name: FileName,
        classification: Classification,
        media_type: &str,
        (size, content): (u64, ByteStream),
    ) -> AttachmentResult<ContentRef> {
        let (id, key) = new_key(channel);
        let digest = keep_digested(storage.as_ref(), &key, size, content, self.max_bytes).await?;
        Ok(ContentRef {
            id,
            name,
            media_type: media_type.to_string(),
            size,
            digest,
            classification,
            storage: storage_name.to_string(),
            key,
            number: None,
            content: None,
            left_out: false,
        })
    }

    /// Removes `kept` from `storage` again, the files of a result that could not all be kept: in
    /// the kernel's name, since keeping them was permitted. What cannot be removed is logged and
    /// stays, until its channel is purged.
    async fn roll_back_content(&self, storage: &dyn ContentStorage, kept: &[ContentRef]) {
        for content in kept {
            if let Err(e) = storage.delete(&content.key).await {
                warn!(attachment = %content.id, "Content of a tool not removed again: {e}");
            }
        }
    }

    /// The attachments `ids` name, in their order, for a message `by` says in `channel`: each one
    /// the message carried before (`kept`), or one they uploaded for the channel that no message
    /// claimed yet.
    ///
    /// # Errors
    /// `NotFound` for one that is neither.
    pub(crate) fn named(
        &self,
        by: ActorId,
        channel: ChannelId,
        ids: &[ContentId],
        kept: &[ContentRef],
    ) -> AttachmentResult<Vec<ContentRef>> {
        let uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        ids.iter()
            .map(|id| {
                let uploaded = uploads
                    .get(id)
                    .filter(|upload| upload.is_for(by, channel, self.unclaimed_expire))
                    .map(|upload| &upload.content);
                kept.iter()
                    .find(|content| content.id == *id)
                    .or(uploaded)
                    .cloned()
                    .ok_or(AttachmentError::NotFound(*id))
            })
            .collect()
    }

    /// Claims `attachments` for a message `by` says in `channel`, and answers whether it may carry
    /// them (issue 282): as many as a message may carry at most, none twice, each one the message
    /// carried before (`kept`), or one they uploaded for the channel, as it was uploaded, that no
    /// message claimed yet. Those uploaded are claimed then - no other message can carry them, and
    /// they are not removed as they wait -; nothing is if the message may not carry them. Checked
    /// and claimed at once, so that nothing expires in between: the last thing the channel does as
    /// it admits the message.
    pub(crate) fn claim(
        &self,
        by: ActorId,
        channel: ChannelId,
        attachments: &[ContentRef],
        kept: &[ContentRef],
    ) -> bool {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        let mut seen = HashSet::new();
        let carried = attachments.len() <= self.max_per_message
            && attachments.iter().all(|content| {
                let uploaded = || {
                    uploads.get(&content.id).is_some_and(|upload| {
                        upload.is_for(by, channel, self.unclaimed_expire)
                            && upload.content == *content
                    })
                };
                seen.insert(content.id) && (kept.contains(content) || uploaded())
            });
        if carried {
            for content in attachments {
                uploads.remove(&content.id);
            }
        }
        carried
    }

    /// Whether `content` lies in the key space of channel `channel`: one of its attachments, or a
    /// copy of one (issue 282).
    pub(crate) fn lies_in(&self, channel: ChannelId, content: &ContentRef) -> bool {
        content.key.strip_prefix(&channel_key(channel)).is_some_and(|rest| rest.starts_with('/'))
    }

    /// The content of the attachment `content`, with its size - asking nothing: the view it is read
    /// through decided who may (`ChannelView::attachment`).
    ///
    /// # Errors
    /// `NotFound` if its storage has nothing under its key, `StorageGone`, `Content`.
    pub(crate) async fn content_of(
        &self,
        content: &ContentRef,
    ) -> AttachmentResult<(u64, ByteStream)> {
        self.storages.kept_in(&content.storage)?.get(&content.key).await.map_err(|e| match e {
            ContentError::NotFound(_) => AttachmentError::NotFound(content.id),
            e => e.into(),
        })
    }

    /// A copy of `attachment` - one of the channel's, as the caller of `ctx` read it there through
    /// its view (`ChannelView::attachment`), never as someone else hands it over (issue 549) - for
    /// a message the caller will say in `channel`, handed over as for `create` (issue 282): the
    /// same content, label and name under an id and a key of its own in the same storage, waiting
    /// for the message that claims it as an upload does. What an agent shows of what it heard, so
    /// that each message holds its own.
    ///
    /// # Errors
    /// `NotAParticipant`; `NotFound` for content its storage has nothing of; `StorageGone`,
    /// `Content`.
    pub(crate) async fn copy(
        &self,
        ctx: &SecurityContext,
        channel: &Channel,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        let by = uploader(ctx, channel)?;
        self.forget_expired().await;
        let (id, key) = new_key(channel.id);
        self.storages.kept_in(&attachment.storage)?.copy(&attachment.key, &key).await.map_err(
            |e| match e {
                ContentError::NotFound(_) => AttachmentError::NotFound(attachment.id),
                e => e.into(),
            },
        )?;
        let copy = ContentRef { id, key, number: None, content: None, ..attachment.clone() };
        self.wait(by, channel.id, copy.clone());
        debug!(attachment = %id, of = %attachment.id, channel = %channel.id, by = %by, "Attachment copied");
        Ok(copy)
    }

    /// Lets `content`, kept for a message `by` will say in `channel`, wait for that message.
    fn wait(&self, by: ActorId, channel: ChannelId, content: ContentRef) {
        let upload = Pending { by, channel, at: Instant::now(), content };
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(upload.content.id, upload);
    }

    /// Puts their content on the attachments of `messages`, and on what their tool results gave and
    /// showed, that the model of `profile` is given along (issues 282, 512) - held in memory, else
    /// read from its storage and checked against its digest -, and answers how each is classified:
    /// what the model is given of them. Asking nothing: what lies in an agent's transcript, it
    /// heard (`ActorRuntime::load_attachments`).
    ///
    /// # Errors
    /// `NotFound` if the storage has nothing under an attachment's key, `Altered` if its bytes are
    /// not what the message says they are, `StorageGone`, `Content`.
    pub(crate) async fn load(
        &self,
        messages: &mut [Message],
        profile: &ModelProfile,
    ) -> AttachmentResult<Vec<Classification>> {
        // Collected first: what is held across the reads must be `Send`.
        let carried: Vec<&mut ContentRef> = messages
            .iter_mut()
            .flat_map(Message::carried_mut)
            .filter(|content| profile.gives_along(content))
            .collect();
        let mut given = Vec::with_capacity(carried.len());
        for attachment in carried {
            attachment.content = Some(self.content(attachment).await?);
            given.push(attachment.classification.clone());
        }
        Ok(given)
    }

    /// The bytes of the attachment `attachment`, held in memory or read from its storage - and then
    /// held -, checked against its digest.
    async fn content(&self, attachment: &ContentRef) -> AttachmentResult<Bytes> {
        if let Some(content) = self.cache.get(attachment) {
            return Ok(content);
        }
        let (size, mut chunks) = self.content_of(attachment).await?;
        if size != attachment.size {
            return Err(AttachmentError::Altered(attachment.id));
        }
        let mut content = BytesMut::with_capacity(usize::try_from(size).unwrap_or_default());
        while let Some(chunk) = chunks.next().await {
            content.extend_from_slice(&chunk.map_err(ContentError::unavailable)?);
        }
        let content = content.freeze();
        if digest::content(&content) != attachment.digest {
            return Err(AttachmentError::Altered(attachment.id));
        }
        self.cache.put(attachment, content.clone());
        Ok(content)
    }

    /// Removes every attachment of the channel `channel` for good (issue 282), with the history
    /// that carries them: from every storage, since one configured before may hold some, and the
    /// uploads that wait for a message there and the files remembered for it.
    ///
    /// # Errors
    /// `Content` if a storage cannot remove them.
    pub(crate) async fn purge(&self, channel: ChannelId) -> AttachmentResult<()> {
        let prefix = channel_key(channel);
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, upload| upload.channel != channel);
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, kept| kept.channel != channel);
        self.cache.forget_under(&format!("{prefix}/"));
        for (_, storage) in self.storages.iter() {
            storage.remove_under(&prefix).await?;
        }
        Ok(())
    }

    /// Removes the uploads that waited longer than configured for their message with their content,
    /// and forgets the files remembered as long, whose content stays. Logged where removing fails:
    /// the content lies in its channel's key space until the channel is purged.
    async fn forget_expired(&self) {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, kept| kept.at.elapsed() <= self.unclaimed_expire);
        let expired: Vec<ContentRef> = {
            let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
            let ids: Vec<ContentId> = uploads
                .iter()
                .filter(|(_, upload)| upload.at.elapsed() > self.unclaimed_expire)
                .map(|(id, _)| *id)
                .collect();
            ids.iter().filter_map(|id| uploads.remove(id)).map(|upload| upload.content).collect()
        };
        for content in expired {
            let removed = match self.storages.kept_in(&content.storage) {
                Ok(storage) => storage.delete(&content.key).await.map_err(AttachmentError::from),
                Err(e) => Err(e.into()),
            };
            if let Err(e) = removed {
                warn!(attachment = %content.id, "An upload no message claimed stays: {e}");
            }
        }
    }
}

impl Pending {
    /// Whether it is for a message `by` says in `channel`: theirs, for that channel, and not
    /// waiting longer than `expire`.
    fn is_for(&self, by: ActorId, channel: ChannelId, expire: Duration) -> bool {
        self.by == by && self.channel == channel && self.at.elapsed() <= expire
    }
}

/// What `content` begins with - at least `HEAD` bytes where it has as many -, and the whole of it,
/// to be read from its start.
async fn head_of(mut content: ByteStream) -> (Vec<u8>, ByteStream) {
    let mut head = Vec::new();
    let mut read = Vec::new();
    while head.len() < HEAD {
        match content.next().await {
            Some(Ok(chunk)) => {
                head.extend_from_slice(&chunk);
                read.push(Ok(chunk));
            }
            Some(Err(e)) => {
                read.push(Err(e));
                break;
            }
            None => break,
        }
    }
    (head, Box::pin(stream::iter(read).chain(content)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionLoop;
    use crate::testing::World;
    use bytes::Bytes;
    use caelumora_domain::confidentiality::{Compartment, Confidentiality};
    use caelumora_domain::digest;
    use caelumora_domain::tool::{ToolCall, ToolCallResult};
    use caelumora_domain::user::UserId;
    use caelumora_security::SecurityContext;
    use serde_json::Value;
    use std::fs;
    use std::slice;
    use tokio::time;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nand the rest of it";

    fn bytes(content: &'static [u8]) -> (u64, ByteStream) {
        (content.len() as u64, Box::pin(stream::iter([Ok(Bytes::from_static(content))])))
    }

    async fn read((_, content): (u64, ByteStream)) -> Vec<u8> {
        content
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flat_map(|chunk| chunk.unwrap().to_vec())
            .collect()
    }

    /// A png in memory, as a tool gives it, named `name`.
    fn png(name: &str) -> ToolContent {
        ToolContent { name: name.to_string(), bytes: Bytes::from_static(PNG) }
    }

    /// The channel "main" of `session` as it stands.
    fn main_of(session: &SessionLoop) -> Arc<Channel> {
        session.channels()[0].channel()
    }

    #[tokio::test]
    async fn an_upload_is_kept_as_its_bytes_say_and_claimed_by_one_message_of_its_uploader() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let (alice, bob) = (ActorId::User(world.alice), ActorId::User(world.bob));
        let attachments = world.kernel.attachment_manager();

        let uploaded = attachments
            .create(&hers, &main, "chart.txt", None, bytes(PNG))
            .await
            .expect("hers to upload");
        assert_eq!(
            (uploaded.name.as_str(), uploaded.media_type.as_str(), uploaded.size),
            ("chart.txt", "image/png", PNG.len() as u64),
            "what it is, as its bytes say"
        );
        assert_eq!(uploaded.digest, digest::content(PNG));
        assert_eq!(uploaded.classification, main.classification, "the channel's, unless given");
        assert_eq!(
            (uploaded.storage.as_str(), uploaded.key.clone()),
            ("kept", format!("attachments/{}/{}", main.id, uploaded.id))
        );
        assert_eq!(read(attachments.content_of(&uploaded).await.unwrap()).await, PNG);

        let ids = [uploaded.id];
        assert_eq!(
            attachments.named(alice, main.id, &ids, &[]).unwrap(),
            slice::from_ref(&uploaded),
            "hers, for the channel"
        );
        assert!(
            matches!(attachments.named(bob, main.id, &ids, &[]), Err(AttachmentError::NotFound(_))),
            "not his"
        );
        assert!(
            matches!(
                attachments.named(alice, ChannelId::new(), &ids, &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "not for another channel"
        );
        assert!(
            !attachments.claim(bob, main.id, slice::from_ref(&uploaded), &[]),
            "not his to claim"
        );
        assert!(
            !attachments.claim(alice, ChannelId::new(), slice::from_ref(&uploaded), &[]),
            "not for another channel"
        );
        assert!(
            !attachments.claim(alice, main.id, &[uploaded.clone(), uploaded.clone()], &[]),
            "none twice"
        );
        let elsewhere = ContentRef {
            key: format!("attachments/{}/{}", ChannelId::new(), uploaded.id),
            ..uploaded.clone()
        };
        assert!(!attachments.claim(alice, main.id, &[elsewhere], &[]), "only as it was uploaded");

        assert!(
            attachments.claim(alice, main.id, slice::from_ref(&uploaded), &[]),
            "nothing claimed by what was refused"
        );
        assert!(
            !attachments.claim(alice, main.id, slice::from_ref(&uploaded), &[]),
            "claimed once"
        );
        assert!(
            attachments.claim(
                alice,
                main.id,
                slice::from_ref(&uploaded),
                slice::from_ref(&uploaded)
            ),
            "kept by the message that has it"
        );
        assert!(matches!(
            attachments.named(alice, main.id, &ids, &[]),
            Err(AttachmentError::NotFound(_))
        ));
        assert_eq!(read(attachments.content_of(&uploaded).await.unwrap()).await, PNG, "and kept");

        let copy =
            attachments.copy(&hers, &main, &uploaded).await.expect("a copy for her next message");
        assert!(
            copy.id != uploaded.id
                && copy.key != uploaded.key
                && (copy.digest, &copy.classification)
                    == (uploaded.digest, &uploaded.classification),
            "its own id and key, the same content and label"
        );
        assert_eq!(read(attachments.content_of(&copy).await.unwrap()).await, PNG);
        assert!(
            attachments.lies_in(main.id, &copy) && !attachments.lies_in(ChannelId::new(), &copy),
            "in the channel's key space alone"
        );
        assert!(
            attachments.claim(alice, main.id, slice::from_ref(&copy), &[]),
            "waiting for her message as an upload does"
        );
    }

    #[tokio::test]
    async fn what_a_tool_gave_is_kept_for_the_channel_as_images_all_or_none_waiting_for_nothing() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let alice = ActorId::User(world.alice);
        let attachments = world.kernel.attachment_manager();
        let classification = main.classification.clone();
        let kept = || world.attachments_kept().len();

        let drawn = attachments
            .create_content(&hers, &main, classification.clone(), vec![png("a.png"), png("b.png")])
            .await
            .expect("hers, in a channel she takes part in");
        assert_eq!(
            drawn.iter().map(|d| (d.name.as_str(), d.media_type.as_str())).collect::<Vec<_>>(),
            [("a.png", "image/png"), ("b.png", "image/png")]
        );
        assert!(drawn
            .iter()
            .all(|d| d.classification == classification && attachments.lies_in(main.id, d)));
        assert_eq!(read(attachments.content_of(&drawn[0]).await.unwrap()).await, PNG, "kept");
        let given = |name: &str, bytes: &'static [u8]| ToolContent {
            name: name.to_string(),
            bytes: Bytes::from_static(bytes),
        };
        let others = vec![
            given("notes.txt", b"n"),
            given("scan.pdf", b"%PDF-1.7"),
            given("colours.txt", b"\x1b[1m\x00"),
        ];
        let others = attachments
            .create_content(&hers, &main, classification.clone(), others)
            .await
            .expect("what a tool gave is kept, whatever it is");
        assert_eq!(
            others.iter().map(|o| o.media_type.as_str()).collect::<Vec<_>>(),
            ["text/plain", "application/pdf", "application/octet-stream"]
        );
        assert_eq!(read(attachments.content_of(&others[2]).await.unwrap()).await, b"\x1b[1m\x00");
        assert!(
            matches!(
                attachments.named(alice, main.id, &[drawn[0].id], &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "waiting for no message: its result refers to it"
        );
        let before = kept();

        let many = vec![png("a.png"); AttachmentsConfig::default().max_per_message + 1];
        assert!(matches!(
            attachments.create_content(&hers, &main, classification.clone(), many).await,
            Err(AttachmentError::TooMany(_))
        ));
        let mut huge = PNG.to_vec();
        huge.resize(usize::try_from(AttachmentsConfig::default().max_bytes).unwrap() + 1, 0);
        let huge = ToolContent { name: "huge.png".to_string(), bytes: Bytes::from(huge) };
        let too_large = vec![png("a.png"), huge];
        assert!(matches!(
            attachments.create_content(&hers, &main, classification.clone(), too_large).await,
            Err(AttachmentError::Content(ContentError::TooLarge { .. }))
        ));
        assert_eq!(kept(), before, "all or none: what was kept before one failed is gone again");
        let above = Classification::general(Confidentiality::StrictlyConfidential);
        assert!(matches!(
            attachments.create_content(&hers, &main, above, vec![png("a.png")]).await,
            Err(AttachmentError::ClassificationRefused)
        ));
        let bobs = World::as_user(UserId::new());
        assert!(matches!(
            attachments.create_content(&bobs, &main, classification, vec![png("a.png")]).await,
            Err(AttachmentError::NotAParticipant(_))
        ));
    }

    #[tokio::test]
    async fn an_upload_is_refused_unless_it_may_enter_the_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let attachments = world.kernel.attachment_manager();

        assert!(
            matches!(
                attachments
                    .create(&SecurityContext::system(), &main, "a.txt", None, bytes(b"a"))
                    .await,
                Err(AttachmentError::NotAParticipant(ActorId::System))
            ),
            "who takes no part in it"
        );
        let above = Classification::general(Confidentiality::Confidential);
        assert!(
            matches!(
                attachments.create(&hers, &main, "a.txt", Some(above), bytes(b"a")).await,
                Err(AttachmentError::ClassificationRefused)
            ),
            "above the channel"
        );
        let unknown =
            Classification::new(Confidentiality::Internal, [Compartment::new("hr")]).unwrap();
        assert!(
            matches!(
                attachments.create(&hers, &main, "a.txt", Some(unknown), bytes(b"a")).await,
                Err(AttachmentError::ClassificationRefused)
            ),
            "a compartment not configured"
        );
        assert!(
            matches!(
                attachments.create(&hers, &main, "../a.txt", None, bytes(b"a")).await,
                Err(AttachmentError::Invalid(_))
            ),
            "a path for a name"
        );
        let (_, content) = bytes(b"a");
        let too_large = AttachmentsConfig::default().max_bytes + 1;
        assert!(matches!(
            attachments.create(&hers, &main, "a.txt", None, (too_large, content)).await,
            Err(AttachmentError::Content(ContentError::TooLarge { .. }))
        ));
        let (_, content) = bytes(b"abc");
        assert!(
            matches!(
                attachments.create(&hers, &main, "a.txt", None, (2, content)).await,
                Err(AttachmentError::Content(ContentError::Changed))
            ),
            "other than it was said to be"
        );
        assert!(world.attachments_kept().is_empty(), "nothing kept of any of them");
        assert!(
            attachments
                .create(&hers, &main, "a.txt", Some(Classification::PUBLIC), bytes(b"a"))
                .await
                .is_ok(),
            "released, below the channel"
        );
    }

    #[tokio::test]
    async fn what_a_model_is_given_along_is_loaded_checked_and_held() {
        use caelumora_domain::context::ModelProfile;
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let attachments = world.kernel.attachment_manager();
        let image = attachments.create(&hers, &main, "chart.png", None, bytes(PNG)).await.unwrap();
        let text =
            attachments.create(&hers, &main, "notes.txt", None, bytes(b"notes")).await.unwrap();
        let drawn = attachments
            .create_content(&hers, &main, main.classification.clone(), vec![png("drawn.png")])
            .await
            .unwrap();
        let showing = ToolCallResult {
            files: drawn,
            shown: vec![image.clone()],
            ..ToolCallResult::of(
                &ToolCall {
                    id: None,
                    function_name: "view_attachment".to_string(),
                    arguments: Value::Null,
                    visibility: Default::default(),
                    presentation: Default::default(),
                },
                Default::default(),
                String::new(),
                main.classification.clone(),
            )
        };
        let messages = || {
            vec![
                Message {
                    attachments: vec![image.clone(), text.clone()],
                    ..Message::new(main.classification.clone())
                },
                Message {
                    tool_results: vec![showing.clone()],
                    ..Message::new(main.classification.clone())
                },
            ]
        };
        let contents = |messages: &[Message]| {
            messages[0]
                .attachments
                .iter()
                .map(|a| a.content.as_deref().map(<[u8]>::to_vec))
                .collect::<Vec<_>>()
        };

        let (mut blind, mut seeing) = (messages(), messages());
        assert!(
            attachments.load(&mut blind, &ModelProfile::default()).await.unwrap().is_empty(),
            "nothing given to a model that does not see"
        );
        assert_eq!(contents(&blind), [None, None]);
        let given = attachments
            .load(&mut seeing, &ModelProfile { max_images: 4, ..ModelProfile::default() })
            .await
            .unwrap();
        assert_eq!(contents(&seeing), [Some(PNG.to_vec()), None], "the image alone");
        assert_eq!(
            seeing[1].tool_results[0].shown[0].content.as_deref(),
            Some(PNG),
            "and what a tool showed (issue 282, step 5)"
        );
        assert_eq!(
            seeing[1].tool_results[0].files[0].content.as_deref(),
            Some(PNG),
            "and what a tool gave (issue 512)"
        );
        assert_eq!(given.len(), 3, "what was given, by its classification: {given:?}");
        assert!(given.iter().all(|given| *given == image.classification));

        let file = world.files.path().join("content").join(&image.key);
        fs::write(&file, b"another image").unwrap();
        assert!(
            attachments
                .load(&mut messages(), &ModelProfile { max_images: 4, ..ModelProfile::default() })
                .await
                .is_ok(),
            "held: not read again"
        );
        attachments.purge(main.id).await.unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"another image").unwrap();
        let changed = attachments
            .load(&mut messages(), &ModelProfile { max_images: 4, ..ModelProfile::default() })
            .await;
        assert!(
            matches!(changed, Err(AttachmentError::Altered(id)) if id == image.id),
            "read again once let go of, and refused as not what the message says: {changed:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn what_a_tool_gave_is_remembered_for_its_caller_for_a_while_and_stays_kept() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let alice = ActorId::User(world.alice);
        let attachments = world.kernel.attachment_manager();
        let classification = main.classification.clone();

        let drawn = attachments
            .create_content(&hers, &main, classification.clone(), vec![png("a.png")])
            .await
            .unwrap()
            .remove(0);
        assert_eq!(attachments.kept_content(alice, main.id, drawn.id).as_ref(), Some(&drawn));
        assert_eq!(
            attachments.kept_content(alice, ChannelId::new(), drawn.id),
            None,
            "in no other channel"
        );
        let bob = ActorId::User(world.bob);
        assert_eq!(attachments.kept_content(bob, main.id, drawn.id), None, "for nobody else");

        time::advance(Duration::from_secs(3601)).await;
        assert_eq!(attachments.kept_content(alice, main.id, drawn.id), None, "only so long");
        attachments.create_content(&hers, &main, classification, vec![png("b.png")]).await.unwrap();
        assert_eq!(read(attachments.content_of(&drawn).await.unwrap()).await, PNG, "and it stays");
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_waits_for_its_message_only_so_long_and_goes_with_its_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&session);
        let hers = World::as_user(world.alice);
        let alice = ActorId::User(world.alice);
        let attachments = world.kernel.attachment_manager();

        let unclaimed = attachments.create(&hers, &main, "a.txt", None, bytes(b"a")).await.unwrap();
        time::advance(Duration::from_secs(3601)).await;
        assert!(
            matches!(
                attachments.named(alice, main.id, &[unclaimed.id], &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "waited too long"
        );
        assert!(!attachments.claim(alice, main.id, slice::from_ref(&unclaimed), &[]));
        let claimed = attachments.create(&hers, &main, "b.txt", None, bytes(b"b")).await.unwrap();
        assert!(
            matches!(attachments.content_of(&unclaimed).await, Err(AttachmentError::NotFound(_))),
            "removed as the next is uploaded"
        );
        assert!(attachments.claim(alice, main.id, slice::from_ref(&claimed), &[]));
        let waiting = attachments.create(&hers, &main, "c.txt", None, bytes(b"c")).await.unwrap();

        attachments.purge(main.id).await.unwrap();
        assert!(
            matches!(attachments.content_of(&claimed).await, Err(AttachmentError::NotFound(_))),
            "gone with its channel"
        );
        assert!(
            matches!(
                attachments.named(alice, main.id, &[waiting.id], &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "and what waited there"
        );
    }
}
