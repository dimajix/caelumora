//! Personalities and states of an agent to write beside, and what the tests read of an agent's
//! state past its API, for the tests of this crate and of others; agents stored with a
//! personality of their own are `caelumora_store::testing::stored_agent`.

use super::{AgentChannelState, AgentId, Personality, PersonalityId};
use crate::actor::{ActorId, ActorState, EventContext};
use crate::channel::ChannelId;
use crate::context::ContextRef;
use crate::message::Message;
use crate::scope::Scope;
use crate::security::SecurityAttributes;
use crate::session::SessionId;
use crate::tool::{ToolCall, ToolCallState};
use crate::turn::TurnState;
use crate::value::name::ScopedName;

/// A personality `name` of the global scope, the system's as a seed's is, and naming nothing
/// else: internal, without roles, description or introduction, with the default components,
/// reasoning visibility and hearing during a turn.
#[expect(
    clippy::expect_used,
    reason = "test code: a name that is none fails the test that gives it"
)]
pub fn make_personality(name: &str) -> Personality {
    Personality {
        id: PersonalityId::new(),
        name: ScopedName::new(name, Scope::GlobalScope)
            .expect("a test's name of a personality is one"),
        kind: Default::default(),
        roles: None,
        description: None,
        introduction: None,
        components: Default::default(),
        security_attributes: SecurityAttributes::systems(),
        reasoning_visibility: Default::default(),
        hears_during_turn: Default::default(),
        deleted: None,
        revision: Default::default(),
        purging: false,
    }
}

/// The state of `agent` in the channel `channel_id` of the session `session_id`, whose turn waits
/// for the approval of `calls` (issue 172), each described as `call <function name>`, none
/// suggesting a rule: what it stores while it waits, from which the approvals pending are read.
pub fn waiting_for_approval(
    session_id: SessionId,
    channel_id: ChannelId,
    agent: AgentId,
    calls: Vec<ToolCall>,
) -> AgentChannelState {
    let sender = ActorId::Agent(agent);
    let mut turn = TurnState::default();
    turn.work.event_context = EventContext::new(session_id, channel_id, sender, sender.to_string());
    turn.tool_calls = calls
        .into_iter()
        .map(|call| {
            let description = Some(format!("call {}", call.function_name));
            ToolCallState::pending(ToolCall { description, ..call }, false, None, None)
        })
        .collect();
    AgentChannelState {
        processing_state: ActorState::WaitingForUser,
        turn: Some(turn),
        ..AgentChannelState::default()
    }
}

/// What the tests read of an agent's state past its own API.
impl AgentChannelState {
    /// The context `message`, said by the agent, was produced from, as the
    /// state holds it (issue 426): its frame read from what is held. For a
    /// test that resolves what its turns said in memory; `None` if the
    /// message was not produced from a context, or its frame is stored and
    /// let go (`keep_window`).
    pub fn context_of(&self, message: &Message) -> Option<ContextRef> {
        let (transcript, frame) = message.produced_from()?;
        Some(ContextRef { transcript, frame: self.frames.get(&frame)?.clone() })
    }
}
