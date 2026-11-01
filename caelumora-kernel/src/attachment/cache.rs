//! The content of attachments held in memory for the models given it along (issue 282): every
//! model call gives a model every image of its context again, which is read from a storage once.

use bytes::Bytes;
use caelumora_model::content::ContentRef;
use caelumora_model::digest::Digest;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// The content of attachments by their storage, key and digest - the digest too, so that a
/// reference to other bytes than those held is never answered with them -, held within a budget of
/// bytes for the process: an image several agents of a channel are given lies in memory once. What
/// was used least recently leaves first - a question of memory, not of meaning: what left is read
/// from its storage again when it is next given.
#[derive(Debug)]
pub(super) struct ContentCache {
    budget: u64,
    held: Mutex<Held>,
}

#[derive(Debug, Default)]
struct Held {
    /// By storage, key and digest: the content, and when it was last used.
    entries: HashMap<(String, String, Digest), (Bytes, u64)>,
    /// The bytes of all of them.
    size: u64,
    /// A counter of uses, the order of their recency.
    uses: u64,
}

impl ContentCache {
    /// A cache holding at most `budget` bytes.
    pub(super) fn new(budget: u64) -> Self {
        Self { budget, held: Mutex::new(Held::default()) }
    }

    /// The content `attachment` refers to, if held - used now.
    pub(super) fn get(&self, attachment: &ContentRef) -> Option<Bytes> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.uses += 1;
        let now = held.uses;
        let (content, used) = held.entries.get_mut(&at(attachment))?;
        *used = now;
        Some(content.clone())
    }

    /// Holds `content`, what `attachment` refers to, letting go of what was used least recently
    /// until it fits; content larger than the whole budget is not held.
    pub(super) fn put(&self, attachment: &ContentRef, content: Bytes) {
        let size = content.len() as u64;
        if size > self.budget {
            return;
        }
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.uses += 1;
        let now = held.uses;
        if let Some((replaced, _)) = held.entries.insert(at(attachment), (content, now)) {
            held.size -= replaced.len() as u64;
        }
        held.size += size;
        while held.size > self.budget {
            let Some(oldest) =
                held.entries.iter().min_by_key(|(_, (_, used))| *used).map(|(at, _)| at.clone())
            else {
                break;
            };
            if let Some((left, _)) = held.entries.remove(&oldest) {
                held.size -= left.len() as u64;
            }
        }
    }

    /// Lets go of everything held under keys beginning with `prefix`, in any storage.
    pub(super) fn forget_under(&self, prefix: &str) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let mut freed = 0;
        held.entries.retain(|(_, key, _), (content, _)| {
            let keep = !key.starts_with(prefix);
            if !keep {
                freed += content.len() as u64;
            }
            keep
        });
        held.size -= freed;
    }
}

/// Where `attachment`'s content is held.
fn at(attachment: &ContentRef) -> (String, String, Digest) {
    (attachment.storage.clone(), attachment.key.clone(), attachment.digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::content::testing::make_content_ref;
    use caelumora_model::digest;

    /// A reference to `bytes` under `key` in the storage "kept".
    fn at_key(key: &str, bytes: &[u8]) -> ContentRef {
        ContentRef { key: key.to_string(), ..make_content_ref("a", bytes) }
    }

    #[test]
    fn what_was_used_least_recently_leaves_first_to_keep_within_the_budget() {
        let cache = ContentCache::new(10);
        let first = at_key("attachments/a/1", b"aaaa");
        let second = at_key("attachments/a/2", b"bbbb");
        let third = at_key("attachments/b/3", b"cccc");
        cache.put(&first, Bytes::from_static(b"aaaa"));
        cache.put(&second, Bytes::from_static(b"bbbb"));
        assert!(cache.get(&first).is_some(), "used again");
        cache.put(&third, Bytes::from_static(b"cccc"));
        assert!(cache.get(&second).is_none(), "the least recently used left");
        assert_eq!(cache.get(&first).as_deref(), Some(&b"aaaa"[..]));
        assert!(
            cache.get(&ContentRef { storage: "other".to_string(), ..first.clone() }).is_none(),
            "by storage"
        );
        assert!(
            cache.get(&ContentRef { digest: digest::content(b"other"), ..first.clone() }).is_none(),
            "and by digest: never other bytes than referred to"
        );

        let large = at_key("attachments/b/4", b"far more than ten bytes");
        cache.put(&large, Bytes::from_static(b"far more than ten bytes"));
        assert!(cache.get(&large).is_none(), "larger than the budget: not held");
        cache.forget_under("attachments/a/");
        assert!(cache.get(&first).is_none(), "a channel's purged");
        assert!(cache.get(&third).is_some(), "another's kept");
    }
}
