//! The spans the runtime opens itself.
//!
//! The whole workspace logs through `tracing`; the runtime only emits records.
//! Which subscriber writes them, where and how, is the application's decision:
//! the binaries install it with `caelumora_logging::init` (issue 330).
//!
//! Operations that cross actors are tied together by the ids of their
//! `EventContext`, which the spans carry as fields under the same names:
//! `span_id` (shared by every envelope caused by one initial event),
//! `event_id`, `correlation_id` and `span_depth`. Searching the log
//! for one `span_id` follows a message from the REST request through the
//! channel into the agent's turn. Note that the `span_id` of an envelope chain
//! is unrelated to `tracing`'s own spans, which only structure the log.

use caelumora_model::actor::{ActorState, EventContext};
use caelumora_model::agent::AgentId;
use tracing::Span;

/// The span for handling one envelope in a channel: which channel, who sent
/// it, what kind of event, and the ids that tie it to the rest of its chain.
pub fn envelope_span(context: &EventContext, event: &'static str) -> Span {
    tracing::info_span!(
        "envelope",
        session_id = %context.session_id,
        channel_id = %context.channel_id,
        sender = %context.sender_id,
        event,
        span_id = %context.span_id,
        event_id = %context.event_id,
        correlation_id = %context.correlation_id,
        span_depth = context.span_depth,
    )
}

/// The span one iteration of an agent's run loop executes in, in `state`.
/// The waiting states get an `agent` span; the states of a turn get a
/// `turn` span carrying the ids of `turn`, the context of the envelope that
/// started the turn under way, so the turn can be found by the same
/// `span_id` as the REST request and the channel dispatch. Without a turn
/// under way, an `agent` span too.
pub fn agent_span(
    agent_id: AgentId,
    agent: &str,
    state: ActorState,
    turn: Option<&EventContext>,
) -> Span {
    match turn.filter(|_| state.in_turn()) {
        None => {
            tracing::info_span!("agent", agent_id = %agent_id, agent = %agent, state = ?state)
        }
        Some(turn) => tracing::info_span!(
            "turn",
            agent_id = %agent_id,
            agent = %agent,
            state = ?state,
            session_id = %turn.session_id,
            channel_id = %turn.channel_id,
            span_id = %turn.span_id,
            event_id = %turn.event_id,
            correlation_id = %turn.correlation_id,
            span_depth = turn.span_depth,
        ),
    }
}
