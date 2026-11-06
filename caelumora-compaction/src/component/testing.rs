//! What the tests of the component and of its facets share: the component with a strategy of a
//! test, and what an agent's turn did before it compacts.

use super::{CompactionComponent, DEFAULT_THRESHOLD};
use crate::compactor::Compactor;
use crate::testing::Scripted;
use caelumora_agents::turn::Turn;
use caelumora_domain::agent::AgentId;
use std::sync::Arc;

/// The component with `strategy`, keeping a tenth of the context, giving nothing again.
pub(super) fn component(strategy: &Arc<Scripted>) -> CompactionComponent {
    let compactor = Compactor::new(Arc::<Scripted>::clone(strategy), Vec::new(), 0.1, 0.0);
    CompactionComponent::new(compactor, DEFAULT_THRESHOLD)
}

/// What the turn of an agent that heard `u1` did before it compacts.
pub(super) type Before = fn(&mut Turn<'_>, AgentId);

/// Nothing done in the turn: nothing to compact, beside `two_calls`.
pub(super) fn nothing(_: &mut Turn<'_>, _: AgentId) {}
