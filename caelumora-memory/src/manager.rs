//! `MemoryManager`: the one keeper of agents' memory (issue 518) - it reads an agent's memory,
//! hands out the numbers of its new entries, applies the changes of a turn to it one commit after
//! another, and removes it with its agent.
//!
//! An agent takes part in several channels, and agents of user or global scope in several
//! sessions, each with an actor of its own, whose turns commit to the one memory of the agent. The
//! manager keeps, per agent it serves, the number the next entry gets and a lock under which one
//! commit after another is checked and applied to the memory as it is stored then, so that no turn
//! overwrites another's changes. The number is handed out when a turn remembers, not when it
//! completes: several turns may remember at the same time, and each needs its number at once to
//! tell the model. It begins from the memory as stored, read in the kernel's name - a number tells
//! nothing of what is remembered -, and is raised to what a commit finds stored, so that it never
//! falls behind what another process wrote.

use crate::memory::{AgentMemory, AgentMemoryId, MemoryChange, MemoryId, MemoryOrigin};
use async_trait::async_trait;
use caelumora_domain::agent::{Agent, AgentId};
use caelumora_domain::message::MessageId;
use caelumora_domain::store::{StoreError, StoreResult, StoreResultExt};
use caelumora_kernel::agent::AgentMember;
use caelumora_kernel::scoped::purge_for_good;
use caelumora_security::SecurityContext;
use caelumora_store::EntityStore;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};
use tracing::debug;

/// The one keeper of agents' memory.
pub(crate) struct MemoryManager {
    memories: Arc<dyn EntityStore<AgentMemory>>,
    /// What is kept of each agent served since the process started: its numbers and its lock.
    agents: Mutex<HashMap<AgentId, Arc<Kept>>>,
}

/// What the manager keeps of one agent's memory.
#[derive(Default)]
struct Kept {
    /// The number the next entry gets, once begun.
    next_id: AtomicU32,
    /// Set once the number was begun from the memory as stored.
    begun: OnceCell<()>,
    /// Held while a commit is checked, applied and stored.
    write: Mutex<()>,
}

impl fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryManager").finish_non_exhaustive()
    }
}

impl MemoryManager {
    /// The keeper of the memories `memories` holds.
    pub(crate) fn new(memories: Arc<dyn EntityStore<AgentMemory>>) -> Self {
        Self { memories, agents: Mutex::new(HashMap::new()) }
    }

    /// The memory of `agent` as it is stored, read in `context`; while it remembers nothing, an
    /// empty one owned by the principal `context` answers to - the agent's owner -, as it is made
    /// once it remembers something (`apply`).
    ///
    /// # Errors
    /// Whatever the store reports, but that there is none.
    pub(crate) async fn memory_of(
        &self,
        context: &SecurityContext,
        agent: AgentId,
    ) -> StoreResult<AgentMemory> {
        let stored = self.memories.get(context, AgentMemoryId::of(agent)).await.optional()?;
        Ok(stored.unwrap_or_else(|| AgentMemory::new(agent, context.principal_id)))
    }

    /// A number for a new entry of `agent`, never handed out before.
    ///
    /// # Errors
    /// Whatever the store reports as the memory is read to begin the numbers.
    pub(crate) async fn reserve_id(&self, agent: AgentId) -> StoreResult<MemoryId> {
        let kept = self.kept(agent).await?;
        Ok(MemoryId(kept.next_id.fetch_add(1, Ordering::SeqCst)))
    }

    /// Whether `id` was handed out for a new entry of `agent` (`reserve_id`).
    ///
    /// # Errors
    /// As `reserve_id`.
    pub(crate) async fn is_reserved(&self, agent: AgentId, id: MemoryId) -> StoreResult<bool> {
        let kept = self.kept(agent).await?;
        Ok(id.0 < kept.next_id.load(Ordering::SeqCst))
    }

    /// Has `check` judge the memory of `agent` as it is stored now, read in `context` under the
    /// agent's lock, and applies the changes it answers, made at `origin` in a turn that ended with
    /// message `after`, and stores them, in `context`: one commit after another. A memory not
    /// stored yet is made (`memory_of`). Nothing is changed if `check` refuses or storing fails.
    ///
    /// # Errors
    /// What `check` answers, or whatever the store reports - a refusal, or a revision another
    /// process wrote meanwhile.
    pub(crate) async fn apply<E: From<StoreError>>(
        &self,
        context: &SecurityContext,
        agent: AgentId,
        origin: MemoryOrigin,
        after: MessageId,
        check: impl AsyncFnOnce(&AgentMemory) -> Result<Vec<MemoryChange>, E>,
    ) -> Result<(), E> {
        let kept = self.kept(agent).await?;
        let _writing = kept.write.lock().await;
        let mut memory = self.memory_of(context, agent).await?;
        kept.next_id.fetch_max(memory.chain.next_id().0, Ordering::SeqCst);
        let changes = check(&memory).await?;
        if changes.is_empty() {
            return Ok(());
        }
        // What was never stored carries the revision before the first.
        let made = memory.revision == Default::default();
        let count = changes.len();
        for MemoryChange(change) in changes {
            memory.chain.push(change, origin.clone(), after);
        }
        if made {
            self.memories.create(context, &mut memory).await?;
        } else {
            self.memories.update(context, &mut memory).await?;
        }
        debug!("Applied {} changes to the memory of agent {}", count, agent);
        Ok(())
    }

    /// What is kept of `agent`, its numbers begun from its memory as stored, read in the kernel's
    /// name once.
    async fn kept(&self, agent: AgentId) -> StoreResult<Arc<Kept>> {
        let kept = Arc::clone(self.agents.lock().await.entry(agent).or_default());
        kept.begun
            .get_or_try_init(async || {
                let system = SecurityContext::system();
                let next_id = self.memory_of(&system, agent).await?.chain.next_id().0;
                kept.next_id.fetch_max(next_id, Ordering::SeqCst);
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(kept)
    }
}

/// An agent's memory goes with it: removed, in the kernel's name, when the agent is purged.
#[async_trait]
impl AgentMember for MemoryManager {
    async fn remove_behind(&self, agent: &Agent) -> StoreResult<()> {
        self.agents.lock().await.remove(&agent.id);
        purge_for_good(self.memories.as_ref(), AgentMemoryId::of(agent.id)).await?;
        Ok(())
    }
}
