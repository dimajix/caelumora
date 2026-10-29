//! The `AttachmentManager`: what is uploaded or copied for a message - checked, kept and waiting
//! for the message that claims it -, what a message may carry as its channel admits it, and the
//! content of an attachment for a download or for a model given it along.

use super::cache::ContentCache;
use super::{AttachmentError, AttachmentResult};
use crate::backend::NamedBackends;
use crate::component::{ComponentError, ComponentResult};
use crate::content::{keep_digested, ContentError, ContentStorage};
use crate::terminal::ByteStream;
use crate::view::ChannelView;
use bytes::{Bytes, BytesMut};
use caelumora_model::actor::ActorId;
use caelumora_model::confidentiality::{
    Classification, ConfidentialityPolicy, ConfidentialityQuestions,
};
use caelumora_model::config::AttachmentsConfig;
use caelumora_model::content::{self, ContentId, ContentRef};
use caelumora_model::context::ModelProfile;
use caelumora_model::digest;
use caelumora_model::message::Message;
use caelumora_model::session::ChannelId;
use caelumora_model::value::text::FileName;
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

/// Who keeps content for a message in `channel`: its viewer, who takes part in it.
///
/// # Errors
/// `NotAParticipant` for a viewer who does not.
fn uploader(channel: &ChannelView) -> AttachmentResult<ActorId> {
    let by = channel.views().context().caller_id;
    channel.lists(by).then_some(by).ok_or(AttachmentError::NotAParticipant(by))
}

/// A new id for content of channel `channel`, and the key its bytes lie under.
fn new_key(channel: ChannelId) -> (ContentId, String) {
    let id = ContentId::new();
    (id, format!("{}/{id}", channel_key(channel)))
}

