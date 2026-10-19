//! What every component that talks to a remote system over HTTP shares
//! besides its endpoint (`caelumora_model::remote`): how its errors are
//! described, without the URL a library writes into them (issue 188), and
//! how what it reads of its remote side is kept (`ExpiringCache`, issue
//! 126). Public for the component kinds outside the core, such as
//! `a2a_agent` (issue 326).

use arc_swap::ArcSwapOption;
use reqwest::Error;
use std::fmt::{self, Debug};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// The full chain of the causes of `error`, a failed request, without the
/// URL that `reqwest` writes into its messages.
pub fn describe_request_error(error: Error) -> String {
    describe_chain(&error.without_url())
}

/// The full chain of an error's causes as one line, e.g. "error sending
/// request: connection refused", for errors whose own `Display` stops at
/// the outermost layer.
pub fn describe_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        let text = e.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        current = e.source();
    }
    parts.join(": ")
}

/// A value read from a remote side and kept for a time (issue 126): the
/// tools of an MCP server, where an A2A agent is reached. It is read the
/// first time it is asked for, by whoever asks and in their name - a
/// component keeps no security context of its own (`ComponentContext`) -
/// and read again once it is older than its time-to-live, or once whoever
/// used it found it outdated (`invalidate`). A read that fails is not
/// kept: what was read before stays (`current`), and the next ask reads
/// again. One read at a time: whoever asks while another reads waits for
/// that read and takes what it read.
pub struct ExpiringCache<T> {
    ttl: Duration,
    kept: ArcSwapOption<Kept<T>>,
    reading: Mutex<()>,
}

/// A value read, and until when it is taken without reading it again;
/// `None` for ever, for a time-to-live beyond what an `Instant` can hold.
struct Kept<T> {
    value: Arc<T>,
    fresh_until: Option<Instant>,
}

impl<T> Kept<T> {
    fn fresh(&self) -> bool {
        self.fresh_until.is_none_or(|until| Instant::now() < until)
    }
}

impl<T> ExpiringCache<T> {
    /// A cache of nothing yet, whose values are kept for `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, kept: ArcSwapOption::empty(), reading: Mutex::new(()) }
    }

    /// The value as last read, however old; `None` if none was read yet.
    pub fn current(&self) -> Option<Arc<T>> {
        self.kept.load().as_ref().map(|kept| Arc::clone(&kept.value))
    }

    /// The value, read with `read` if none was read yet, or what was read
    /// is outdated.
    ///
    /// # Errors
    /// What `read` failed with; what was read before stays (`current`).
    pub async fn get<E, F, Fut>(&self, read: F) -> Result<Arc<T>, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if let Some(value) = self.fresh() {
            return Ok(value);
        }
        let _reading = self.reading.lock().await;
        // Read while this one waited.
        if let Some(value) = self.fresh() {
            return Ok(value);
        }
        let value = Arc::new(read().await?);
        let fresh_until = Instant::now().checked_add(self.ttl);
        self.kept.store(Some(Arc::new(Kept { value: Arc::clone(&value), fresh_until })));
        Ok(value)
    }

    /// Marks what was read as outdated: the next ask reads it again, and
    /// until then it is `current` still.
    pub fn invalidate(&self) {
        self.kept.rcu(|kept| kept.as_ref().map(|kept| Arc::new(Kept { value: Arc::clone(&kept.value), fresh_until: Some(Instant::now()) })));
    }

    fn fresh(&self) -> Option<Arc<T>> {
        self.kept.load().as_ref().filter(|kept| kept.fresh()).map(|kept| Arc::clone(&kept.value))
    }
}

impl<T> Debug for ExpiringCache<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kept = self.kept.load();
        f.debug_struct("ExpiringCache")
            .field("ttl", &self.ttl)
            .field("read", &kept.is_some())
            .field("fresh", &kept.as_ref().is_some_and(|kept| kept.fresh()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TTL: Duration = Duration::from_secs(60);

    /// A read that counts itself and answers with how many reads there
    /// were, or fails.
    async fn read(reads: &AtomicUsize, fails: bool) -> Result<usize, String> {
        let n = reads.fetch_add(1, Ordering::SeqCst) + 1;
        if fails { Err(format!("read {n} failed")) } else { Ok(n) }
    }

    #[tokio::test(start_paused = true)]
    async fn a_value_is_read_once_and_again_after_its_time_to_live() {
        let cache = ExpiringCache::new(TTL);
        let reads = AtomicUsize::new(0);
        assert_eq!(cache.current(), None, "nothing read yet");
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 1);
        tokio::time::advance(TTL - Duration::from_secs(1)).await;
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 1, "kept within its time-to-live");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 2, "read again after it");
        assert_eq!(cache.current().as_deref(), Some(&2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_value_found_outdated_is_read_again_at_the_next_ask() {
        let cache = ExpiringCache::new(TTL);
        let reads = AtomicUsize::new(0);
        cache.invalidate();
        assert_eq!(cache.current(), None, "nothing to mark");
        cache.get(|| read(&reads, false)).await.unwrap();
        cache.invalidate();
        assert_eq!(cache.current().as_deref(), Some(&1), "current until read again");
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 2);
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 2, "fresh again");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_read_is_not_kept_and_leaves_what_was_read_before() {
        let cache = ExpiringCache::new(TTL);
        let reads = AtomicUsize::new(0);
        assert_eq!(cache.get(|| read(&reads, true)).await.unwrap_err(), "read 1 failed");
        assert_eq!(cache.current(), None);
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 2, "read again at the next ask");
        tokio::time::advance(TTL).await;
        assert_eq!(cache.get(|| read(&reads, true)).await.unwrap_err(), "read 3 failed");
        assert_eq!(cache.current().as_deref(), Some(&2), "what was read before stays");
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 4, "and is still outdated");
    }

    #[tokio::test(start_paused = true)]
    async fn who_asks_while_another_reads_takes_what_that_one_read() {
        let cache = Arc::new(ExpiringCache::new(TTL));
        let reads = Arc::new(AtomicUsize::new(0));
        let slow = |cache: Arc<ExpiringCache<usize>>, reads: Arc<AtomicUsize>| async move {
            cache.get(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                read(&reads, false).await
            }).await
        };
        let (first, second) = tokio::join!(slow(Arc::clone(&cache), Arc::clone(&reads)), slow(Arc::clone(&cache), Arc::clone(&reads)));
        assert_eq!((*first.unwrap(), *second.unwrap()), (1, 1));
        assert_eq!(reads.load(Ordering::SeqCst), 1, "read once");
    }

    #[tokio::test(start_paused = true)]
    async fn a_time_to_live_beyond_any_instant_keeps_a_value_for_ever() {
        let cache = ExpiringCache::new(Duration::MAX);
        let reads = AtomicUsize::new(0);
        cache.get(|| read(&reads, false)).await.unwrap();
        tokio::time::advance(Duration::from_secs(3600 * 24 * 365)).await;
        assert_eq!(*cache.get(|| read(&reads, false)).await.unwrap(), 1);
    }
}
