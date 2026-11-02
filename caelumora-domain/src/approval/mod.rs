//! The approval of tool calls: who is asked and how their answers decide (`Approver`,
//! `ApprovalStrategy`), and a request with its answers and how it stands (`ApprovalRequest`,
//! `ApprovalResponse`, `ApprovalDecision`, `ApprovalStatus`). The standing decisions on calls - the
//! rules - are `rule`'s, the reading of a command line for them `shell`'s.

mod rule;
mod shell;

pub use rule::{
    ApprovalRule, ApprovalRuleId, ArgumentMatcher, Coverage, Reach, RuleDecision, RuleError,
    ToolMatcher, MAX_EQUALS_CHARS, MAX_PATTERN_CHARS, MAX_PERSONAL_RULES, MAX_SHELL_WORDS,
    MAX_WORD_CHARS, PROPOSED_SHELL_WORDS,
};

use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::session::SessionId;
use crate::tool::ToolCall;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

define_id!(ApproverId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approver {
    pub id: ApproverId,
    pub strategy: ApprovalStrategy,
}

impl Approver {
    /// A new approver, deciding by `strategy`.
    pub fn new(strategy: ApprovalStrategy) -> Self {
        Self { id: ApproverId::new(), strategy }
    }
}

define_id!(ApprovalId);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStrategy {
    #[default]
    Any,
    Majority,
    All,
    DenyOnFirst,
}

impl ApprovalStrategy {
    /// The decision this strategy reaches from `responses`, counting the
    /// answers of `approvers` - those who may approve - and only theirs: who
    /// may not does not count, either way. `None` while it waits for more
    /// answers. Without anyone who may approve, it is denied.
    pub fn decide(
        &self,
        approvers: &[ActorId],
        responses: &HashMap<ActorId, ApprovalStatus>,
    ) -> Option<ApprovalStatus> {
        let user_count = approvers.len();
        if user_count == 0 {
            return Some(ApprovalStatus::Denied);
        }
        let counted =
            || responses.iter().filter(|(id, _)| approvers.contains(id)).map(|(_, status)| *status);
        let approved_count = counted().filter(|&s| s == ApprovalStatus::Approved).count();
        let denied_count = counted().filter(|&s| s == ApprovalStatus::Denied).count();
        let total_responses = counted().count();

        match self {
            ApprovalStrategy::Any => {
                if approved_count > 0 {
                    Some(ApprovalStatus::Approved)
                } else if total_responses >= user_count && denied_count == user_count {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::Majority => {
                if approved_count > user_count / 2 {
                    Some(ApprovalStatus::Approved)
                } else if denied_count >= user_count.div_ceil(2) {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::All => {
                if approved_count >= user_count {
                    Some(ApprovalStatus::Approved)
                } else if denied_count > 0 {
                    Some(ApprovalStatus::Denied)
                } else {
                    None
                }
            }
            ApprovalStrategy::DenyOnFirst => {
                if denied_count > 0 {
                    Some(ApprovalStatus::Denied)
                } else if approved_count >= user_count {
                    Some(ApprovalStatus::Approved)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub tool_call: ToolCall,
    pub description: String,
    /// Whether a rule of a place asked for this very call to be decided by
    /// a person (issue 13): no rule of a person answers it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub explicit: bool,
    /// What other calls are like this one, as its tool says (issue 13): the
    /// matcher a rule derived from it is offered with. `None` for a request
    /// asked before there were any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested: Option<ToolMatcher>,
}

/// An answer to the approval `id` of the call of agent `agent_id`, or the
/// approver's decision on it - which `agent_id` routes to the agent and
/// whose permission it is checked by.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub id: ApprovalId,
    pub agent_id: AgentId,
    /// Who decided (issue 419): a person's answer holds theirs alone, set by
    /// their actor - the channel lets in no other -; the approver's decision
    /// those whose answers made it, as it counted them, empty where nobody's
    /// did: it timed out, or nobody may approve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<ApprovalDecision>,
}

impl ApprovalResponse {
    /// An answer to the request `id` of agent `agent_id`, before the actor
    /// that sends it says whose it is.
    pub fn new(id: ApprovalId, agent_id: AgentId) -> Self {
        Self { id, agent_id, decisions: Vec::new() }
    }
}

/// One person's part in an approval (issue 419): `by`, called `name` then,
/// answered at `at`, by their rule `by_rule` if one answered for them
/// (issue 13). What a person's answer says of them, and what the record of
/// a call keeps of whoever's answers decided it - the reader of the message
/// sees by whom it was approved or denied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub by: ActorId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_rule: Option<ApprovalRuleId>,
    pub at: DateTime<Utc>,
}

/// A decision on an approval: a person's answer, or the approver's verdict.
/// Where the call stands meanwhile is its `ToolCallStatus` (issue 165).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Approved,
    Denied,
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::UserId;

    /// What `strategy` decides of `approvers` people, of whom the first
    /// answered as `answers` says, and one who may not approve approved.
    fn decided(
        strategy: ApprovalStrategy,
        approvers: usize,
        answers: &[ApprovalStatus],
    ) -> Option<ApprovalStatus> {
        let approvers: Vec<ActorId> =
            (0..approvers).map(|_| ActorId::User(UserId::new())).collect();
        let mut responses: HashMap<ActorId, ApprovalStatus> =
            approvers.iter().copied().zip(answers.iter().copied()).collect();
        responses.insert(ActorId::User(UserId::new()), ApprovalStatus::Approved);
        strategy.decide(&approvers, &responses)
    }

    #[test]
    fn each_strategy_decides_by_the_answers_of_those_who_may_approve() {
        use ApprovalStatus::{Approved, Denied};
        use ApprovalStrategy::{All, Any, DenyOnFirst, Majority};
        for strategy in [Any, Majority, All, DenyOnFirst] {
            assert_eq!(decided(strategy, 0, &[]), Some(Denied), "{strategy:?}: nobody may approve");
            assert_eq!(
                decided(strategy, 3, &[]),
                None,
                "{strategy:?}: nobody answered; who may not approve does not count"
            );
        }

        assert_eq!(decided(Any, 3, &[Denied, Approved]), Some(Approved), "one approval is enough");
        assert_eq!(decided(Any, 3, &[Denied, Denied]), None, "denied only once all denied");
        assert_eq!(decided(Any, 3, &[Denied, Denied, Denied]), Some(Denied));

        assert_eq!(decided(Majority, 3, &[Approved]), None);
        assert_eq!(decided(Majority, 3, &[Approved, Approved]), Some(Approved), "more than half");
        assert_eq!(
            decided(Majority, 4, &[Approved, Approved, Denied]),
            None,
            "half is no majority"
        );
        assert_eq!(
            decided(Majority, 4, &[Denied, Denied]),
            Some(Denied),
            "half denied: no majority can come"
        );
        assert_eq!(decided(Majority, 3, &[Denied, Denied]), Some(Denied));

        assert_eq!(decided(All, 3, &[Approved, Approved]), None);
        assert_eq!(decided(All, 3, &[Approved, Approved, Approved]), Some(Approved));
        assert_eq!(decided(All, 3, &[Approved, Denied]), Some(Denied), "one denial ends it");

        assert_eq!(decided(DenyOnFirst, 3, &[Denied]), Some(Denied), "the first denial ends it");
        assert_eq!(decided(DenyOnFirst, 2, &[Approved]), None);
        assert_eq!(decided(DenyOnFirst, 2, &[Approved, Approved]), Some(Approved));
    }
}
