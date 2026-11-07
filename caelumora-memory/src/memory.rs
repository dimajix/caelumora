//! What an agent remembers across its channels and sessions (issues 62, 518): its entries, the
//! changes a turn stages of them, and the document that keeps them, one per agent.
//!
//! Like notes, memory is a ledger (`caelumora_domain::ledger`, issue 92): something is remembered
//! (added), updated, forgotten (removed) or reclassified. Unlike notes, it belongs to the agent
//! itself, not to one of its channels: it does not branch with a channel's history (principle 7 in
//! issue 19), and every change records the channel it was made in and the privacy of that
//! channel's session then. An entry carries the label of the turn that wrote it, so that it is
//! recalled only where that may be read (the flow rule of issue 133), and its session's privacy as
//! it was, so that it is recalled in another session only where that session's people agreed to it
//! (issue 134).

use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::channel::ChannelId;
use caelumora_domain::define_id;
use caelumora_domain::ledger::{Change, Item, Ledger, LedgerKind};
use caelumora_domain::privacy::SessionPrivacy;
use caelumora_domain::query::NoField;
use caelumora_domain::removal::Removal;
use caelumora_domain::security::{Secured, SecurityAttributes};
use caelumora_domain::store::{EntityKind, NoEntity, Revision, Stored};
use caelumora_domain::turn::StagedChange;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Not;

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
    pub channel: ChannelId,
    /// The channel's session, which it names (`SessionPrivacy::id`).
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

/// A change to an agent's memory, as a turn stages it (issue 518): a change of its ledger, without
/// where it is made - the plugin derives that from the place of the work as it commits it. A type
/// of the crate's own, so that the crate says what it is staged as (`StagedChange`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryChange(pub Change<MemoryId>);

/// Something an agent remembers now.
pub type Memory = Item<Memories>;
/// An agent's memory, as a chain of changes.
pub type MemoryChain = Ledger<Memories>;

impl StagedChange for MemoryChange {
    const KIND: &'static str = "memory";
}

define_id!(AgentMemoryId);

impl AgentMemoryId {
    /// The id of the memory of `agent`: the agent's own, so that each agent has one.
    pub fn of(agent: AgentId) -> Self {
        Self(agent.0)
    }
}

/// The memory of one agent, as it is kept (issue 518): the chain of every change to it, owned by
/// the agent's owner, who alone - and the agent, acting for them - reads and changes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    /// Its agent's id (`AgentMemoryId::of`).
    pub id: AgentMemoryId,
    /// Every change to it, on one branch.
    pub chain: MemoryChain,
    /// Its owner - the agent's -, and no access list: nobody else reads it.
    pub security_attributes: SecurityAttributes,
    /// Set when it was deleted (issue 164), which only the purge of its agent does, on the way to
    /// removing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<Removal>,
    /// Its revision (issue 492), counted up by every write of it.
    #[serde(default)]
    pub revision: Revision,
    /// Whether its purge has begun (issue 492): set by the store alone.
    #[serde(default, skip_serializing_if = "Not::not")]
    pub purging: bool,
}

impl AgentMemory {
    /// The memory of `agent`, owned by `owner`, that remembers nothing yet.
    pub fn new(agent: AgentId, owner: ActorId) -> Self {
        Self {
            id: AgentMemoryId::of(agent),
            chain: MemoryChain::default(),
            security_attributes: SecurityAttributes::owned_by(owner),
            deleted: None,
            revision: Revision::default(),
            purging: false,
        }
    }
}

impl Secured for AgentMemory {
    fn security_attributes(&self) -> &SecurityAttributes {
        &self.security_attributes
    }
}

impl Stored for AgentMemory {
    const KIND: EntityKind = EntityKind::new("memory");
    type Id = AgentMemoryId;
    type Within = NoEntity;
    type Field = NoField;

    fn id(&self) -> AgentMemoryId {
        self.id
    }

    fn deleted(&self) -> Option<&Removal> {
        self.deleted.as_ref()
    }

    fn set_deleted(&mut self, deleted: Option<Removal>) {
        self.deleted = deleted;
    }

    fn revision(&self) -> Revision {
        self.revision
    }

    fn set_revision(&mut self, revision: Revision) {
        self.revision = revision;
    }

    fn purging(&self) -> bool {
        self.purging
    }

    fn set_purging(&mut self) {
        self.purging = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_domain::confidentiality::Classification;
    use caelumora_domain::message::MessageId;

    #[test]
    fn every_entry_knows_where_it_was_written() {
        let mut chain = MemoryChain::default();
        let origin =
            MemoryOrigin { channel: ChannelId::default(), session: SessionPrivacy::default() };
        let remember = |id, text: &str| Change::Add {
            id: MemoryId(id),
            text: text.to_string(),
            classification: Classification::CHANNEL_DEFAULT,
        };
        chain.push(remember(1, "Kaya likes tea."), origin.clone(), MessageId::new());
        chain.push(remember(2, "Kaya lives in Berlin."), origin.clone(), MessageId::new());
        chain.push(Change::Remove { id: MemoryId(1) }, origin.clone(), MessageId::new());

        let current = chain.current();
        assert_eq!(
            current.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            ["Kaya lives in Berlin."]
        );
        assert_eq!(current[0].origin, origin);
        assert_eq!(chain.next_id(), MemoryId(3));
        assert!(chain.has_given(MemoryId(1)), "a number removed stays given");
    }

    #[test]
    fn the_digest_covers_where_a_change_was_made() {
        let after = MessageId::new();
        let remember = Change::Add {
            id: MemoryId(1),
            text: "a".to_string(),
            classification: Classification::CHANNEL_DEFAULT,
        };
        let digest_in = |origin: MemoryOrigin| {
            let mut chain = MemoryChain::default();
            chain.push(remember.clone(), origin, after);
            chain.digest()
        };
        let here = MemoryOrigin::default();
        let elsewhere = MemoryOrigin { channel: ChannelId::new(), ..MemoryOrigin::default() };
        assert_ne!(digest_in(here.clone()), digest_in(elsewhere));
        assert_eq!(digest_in(here.clone()), digest_in(here));
    }

    #[test]
    fn an_agents_memory_is_kept_whole_under_its_agents_id() {
        let agent = AgentId::new();
        let mut memory = AgentMemory::new(agent, ActorId::Agent(agent));
        let remember = Change::Add {
            id: MemoryId(1),
            text: "kept".to_string(),
            classification: Classification::CHANNEL_DEFAULT,
        };
        memory.chain.push(remember, MemoryOrigin::default(), MessageId::new());
        let stored: AgentMemory =
            serde_json::from_value(serde_json::to_value(&memory).unwrap()).unwrap();
        assert_eq!(stored.id, AgentMemoryId::of(agent));
        assert_eq!(stored.chain.current()[0].text, "kept");
        assert_eq!(stored.chain.digest(), memory.chain.digest());
    }
}
