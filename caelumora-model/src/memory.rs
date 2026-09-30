//! Agent memory (issue 62, step 3): what an agent remembers across its
//! channels and sessions, kept in its `AgentState`.
//!
//! Like notes (`crate::note`), memory is a ledger (`crate::ledger`, issue
//! 92): something is remembered (added), forgotten (removed) or
//! reclassified. Unlike notes, it belongs to the agent itself, not to one of
//! its channels: it does not branch with a channel's history (principle 7 in
//! issue 19), and every change records the channel it was made in and the
//! privacy of that channel's session then. An entry carries the
//! classification of the channel it was written in, so that it is recalled
//! only where that channel's contents may be read (the flow rule of issue
//! 133, `Classification::flows_to`), and its session's privacy as it was,
//! so that it is recalled in another session only where that session's
//! people agreed to it (issue 134).

use crate::confidentiality::ChannelRef;
use crate::ledger::{Change, Item, Ledger, LedgerKind};
use crate::privacy::SessionPrivacy;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The number of an entry of an agent's memory: 1 for the first, counting
/// on, never given twice. Small, because a model has to repeat it to forget
/// the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub u32);

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for MemoryId {
    fn from(n: u32) -> Self {
        MemoryId(n)
    }
}

impl From<MemoryId> for u32 {
    fn from(id: MemoryId) -> Self {
        id.0
    }
}

/// Where a change to memory was made: the channel, and its session as
/// privacy saw it then (issue 134) - what the session's people agreed to
/// when it was said holds, whatever the session's policy becomes. Part of
/// the digest: a reader learns where what it recalls comes from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryOrigin {
    pub channel: ChannelRef,
    pub session: SessionPrivacy,
}

/// The kind of ledger an agent's memory is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Memories {}

impl LedgerKind for Memories {
    type Id = MemoryId;
    type Origin = MemoryOrigin;
    const SCHEME: &'static str = "caelumora.memory-chain.v2";
    const ORIGIN_DIGESTED: bool = true;
}

/// A change to an agent's memory.
pub type MemoryChange = Change<MemoryId>;
/// Something an agent remembers now.
pub type Memory = Item<Memories>;
/// An agent's memory, as a chain of changes.
pub type MemoryChain = Ledger<Memories>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidentiality::Classification;
    use crate::message::MessageId;

    #[test]
    fn every_entry_knows_where_it_was_written() {
        let mut chain = MemoryChain::default();
        let origin = MemoryOrigin { channel: ChannelRef::default(), session: SessionPrivacy::default() };
        let remember = |id, text: &str| Change::Add { id: MemoryId(id), text: text.to_string(), classification: Classification::CHANNEL_DEFAULT };
        chain.push(remember(1, "Kaya likes tea."), origin.clone(), MessageId::new());
        chain.push(remember(2, "Kaya lives in Berlin."), origin.clone(), MessageId::new());
        chain.push(Change::Remove { id: MemoryId(1) }, origin.clone(), MessageId::new());

        let current = chain.current();
        assert_eq!(current.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Kaya lives in Berlin."]);
        assert_eq!(current[0].origin, origin);
        assert_eq!(chain.next_id(), MemoryId(3));
    }

    #[test]
    fn the_digest_covers_where_a_change_was_made() {
        let after = MessageId::new();
        let remember = Change::Add { id: MemoryId(1), text: "a".to_string(), classification: Classification::CHANNEL_DEFAULT };
        let digest_in = |origin: MemoryOrigin| {
            let mut chain = MemoryChain::default();
            chain.push(remember.clone(), origin, after);
            chain.digest()
        };
        let here = MemoryOrigin::default();
        let elsewhere = MemoryOrigin { channel: ChannelRef { channel_id: crate::channel::ChannelId::new(), ..ChannelRef::default() }, ..MemoryOrigin::default() };
        assert_ne!(digest_in(here.clone()), digest_in(elsewhere));
        assert_eq!(digest_in(here.clone()), digest_in(here));
    }
}
