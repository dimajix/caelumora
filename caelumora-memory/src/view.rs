//! How an agent's work reaches its memory (issue 518): its reader and its writer
//! (`crate::extension`), made by the plugin from the session of every piece of work as the work
//! reads it, which decide what of the memory may be recalled where the work reads from and check
//! every change a turn staged as they commit it, in the work's context.
//!
//! An entry may be recalled where it may be copied from where it was written into the place of the
//! work (`ConfidentialityPolicy::may_copy_information`, issues 131, 133), asked once per channel,
//! classification and session the entries come with. An entry of another session (issue 134) is
//! recalled only where both the privacy its session had when it was written and the one it has now
//! permit it, looked up through the work's views; of a session that is gone, the one it had
//! decides, and one that cannot be looked up recalls nothing. Where a change was made the writer
//! derives from the place of the work - its channel, and its session's privacy as it is when the
//! change is committed -, never from what the agent side tells.

use crate::access::{MemoryAccess, RECLASSIFY};
use crate::extension::{MemoryReader, MemoryWriter};
use crate::manager::MemoryManager;
use crate::memory::{AgentMemory, Memory, MemoryChange, MemoryId, MemoryOrigin};
use async_trait::async_trait;
use caelumora_api::staging::{CommitError, CommitResult, Committing};
use caelumora_domain::actor::ActorId;
use caelumora_domain::agent::AgentId;
use caelumora_domain::confidentiality::{Classification, Place};
use caelumora_domain::digest::Digest;
use caelumora_domain::ledger::Change;
use caelumora_domain::message::MessageId;
use caelumora_domain::privacy::SessionPrivacy;
use caelumora_domain::session::SessionId;
use caelumora_domain::store::{StoreResult, Stored};
use caelumora_kernel::view::SessionView;
use caelumora_security::confidentiality::ConfidentialityQuestions;
use caelumora_security::{ActionType, Enforcement};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

/// What of its agent's memory a work may recall where it reads from, as the memory was when it was
/// made: the reader.
pub(crate) struct RecalledMemory {
    recalled: Vec<Memory>,
    digest: Option<Digest>,
}

impl RecalledMemory {
    /// The memory the work that reads `session` may recall, of its agent; `None` for a work of no
    /// agent's, that reads from no place, or whose agent's memory cannot be read - which is
    /// logged.
    pub(crate) async fn of_work(session: &SessionView, manager: &MemoryManager) -> Option<Self> {
        let (agent, here) = work_of(session)?;
        let memory = match manager.memory_of(session.views().context(), agent).await {
            Ok(memory) => memory,
            Err(e) => {
                warn!(agent = %agent, "The memory of an agent could not be read: {e}");
                return None;
            }
        };
        let recalled = recallable(session, &here, memory.chain.current()).await;
        Some(Self { recalled, digest: memory.chain.digest() })
    }
}

impl MemoryReader for RecalledMemory {
    fn recalled(&self) -> &[Memory] {
        &self.recalled
    }

    fn digest(&self) -> Option<Digest> {
        self.digest
    }
}

/// What a turn of a work changes of its agent's memory, committed in the work's context: the
/// writer.
pub(crate) struct MemoryUse {
    session: SessionView,
    agent: AgentId,
    /// Where the work reads from, which every change it commits is made at.
    here: Place,
    manager: Arc<MemoryManager>,
}

impl MemoryUse {
    /// The memory the work that reads `session` changes, of its agent; `None` as for
    /// `RecalledMemory::of_work`.
    pub(crate) fn of_work(session: SessionView, manager: Arc<MemoryManager>) -> Option<Self> {
        let (agent, here) = work_of(&session)?;
        Some(Self { session, agent, here, manager })
    }

    /// Refuses `classification` for what the work writes if it may not flow into the work's place.
    fn require_within(&self, classification: &Classification) -> CommitResult<()> {
        let views = self.session.views();
        let within = &self.here.classification;
        if views.confidentiality().may_publish(views.context(), classification, within) {
            return Ok(());
        }
        Err(CommitError::Invalid(format!("an entry classified {classification} is above the work")))
    }

    /// Refuses an entry of `memory` to be reclassified as `classification` unless the policy lets
    /// the work (`RECLASSIFY`), and the classification is within the work's.
    async fn require_reclassify(
        &self,
        memory: &AgentMemory,
        classification: &Classification,
    ) -> CommitResult<()> {
        let views = self.session.views();
        let reclassify =
            ActionType::custom(AgentMemory::KIND, &RECLASSIFY, &memory.security_attributes);
        let what = || format!("reclassifying an entry as {classification}");
        views
            .access_policy()
            .require(views.context(), reclassify, |refused| CommitError::Refused {
                what: what(),
                refused,
            })
            .await?;
        self.require_within(classification)
    }
}

