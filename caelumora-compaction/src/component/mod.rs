//! The component kind `compaction` (issues 269, 284, 464, 534): an agent's context compacted, on
//! its own and when asked; configured with how full the context may be and how much of it is
//! kept. It serves two facets: the compaction on its own, an activity (`activity`), and `/compact`
//! (`command`).
//!
//! On its own, as an activity, once its context fills `threshold` of its model's context size;
//! when asked, by `/compact @agent`, a command the agent carries out itself, for its owners alone.
//! Either way it keeps `keep` of the context size as it stands, below `threshold`, has the rest
//! replaced by what its [`CompactionStrategy`] puts there (`Compactor`), and gives again what fits
//! into a share of the room between the two (`ROOM_TO_GIVE_AGAIN`); it tells the channel what it
//! did. An agent whose personality does not list it does not compact at all.

mod activity;
mod command;
#[cfg(test)]
mod testing;

use crate::compactor::Compactor;
use crate::{CompactionError, CompactionStrategy, KnowledgeRestorer};
use caelumora_agents::activity::ActivityProvider;
use caelumora_agents::command::AgentCommandProvider;
use caelumora_agents::turn::Turn;
use caelumora_api::component::{ComponentError, ComponentResult, Params};
use caelumora_api::component::{ComponentFacets, ComponentKind, LibraryComponents};
use serde::Deserialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// The kind of the component.
pub const COMPACTION_KIND: &str = "compaction";

/// How full the context may be before the agent compacts it, as a share of its model's context
/// size, unless configured.
const DEFAULT_THRESHOLD: f64 = 0.8;

/// How much of the context a compaction keeps as it stands, as a share of its model's context
/// size, unless configured.
const DEFAULT_KEEP: f64 = 0.25;

/// How much of the room between what a compaction keeps and its threshold what it gives again may
/// take: the rest is left for the replacement, the context outside the transcript, and for the
/// context to grow by until the next compaction, which would follow at once without it (issue
/// 542).
const ROOM_TO_GIVE_AGAIN: f64 = 0.5;

/// A component of the kind `compaction`: the agent's context compacted on its own, as an activity,
/// once it fills `threshold`, and by `/compact`.
#[derive(Debug)]
struct CompactionComponent {
    /// What carries out a compaction, with the agent's strategy.
    compactor: Compactor,
    /// How full the context may be before the agent compacts it on its own, as a share of the
    /// context size.
    threshold: f64,
    /// Whether it said that the agent's model reports no context size.
    told_no_size: AtomicBool,
}

/// What the component is configured with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactionParams {
    #[serde(default = "default_threshold")]
    threshold: f64,
    #[serde(default = "default_keep")]
    keep: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

fn default_keep() -> f64 {
    DEFAULT_KEEP
}

impl Params for CompactionParams {
    /// # Errors
    /// `InvalidConfig` if `keep` and `threshold` are no shares of the context size with `keep`
    /// below `threshold`: kept as much as it compacts at, a compaction would free too little.
    fn check(&self) -> ComponentResult<()> {
        let (threshold, keep) = (self.threshold, self.keep);
        if !(0.0 <= keep && keep < threshold && threshold <= 1.0) {
            return Err(ComponentError::InvalidConfig(format!("'keep' and 'threshold' are shares of the context size, 'keep' at least 0 and less than 'threshold', 'threshold' at most 1, not {keep} and {threshold}")));
        }
        Ok(())
    }
}

impl CompactionComponent {
    /// The component compacting with `compactor` on its own once the context fills `threshold`;
    /// that its model reports no context size, it has not said yet.
    fn new(compactor: Compactor, threshold: f64) -> Self {
        Self { compactor, threshold, told_no_size: AtomicBool::new(false) }
    }

    /// Compacts the context of `turn`'s agent: what it did, in words for its channel, or `None`
    /// if there was nothing to compact.
    async fn compact(&self, turn: &mut Turn<'_>) -> Result<Option<String>, CompactionError> {
        Ok(self
            .compactor
            .compact(turn)
            .await?
            .map(|compaction| compaction.describe(turn.agent_name())))
    }
}

/// What the agent `agent_name`'s channel is told when there was nothing to compact.
fn nothing_to_compact(agent_name: &str) -> String {
    format!(
        "{agent_name} found nothing to compact: it keeps the newest part of its context as it is"
    )
}

/// Registers the kind `compaction` with `registry`.
pub(crate) fn register(registry: &mut LibraryComponents<'_>) {
    registry.register_async(
        ComponentKind::new(COMPACTION_KIND)
            .params::<CompactionParams>()
            .provides::<dyn AgentCommandProvider>()
            .provides::<dyn ActivityProvider>()
            .needs::<dyn CompactionStrategy>()
            .needs::<dyn KnowledgeRestorer>(),
        |descriptor, context| async move {
            let CompactionParams { threshold, keep } = CompactionParams::read(&descriptor.params)?;
            let strategy = context.one::<dyn CompactionStrategy>().await?.facet;
            // What the agent's components give again, each of its kind.
            let restorers = context
                .all::<dyn KnowledgeRestorer>()
                .await?
                .into_iter()
                .map(|restorer| restorer.facet)
                .collect();
            let room = ROOM_TO_GIVE_AGAIN * (threshold - keep);
            let compactor = Compactor::new(strategy, restorers, keep, room);
            let component = Arc::new(CompactionComponent::new(compactor, threshold));
            let mut facets = ComponentFacets::empty();
            facets
                .insert::<dyn AgentCommandProvider>(Arc::<CompactionComponent>::clone(&component));
            facets.insert::<dyn ActivityProvider>(component);
            Ok(facets)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn what_it_keeps_and_when_it_compacts_are_shares_of_the_context_size() {
        let read = |params| CompactionParams::read(&params).map(|c| (c.threshold, c.keep));
        assert_eq!(read(Value::Null).ok(), Some((DEFAULT_THRESHOLD, DEFAULT_KEEP)));
        assert_eq!(
            read(serde_json::json!({ "threshold": 0.5, "keep": 0.2 })).ok(),
            Some((0.5, 0.2))
        );
        assert!(
            read(serde_json::json!({ "threshold": 0.5, "keep": 0.5 })).is_err(),
            "kept as much as it compacts at, it would free too little"
        );
        assert!(read(serde_json::json!({ "threshold": 1.5 })).is_err(), "at most all of it");
        assert!(read(serde_json::json!({ "keep": -0.1 })).is_err(), "nothing less than nothing");
        assert!(
            read(serde_json::json!({ "keep_last": 10 })).is_err(),
            "what it does not take is refused"
        );
    }
}