/// An upload no message claimed yet: who uploaded it for which channel, and when.
#[derive(Debug)]
struct Upload {
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
    uploads: Mutex<HashMap<ContentId, Upload>>,
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
            cache: ContentCache::new(config.cache_bytes),
        })
    }

    /// Keeps `content` - its size and its bytes - as the file `name`, uploaded by the viewer of
    /// `channel` for a message they will say there (issue 282): classified `classification` - the
    /// channel's if `None` -, which may flow into the channel and into the storage it is kept
    /// in, by a viewer taking part in the channel. What it is, is told by its first bytes, never
    /// by the name. It waits for the message that claims it (`named`, `claim`) as long as
    /// configured, and is removed once it waited longer, as the next upload is kept.
    ///
    /// # Errors
    /// `NotConfigured`, `NotAParticipant`, `Invalid` for a name that is none,
    /// `ClassificationRefused`, `StorageNotCleared`, `Content` for content too large, other than
    /// said, or a storage that failed.
    pub async fn create(
        &self,
        channel: &ChannelView,
        name: &str,
        classification: Option<Classification>,
        (size, content): (u64, ByteStream),
    ) -> AttachmentResult<ContentRef> {
        let context = channel.views().context();
        let by = uploader(channel)?;
        let name = FileName::new(name)?;
        let classification = classification.unwrap_or_else(|| channel.classification().clone());
        self.confidentiality
            .require_classifiable(&classification, || AttachmentError::ClassificationRefused)?;
        self.confidentiality.require_publish(
            context,
            &classification,
            channel.classification(),
            || AttachmentError::ClassificationRefused,
        )?;
        let (storage_name, storage) =
            self.keep_in.as_ref().ok_or(AttachmentError::NotConfigured)?;
        self.confidentiality.require_storable(
            context,
            &classification,
            storage.destination(),
            || AttachmentError::StorageNotCleared(storage_name.to_string()),
        )?;
        self.forget_expired().await;

        let (id, key) = new_key(channel.id());
        let (head, content) = head_of(content).await;
        let digest = keep_digested(storage.as_ref(), &key, size, content, self.max_bytes).await?;
        let content = ContentRef {
            id,
            name,
            media_type: content::media_type(&head).to_string(),
            size,
            digest,
            classification,
            storage: storage_name.to_string(),
            key,
            number: None,
            content: None,
        };
        self.wait(by, channel.id(), content.clone());
        debug!(attachment = %id, channel = %channel.id(), by = %by, size, "Attachment uploaded");
        Ok(content)
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

    /// A copy of `attachment` - one of the channel's, as the viewer of `channel` read it there -
    /// for a message the viewer will say there (issue 282): the same content, label and name under
    /// an id and a key of its own in the same storage, waiting for the message that claims it as an
    /// upload does. What an agent shows of what it heard, so that each message holds its own.
    ///
    /// # Errors
    /// `NotAParticipant`; `NotFound` for content that is none of the channel's, or that its storage
    /// has nothing of; `StorageGone`, `Content`.
    pub async fn copy(
        &self,
        channel: &ChannelView,
        attachment: &ContentRef,
    ) -> AttachmentResult<ContentRef> {
        let by = uploader(channel)?;
        if !self.lies_in(channel.id(), attachment) {
            return Err(AttachmentError::NotFound(attachment.id));
        }
        self.forget_expired().await;
        let (id, key) = new_key(channel.id());
        self.storages.kept_in(&attachment.storage)?.copy(&attachment.key, &key).await.map_err(
            |e| match e {
                ContentError::NotFound(_) => AttachmentError::NotFound(attachment.id),
                e => e.into(),
            },
        )?;
        let copy = ContentRef { id, key, number: None, content: None, ..attachment.clone() };
        self.wait(by, channel.id(), copy.clone());
        debug!(attachment = %id, of = %attachment.id, channel = %channel.id(), by = %by, "Attachment copied");
        Ok(copy)
    }

    /// Lets `content`, kept for a message `by` will say in `channel`, wait for that message.
    fn wait(&self, by: ActorId, channel: ChannelId, content: ContentRef) {
        let upload = Upload { by, channel, at: Instant::now(), content };
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(upload.content.id, upload);
    }

    /// Puts their content on the attachments of `messages`, and on what their tool results showed,
    /// that the model of `profile` is given along (issue 282) - held in memory, else read from its
    /// storage and checked against its digest -, and answers how each is classified: what the
    /// model is given of them. Asking nothing: what
    /// lies in an agent's transcript, it heard (`ActorRuntime::load_attachments`).
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
    /// uploads that wait for a message there.
    ///
    /// # Errors
    /// `Content` if a storage cannot remove them.
    pub(crate) async fn purge(&self, channel: ChannelId) -> AttachmentResult<()> {
        let prefix = channel_key(channel);
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, upload| upload.channel != channel);
        self.cache.forget_under(&format!("{prefix}/"));
        for (_, storage) in self.storages.iter() {
            storage.remove_under(&prefix).await?;
        }
        Ok(())
    }

    /// Removes the uploads that waited longer than configured for their message, and their content.
    /// Logged where that fails: it lies in its channel's key space until the channel is purged.
    async fn forget_expired(&self) {
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

impl Upload {
    /// Whether it waits for a message `by` says in `channel`: theirs, for that channel, and not
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
    use crate::view::Viewer;
    use bytes::Bytes;
    use caelumora_model::confidentiality::{Compartment, Confidentiality};
    use caelumora_model::digest;
    use caelumora_model::security::SecurityContext;
    use std::fs;
    use std::path::{Path, PathBuf};
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

    /// The channel "main" of `session`, as `viewer` reaches it.
    async fn main_of(world: &World, session: &SessionLoop, viewer: SecurityContext) -> ChannelView {
        let views = world.manager.views(Viewer::new(viewer));
        views
            .session(session.id())
            .await
            .unwrap()
            .channel(session.channels()[0].id())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_upload_is_kept_as_its_bytes_say_and_claimed_by_one_message_of_its_uploader() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&world, &session, World::as_user(world.alice)).await;
        let (alice, bob) = (ActorId::User(world.alice), ActorId::User(world.bob));
        let attachments = world.runtime.attachment_manager();

        let uploaded =
            attachments.create(&main, "chart.txt", None, bytes(PNG)).await.expect("hers to upload");
        assert_eq!(
            (uploaded.name.as_str(), uploaded.media_type.as_str(), uploaded.size),
            ("chart.txt", "image/png", PNG.len() as u64),
            "what it is, as its bytes say"
        );
        assert_eq!(uploaded.digest, digest::content(PNG));
        assert_eq!(uploaded.classification, *main.classification(), "the channel's, unless given");
        assert_eq!(
            (uploaded.storage.as_str(), uploaded.key.clone()),
            ("kept", format!("attachments/{}/{}", main.id(), uploaded.id))
        );
        assert_eq!(read(attachments.content_of(&uploaded).await.unwrap()).await, PNG);

        let ids = [uploaded.id];
        assert_eq!(
            attachments.named(alice, main.id(), &ids, &[]).unwrap(),
            slice::from_ref(&uploaded),
            "hers, for the channel"
        );
        assert!(
            matches!(
                attachments.named(bob, main.id(), &ids, &[]),
                Err(AttachmentError::NotFound(_))
            ),
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
            !attachments.claim(bob, main.id(), slice::from_ref(&uploaded), &[]),
            "not his to claim"
        );
        assert!(
            !attachments.claim(alice, ChannelId::new(), slice::from_ref(&uploaded), &[]),
            "not for another channel"
        );
        assert!(
            !attachments.claim(alice, main.id(), &[uploaded.clone(), uploaded.clone()], &[]),
            "none twice"
        );
        let elsewhere = ContentRef {
            key: format!("attachments/{}/{}", ChannelId::new(), uploaded.id),
            ..uploaded.clone()
        };
        assert!(!attachments.claim(alice, main.id(), &[elsewhere], &[]), "only as it was uploaded");

        assert!(
            attachments.claim(alice, main.id(), slice::from_ref(&uploaded), &[]),
            "nothing claimed by what was refused"
        );
        assert!(
            !attachments.claim(alice, main.id(), slice::from_ref(&uploaded), &[]),
            "claimed once"
        );
        assert!(
            attachments.claim(
                alice,
                main.id(),
                slice::from_ref(&uploaded),
                slice::from_ref(&uploaded)
            ),
            "kept by the message that has it"
        );
        assert!(matches!(
            attachments.named(alice, main.id(), &ids, &[]),
            Err(AttachmentError::NotFound(_))
        ));
        assert_eq!(read(attachments.content_of(&uploaded).await.unwrap()).await, PNG, "and kept");

        let copy = attachments.copy(&main, &uploaded).await.expect("a copy for her next message");
        assert!(
            copy.id != uploaded.id
                && copy.key != uploaded.key
                && (copy.digest, &copy.classification)
                    == (uploaded.digest, &uploaded.classification),
            "its own id and key, the same content and label"
        );
        assert_eq!(read(attachments.content_of(&copy).await.unwrap()).await, PNG);
        assert!(
            attachments.lies_in(main.id(), &copy) && !attachments.lies_in(ChannelId::new(), &copy),
            "in the channel's key space alone"
        );
        assert!(
            attachments.claim(alice, main.id(), slice::from_ref(&copy), &[]),
            "waiting for her message as an upload does"
        );
    }

    #[tokio::test]
    async fn an_upload_is_refused_unless_it_may_enter_the_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&world, &session, World::as_user(world.alice)).await;
        let attachments = world.runtime.attachment_manager();

        let outsider = main_of(&world, &session, SecurityContext::system()).await;
        assert!(
            matches!(
                attachments.create(&outsider, "a.txt", None, bytes(b"a")).await,
                Err(AttachmentError::NotAParticipant(ActorId::System))
            ),
            "who takes no part in it"
        );
        let above = Classification::general(Confidentiality::Confidential);
        assert!(
            matches!(
                attachments.create(&main, "a.txt", Some(above), bytes(b"a")).await,
                Err(AttachmentError::ClassificationRefused)
            ),
            "above the channel"
        );
        let unknown =
            Classification::new(Confidentiality::Internal, [Compartment::new("hr")]).unwrap();
        assert!(
            matches!(
                attachments.create(&main, "a.txt", Some(unknown), bytes(b"a")).await,
                Err(AttachmentError::ClassificationRefused)
            ),
            "a compartment not configured"
        );
        assert!(
            matches!(
                attachments.create(&main, "../a.txt", None, bytes(b"a")).await,
                Err(AttachmentError::Invalid(_))
            ),
            "a path for a name"
        );
        let (_, content) = bytes(b"a");
        let too_large = AttachmentsConfig::default().max_bytes + 1;
        assert!(matches!(
            attachments.create(&main, "a.txt", None, (too_large, content)).await,
            Err(AttachmentError::Content(ContentError::TooLarge { .. }))
        ));
        let (_, content) = bytes(b"abc");
        assert!(
            matches!(
                attachments.create(&main, "a.txt", None, (2, content)).await,
                Err(AttachmentError::Content(ContentError::Changed))
            ),
            "other than it was said to be"
        );
        let kept = world.files.path().join("content").join("attachments");
        assert!(!kept.exists() || walk(&kept).is_empty(), "nothing kept of any of them");
        assert!(
            attachments
                .create(&main, "a.txt", Some(Classification::PUBLIC), bytes(b"a"))
                .await
                .is_ok(),
            "released, below the channel"
        );
    }

    #[tokio::test]
    async fn what_a_model_is_given_along_is_loaded_checked_and_held() {
        use caelumora_model::context::ModelProfile;
        use caelumora_model::message::Message;
        use caelumora_model::tool::{ToolCall, ToolCallResult};
        use serde_json::Value;
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&world, &session, World::as_user(world.alice)).await;
        let attachments = world.runtime.attachment_manager();
        let image = attachments.create(&main, "chart.png", None, bytes(PNG)).await.unwrap();
        let text = attachments.create(&main, "notes.txt", None, bytes(b"notes")).await.unwrap();
        let showing = ToolCallResult {
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
                main.classification().clone(),
            )
        };
        let messages = || {
            vec![
                Message {
                    attachments: vec![image.clone(), text.clone()],
                    ..Message::new(main.classification().clone())
                },
                Message {
                    tool_results: vec![showing.clone()],
                    ..Message::new(main.classification().clone())
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
            .load(&mut seeing, &ModelProfile { images: true, ..ModelProfile::default() })
            .await
            .unwrap();
        assert_eq!(contents(&seeing), [Some(PNG.to_vec()), None], "the image alone");
        assert_eq!(
            seeing[1].tool_results[0].shown[0].content.as_deref(),
            Some(PNG),
            "and what a tool showed (issue 282, step 5)"
        );
        assert_eq!(
            given,
            [image.classification.clone(), image.classification.clone()],
            "what was given, by its classification"
        );

        let file = world.files.path().join("content").join(&image.key);
        fs::write(&file, b"another image").unwrap();
        assert!(
            attachments
                .load(&mut messages(), &ModelProfile { images: true, ..ModelProfile::default() })
                .await
                .is_ok(),
            "held: not read again"
        );
        attachments.purge(main.id()).await.unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"another image").unwrap();
        let changed = attachments
            .load(&mut messages(), &ModelProfile { images: true, ..ModelProfile::default() })
            .await;
        assert!(
            matches!(changed, Err(AttachmentError::Altered(id)) if id == image.id),
            "read again once let go of, and refused as not what the message says: {changed:?}"
        );
    }

    /// The files below `dir`.
    fn walk(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .flat_map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn an_upload_waits_for_its_message_only_so_long_and_goes_with_its_channel() {
        let world = World::new().await;
        let session = world.shared_session(&[], &[]).await;
        let main = main_of(&world, &session, World::as_user(world.alice)).await;
        let alice = ActorId::User(world.alice);
        let attachments = world.runtime.attachment_manager();

        let unclaimed = attachments.create(&main, "a.txt", None, bytes(b"a")).await.unwrap();
        time::advance(Duration::from_secs(3601)).await;
        assert!(
            matches!(
                attachments.named(alice, main.id(), &[unclaimed.id], &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "waited too long"
        );
        assert!(!attachments.claim(alice, main.id(), slice::from_ref(&unclaimed), &[]));
        let claimed = attachments.create(&main, "b.txt", None, bytes(b"b")).await.unwrap();
        assert!(
            matches!(attachments.content_of(&unclaimed).await, Err(AttachmentError::NotFound(_))),
            "removed as the next is uploaded"
        );
        assert!(attachments.claim(alice, main.id(), slice::from_ref(&claimed), &[]));
        let waiting = attachments.create(&main, "c.txt", None, bytes(b"c")).await.unwrap();

        attachments.purge(main.id()).await.unwrap();
        assert!(
            matches!(attachments.content_of(&claimed).await, Err(AttachmentError::NotFound(_))),
            "gone with its channel"
        );
        assert!(
            matches!(
                attachments.named(alice, main.id(), &[waiting.id], &[]),
                Err(AttachmentError::NotFound(_))
            ),
            "and what waited there"
        );
    }
}