#[async_trait]
impl Committing<MemoryChange> for MemoryUse {
    /// Applies `changes` to the memory as it is stored now, made at the work's place, if every one
    /// is the work's to make: a new entry of a number reserved and never given, nor added before in
    /// `changes`; an entry changed that may be recalled here or was added before in `changes`; each
    /// classified within the work, and reclassified as the policy lets it. Judged under the agent's
    /// lock, on the memory as `MemoryManager::apply` reads it there.
    ///
    /// Who reserved a number is not kept: a number another turn of the agent reserved and has not
    /// committed yet passes as one of this turn's.
    async fn commit(&self, changes: Vec<MemoryChange>, after: MessageId) -> CommitResult<()> {
        let context = self.session.views().context();
        let origin =
            MemoryOrigin { channel: self.here.channel, session: self.here.session.clone() };
        self.manager
            .apply(context, self.agent, origin, after, async |memory: &AgentMemory| {
                self.check(memory, &changes).await?;
                Ok(changes)
            })
            .await
    }
}

impl MemoryUse {
    /// Refuses `changes` unless every one is the work's to make on `memory` (see `commit`).
    async fn check(&self, memory: &AgentMemory, changes: &[MemoryChange]) -> CommitResult<()> {
        let recalled = recallable(&self.session, &self.here, memory.chain.current()).await;
        let mut changeable: HashSet<MemoryId> = recalled.iter().map(|entry| entry.id).collect();
        let mut added: HashSet<MemoryId> = HashSet::new();
        let changing = |changeable: &HashSet<MemoryId>, id: &MemoryId| {
            if changeable.contains(id) {
                return Ok(());
            }
            Err(CommitError::Invalid(format!("entry #{id} is not the work's to change")))
        };
        for MemoryChange(change) in changes {
            match change {
                Change::Add { id, classification, .. } => {
                    let reserved = self.manager.is_reserved(self.agent, *id).await?;
                    if memory.chain.has_given(*id) || !added.insert(*id) || !reserved {
                        return Err(CommitError::Invalid(format!("entry #{id} is no new one")));
                    }
                    self.require_within(classification)?;
                    changeable.insert(*id);
                }
                Change::Update { id, classification, .. } => {
                    changing(&changeable, id)?;
                    self.require_within(classification)?;
                }
                Change::Remove { id } => {
                    changing(&changeable, id)?;
                    changeable.remove(id);
                }
                Change::Reclassify { id, classification } => {
                    changing(&changeable, id)?;
                    self.require_reclassify(memory, classification).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl MemoryWriter for MemoryUse {
    async fn reserve_id(&self) -> StoreResult<MemoryId> {
        self.manager.reserve_id(self.agent).await
    }

    async fn may_reclassify(&self, classification: &Classification) -> bool {
        let views = self.session.views();
        let Ok(memory) = self.manager.memory_of(views.context(), self.agent).await else {
            return false;
        };
        let attributes = &memory.security_attributes;
        views.access_policy().may_reclassify_memory(views.context(), attributes).await
            && self.require_within(classification).is_ok()
    }
}

/// The agent whose work reads `session`, and where the work reads from; `None` for a work of no
/// agent's, or that reads from no place.
fn work_of(session: &SessionView) -> Option<(AgentId, Place)> {
    let views = session.views();
    let ActorId::Agent(agent) = views.context().caller_id else { return None };
    Some((agent, views.viewer().place()?.clone()))
}

/// Of `memory`, what may be recalled at `here`, where the work that reads `session` reads from (see
/// the module's description).
async fn recallable(session: &SessionView, here: &Place, memory: Vec<Memory>) -> Vec<Memory> {
    let mut sessions: Vec<(SessionId, Option<SessionPrivacy>)> = Vec::new();
    let mut decided: Vec<(MemoryOrigin, Classification, bool)> = Vec::new();
    let mut recalled = Vec::new();
    for entry in memory {
        let known = decided
            .iter()
            .find(|(origin, c, _)| *origin == entry.origin && *c == entry.classification)
            .map(|(.., readable)| *readable);
        let readable = match known {
            Some(readable) => readable,
            None => {
                let readable = may_recall(session, here, &entry, &mut sessions).await;
                decided.push((entry.origin.clone(), entry.classification.clone(), readable));
                readable
            }
        };
        if readable {
            recalled.push(entry);
        }
    }
    recalled
}

/// Whether `entry` may be recalled at `here` (see the module's description). `sessions` holds the
/// privacy of the other sessions looked up so far - `None` for one that is gone.
async fn may_recall(
    session: &SessionView,
    here: &Place,
    entry: &Memory,
    sessions: &mut Vec<(SessionId, Option<SessionPrivacy>)>,
) -> bool {
    let views = session.views();
    let origin = &entry.origin;
    // Where it was written, with its session as privacy sees it then or now.
    let source = |session: SessionPrivacy| Place {
        channel: origin.channel,
        classification: entry.classification.clone(),
        session,
    };
    let copies =
        |from: &Place| views.confidentiality().may_copy_information(views.context(), from, here);
    if origin.session.id == here.session.id {
        return copies(&source(here.session.clone()));
    }
    if !copies(&source(origin.session.clone())) {
        return false;
    }
    let id = origin.session.id;
    let current = match sessions.iter().find(|(known, _)| *known == id) {
        Some((_, current)) => current.clone(),
        None => match views.privacy_of(id).await {
            Ok(current) => {
                sessions.push((id, current.clone()));
                current
            }
            Err(e) => {
                warn!("Looking up session {} to recall from failed: {}", id, e);
                return false;
            }
        },
    };
    match current {
        Some(current) => copies(&source(current)),
        None => true,
    }
}
