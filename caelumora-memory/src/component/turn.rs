//! An agent's memory as a turn sees and changes it (issues 62, 518).
//!
//! The memory is the plugin's to keep and to decide about: a work reads what it may recall where
//! it reads from through the `MemoryReader` of its situation, and a turn reserves the number of a
//! new entry through the `MemoryWriter` it is given. What a turn changes it stages
//! (`Turn::stage`), and the plugin commits it when the turn completes, checking every change and
//! deriving where it was made. [`TurnMemory`] is the logic on top: remembering, updating,
//! forgetting and reclassifying, each only of what the turn sees - an entry recalled here, or one
//! it remembered itself -, so that a tool can answer its model at once. The `agent_memory`
//! component uses it for its tools.

use crate::extension::{MemoryReader, MemoryWriter};
use crate::memory::{MemoryChange, MemoryId};
use caelumora_agents::tool::{ToolError, ToolResult};
use caelumora_agents::turn::Turn;
use caelumora_domain::confidentiality::Classification;
use caelumora_domain::ledger::Change;
use std::sync::Arc;

/// An agent's memory as a turn sees it: every function takes the turn.
pub(crate) struct TurnMemory;

impl TurnMemory {
    /// Remembers `text`, classified as the turn has seen so far (`Situation::label`, issue 168):
    /// committed when the turn completes. Returns its number.
    ///
    /// Unlike a note's, the number is reserved at once by the plugin, which keeps the agent's
    /// numbers (`MemoryWriter::reserve_id`), instead of being derived from the memory and the
    /// turn's staged changes: several actors of one agent - in other channels and sessions -
    /// remember at the same time, and would otherwise hand out the same number. Deliberately the
    /// one exception to "commit, don't write" (issue 19).
    ///
    /// # Errors
    /// `ExecutionFailed` where the memory cannot be reached - a turn in no session -, no number
    /// can be reserved, or the change cannot be kept with the turn.
    pub(crate) async fn remember(turn: &mut Turn<'_>, text: String) -> ToolResult<MemoryId> {
        let reserved = Self::writer(turn).await?.reserve_id().await;
        let id = reserved.map_err(|e| {
            ToolError::ExecutionFailed(format!("no number could be reserved for the entry: {e}"))
        })?;
        let classification = turn.situation().label();
        Self::stage(turn, Change::Add { id, text, classification })?;
        Ok(id)
    }

    /// Updates entry `id` - one the turn sees (`knows`) - to say `text` when the turn completes
    /// (issue 302): classified as the turn has seen so far and written in its channel, as an entry
    /// it remembers is, and recalled from then on where one remembered here would be. Returns
    /// whether there was one.
    ///
    /// # Errors
    /// As `remember`.
    pub(crate) async fn update(
        turn: &mut Turn<'_>,
        id: MemoryId,
        text: String,
    ) -> ToolResult<bool> {
        if !Self::knows(turn, id).await? {
            return Ok(false);
        }
        let classification = turn.situation().label();
        Self::stage(turn, Change::Update { id, text, classification })?;
        Ok(true)
    }

    /// Forgets entry `id` - one the turn sees (`knows`) - when the turn completes. Returns whether
    /// there was one.
    ///
    /// # Errors
    /// As `remember`.
    pub(crate) async fn forget(turn: &mut Turn<'_>, id: MemoryId) -> ToolResult<bool> {
        if !Self::knows(turn, id).await? {
            return Ok(false);
        }
        Self::stage(turn, Change::Remove { id })?;
        Ok(true)
    }

    /// Reclassifies entry `id` as `classification` (issue 133) - one the turn sees (`knows`) - if
    /// the kernel lets the agent (`MemoryWriter::may_reclassify`): never above the turn's channel.
    /// Committed when the turn completes; from then on the entry is recalled where its new
    /// classification may be read.
    ///
    /// # Errors
    /// `InvalidParameter` if there is no such entry here, `PermissionDenied` if the policy refuses,
    /// or as `remember`.
    pub(crate) async fn reclassify(
        turn: &mut Turn<'_>,
        id: MemoryId,
        classification: Classification,
    ) -> ToolResult<()> {
        if !Self::knows(turn, id).await? {
            return Err(ToolError::InvalidParameter(format!(
                "there is no entry #{id} you remember here"
            )));
        }
        if !Self::writer(turn).await?.may_reclassify(&classification).await {
            return Err(ToolError::PermissionDenied(format!(
                "reclassifying entry #{id} as {classification}"
            )));
        }
        Self::stage(turn, Change::Reclassify { id, classification })
    }

    /// Whether entry `id` stands as the turn sees it, and is the turn's to change: recalled here
    /// (`MemoryReader::recalled`), or remembered in the turn - and not forgotten in it since.
    async fn knows(turn: &Turn<'_>, id: MemoryId) -> ToolResult<bool> {
        let reader = turn.situation().reader::<dyn MemoryReader>().await;
        let mut known = reader.is_some_and(|memory| memory.recalled().iter().any(|m| m.id == id));
        let staged = turn.staged::<MemoryChange>().map_err(|e| {
            ToolError::ExecutionFailed(format!("what the turn remembered cannot be read: {e}"))
        })?;
        for MemoryChange(change) in staged {
            match change {
                Change::Add { id: added, .. } if added == id => known = true,
                Change::Remove { id: removed } if removed == id => known = false,
                _ => {}
            }
        }
        Ok(known)
    }

    /// The writer of the agent's memory bound to the turn's work.
    async fn writer(turn: &Turn<'_>) -> ToolResult<Arc<dyn MemoryWriter>> {
        turn.writer::<dyn MemoryWriter>()
            .await
            .ok_or_else(|| ToolError::ExecutionFailed("your memory cannot be reached here".into()))
    }

    /// Stages `change` on the turn.
    fn stage(turn: &mut Turn<'_>, change: Change<MemoryId>) -> ToolResult<()> {
        turn.stage(&MemoryChange(change))
            .map_err(|e| ToolError::ExecutionFailed(format!("the change cannot be kept: {e}")))
    }
}
