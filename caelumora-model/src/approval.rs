use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::session::SessionId;
use crate::tool::ToolCall;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

define_id!(ApproverId);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approver {
    pub id: ApproverId,
    pub strategy: ApprovalStrategy,
}

define_id!(ApprovalId);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStrategy {
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
    pub fn decide(&self, approvers: &[ActorId], responses: &HashMap<ActorId, ApprovalStatus>) -> Option<ApprovalStatus> {
        let user_count = approvers.len();
        if user_count == 0 {
            return Some(ApprovalStatus::Denied);
        }
        let counted = || responses.iter().filter(|(id, _)| approvers.contains(id)).map(|(_, status)| *status);
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub id: ApprovalId,
    pub agent_id: AgentId,
    /// The rule of the person answering that gave the answer (issue 13),
    /// for the record and their own view; `None` for an answer they gave
    /// themselves, and for a decision. Set only by their actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_rule: Option<ApprovalRuleId>,
}

impl ApprovalResponse {
    /// An answer to the request `id` of agent `agent_id`, given by the
    /// person themselves.
    pub fn new(id: ApprovalId, agent_id: AgentId) -> Self {
        Self { id, agent_id, by_rule: None }
    }
}

/// A decision on an approval: a person's answer, or the approver's verdict.
/// Where the call stands meanwhile is its `ToolCallStatus` (issue 165).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalStatus {
    Approved,
    Denied,
    Timeout,
}

define_id!(ApprovalRuleId);

/// A standing decision on tool calls (issue 13): a place's restriction - of
/// the system, a session or a channel - or, later, a person's answer given
/// in advance. Which it may be is said by where it is kept: a place only
/// restricts (`check_restricting`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRule {
    /// Made anew when a rule is written without one, as in the
    /// configuration.
    #[serde(default = "ApprovalRuleId::new")]
    pub id: ApprovalRuleId,
    pub matcher: ToolMatcher,
    pub decision: RuleDecision,
    /// Narrower than where the rule is kept; `everywhere` unless named.
    #[serde(default, skip_serializing_if = "Reach::is_everywhere")]
    pub reach: Reach,
    /// When it stops applying; never unless named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// What a rule decides of a call it applies to. Ordered by strength: where
/// several rules apply, the strongest decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    /// It runs without anyone being asked.
    Allow,
    /// A person decides this very call, whatever rules of their own they
    /// set.
    Ask,
    /// It never runs: refused as by policy, not put to anyone.
    Deny,
}

/// Where a rule applies, within the place it is kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    #[default]
    Everywhere,
    Session(SessionId),
    Channel(ChannelId),
}

impl Reach {
    pub fn is_everywhere(&self) -> bool {
        *self == Reach::Everywhere
    }

    fn covers(&self, session_id: SessionId, channel_id: ChannelId) -> bool {
        match self {
            Reach::Everywhere => true,
            Reach::Session(id) => *id == session_id,
            Reach::Channel(id) => *id == channel_id,
        }
    }
}

/// What calls a rule is about: the tool by the name the model calls it by -
/// `*` for every tool - and, per argument named, a regular expression its
/// value must match as a whole: a string as it is, any other value as its
/// compact JSON. An argument the call lacks does not match. Reliable for
/// arguments with a structure of their own (a channel, an id, a domain), not
/// for one that is a language (a shell command, SQL), which a pattern cannot
/// tell the meaning of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolMatcher {
    pub tool: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, String>,
}

/// The longest pattern of an argument, in characters.
pub const MAX_PATTERN_CHARS: usize = 500;

/// How large a compiled pattern may grow, in bytes: a short pattern can
/// still compile to a large automaton (`a{1000}{1000}`).
const MAX_COMPILED_PATTERN_BYTES: usize = 1 << 20;

/// Why a rule cannot be kept where it was to be. The messages are meant for
/// whoever wrote the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("a rule must name a tool, or `*` for every tool")]
    NoTool,
    #[error("the pattern of argument `{0}` must not be longer than {MAX_PATTERN_CHARS} characters")]
    PatternTooLong(String),
    #[error("the pattern of argument `{argument}` is no regular expression: {reason}")]
    InvalidPattern { argument: String, reason: String },
    #[error("a session, a channel or the system may only ask or deny, everywhere within it")]
    NotRestricting,
    #[error("a rule of a person allows or denies; asking is what happens without one")]
    NotAnAnswer,
    #[error("a person may keep at most {MAX_PERSONAL_RULES} rules")]
    TooMany,
}

/// The most rules a person may keep: they are matched against every call
/// they are asked to approve.
pub const MAX_PERSONAL_RULES: usize = 200;

impl ToolMatcher {
    /// Every call of every tool.
    pub fn everything() -> Self {
        Self { tool: "*".to_string(), arguments: BTreeMap::new() }
    }

    /// The calls exactly like `call`: its tool, and every argument as it is -
    /// what a rule derived from a call is offered with unless its tool
    /// knows better, for the person to widen.
    pub fn exact(call: &ToolCall) -> Self {
        let arguments = match &call.arguments {
            Value::Object(arguments) => arguments.iter()
                .map(|(name, value)| {
                    let text = match value { Value::String(text) => text.clone(), other => other.to_string() };
                    (name.clone(), regex::escape(&text))
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        Self { tool: call.function_name.clone(), arguments }
    }

    /// Whether it is about every call of every tool.
    pub fn is_everything(&self) -> bool {
        self.tool == "*" && self.arguments.is_empty()
    }

    /// Nothing if every pattern is one that can be matched.
    ///
    /// # Errors
    /// `RuleError` naming the first that is not, or a missing tool.
    pub fn check(&self) -> Result<(), RuleError> {
        if self.tool.trim().is_empty() {
            return Err(RuleError::NoTool);
        }
        for (argument, pattern) in &self.arguments {
            compile(argument, pattern)?;
        }
        Ok(())
    }

    /// Whether `call` is one of the calls this matcher is about.
    ///
    /// # Errors
    /// `RuleError` if a pattern cannot be matched - only for a matcher that
    /// was never checked.
    pub fn matches(&self, call: &ToolCall) -> Result<bool, RuleError> {
        if self.tool != "*" && self.tool != call.function_name {
            return Ok(false);
        }
        for (argument, pattern) in &self.arguments {
            let regex = compile(argument, pattern)?;
            let matched = match call.arguments.get(argument) {
                Some(Value::String(text)) => regex.is_match(text),
                Some(value) => regex.is_match(&value.to_string()),
                None => false,
            };
            if !matched {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// `pattern` compiled to match a whole value.
fn compile(argument: &str, pattern: &str) -> Result<regex::Regex, RuleError> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(RuleError::PatternTooLong(argument.to_string()));
    }
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(MAX_COMPILED_PATTERN_BYTES)
        .build()
        .map_err(|e| RuleError::InvalidPattern { argument: argument.to_string(), reason: e.to_string() })
}

impl ApprovalRule {
    /// A rule deciding `decision` of the calls `matcher` is about, within
    /// `reach`, until `expires_at`.
    ///
    /// # Errors
    /// `RuleError` if the matcher cannot be matched (`ToolMatcher::check`).
    pub fn new(matcher: ToolMatcher, decision: RuleDecision, reach: Reach, expires_at: Option<DateTime<Utc>>) -> Result<Self, RuleError> {
        matcher.check()?;
        Ok(Self { id: ApprovalRuleId::new(), matcher, decision, reach, expires_at })
    }

    /// Nothing if a place - the system, a session, a channel - may keep the
    /// rule: one that asks or denies, everywhere within the place, with a
    /// matcher that can be matched. A place cannot allow: there is nobody
    /// there who would stand for an approval.
    ///
    /// # Errors
    /// `RuleError::NotRestricting`, or what `ToolMatcher::check` reports.
    pub fn check_restricting(&self) -> Result<(), RuleError> {
        if self.decision == RuleDecision::Allow || !self.reach.is_everywhere() {
            return Err(RuleError::NotRestricting);
        }
        self.matcher.check()
    }

    /// Nothing if a person may keep the rule (issue 13): one that allows or
    /// denies - their answer given in advance - with a matcher that can be
    /// matched, within any reach.
    ///
    /// # Errors
    /// `RuleError::NotAnAnswer`, or what `ToolMatcher::check` reports.
    pub fn check_personal(&self) -> Result<(), RuleError> {
        if self.decision == RuleDecision::Ask {
            return Err(RuleError::NotAnAnswer);
        }
        self.matcher.check()
    }

    /// Whether it no longer applies anywhere at `now`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }

    /// Whether the rule applies to `call`, made in the channel `channel_id`
    /// of session `session_id` at `now`: within its reach, not expired, and
    /// about the call. A rule whose matcher cannot be matched applies if it
    /// restricts and does not if it allows: a broken rule never opens
    /// anything.
    pub fn applies(&self, call: &ToolCall, session_id: SessionId, channel_id: ChannelId, now: DateTime<Utc>) -> bool {
        if !self.reach.covers(session_id, channel_id) || self.is_expired(now) {
            return false;
        }
        self.matcher.matches(call).unwrap_or(self.decision != RuleDecision::Allow)
    }

    /// The rule that decides `call`, made in the channel `channel_id` of
    /// session `session_id` at `now`: of those in `rules` that apply, the
    /// strongest (`Deny` before `Ask` before `Allow`), the first of them if
    /// several are as strong. `None` if none applies.
    pub fn decide<'r>(rules: impl IntoIterator<Item = &'r ApprovalRule>, call: &ToolCall, session_id: SessionId, channel_id: ChannelId, now: DateTime<Utc>) -> Option<&'r ApprovalRule> {
        rules.into_iter()
            .filter(|rule| rule.applies(call, session_id, channel_id, now))
            .reduce(|strongest, rule| if rule.decision > strongest.decision { rule } else { strongest })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::UserId;
    use serde_json::json;

    /// What `strategy` decides of `approvers` people, of whom the first
    /// answered as `answers` says, and one who may not approve approved.
    fn decided(strategy: ApprovalStrategy, approvers: usize, answers: &[ApprovalStatus]) -> Option<ApprovalStatus> {
        let approvers: Vec<ActorId> = (0..approvers).map(|_| ActorId::User(UserId::new())).collect();
        let mut responses: HashMap<ActorId, ApprovalStatus> = approvers.iter().copied().zip(answers.iter().copied()).collect();
        responses.insert(ActorId::User(UserId::new()), ApprovalStatus::Approved);
        strategy.decide(&approvers, &responses)
    }

    #[test]
    fn each_strategy_decides_by_the_answers_of_those_who_may_approve() {
        use ApprovalStatus::{Approved, Denied};
        use ApprovalStrategy::{All, Any, DenyOnFirst, Majority};
        for strategy in [Any, Majority, All, DenyOnFirst] {
            assert_eq!(decided(strategy, 0, &[]), Some(Denied), "{strategy:?}: nobody may approve");
            assert_eq!(decided(strategy, 3, &[]), None, "{strategy:?}: nobody answered; who may not approve does not count");
        }

        assert_eq!(decided(Any, 3, &[Denied, Approved]), Some(Approved), "one approval is enough");
        assert_eq!(decided(Any, 3, &[Denied, Denied]), None, "denied only once all denied");
        assert_eq!(decided(Any, 3, &[Denied, Denied, Denied]), Some(Denied));

        assert_eq!(decided(Majority, 3, &[Approved]), None);
        assert_eq!(decided(Majority, 3, &[Approved, Approved]), Some(Approved), "more than half");
        assert_eq!(decided(Majority, 4, &[Approved, Approved, Denied]), None, "half is no majority");
        assert_eq!(decided(Majority, 4, &[Denied, Denied]), Some(Denied), "half denied: no majority can come");
        assert_eq!(decided(Majority, 3, &[Denied, Denied]), Some(Denied));

        assert_eq!(decided(All, 3, &[Approved, Approved]), None);
        assert_eq!(decided(All, 3, &[Approved, Approved, Approved]), Some(Approved));
        assert_eq!(decided(All, 3, &[Approved, Denied]), Some(Denied), "one denial ends it");

        assert_eq!(decided(DenyOnFirst, 3, &[Denied]), Some(Denied), "the first denial ends it");
        assert_eq!(decided(DenyOnFirst, 2, &[Approved]), None);
        assert_eq!(decided(DenyOnFirst, 2, &[Approved, Approved]), Some(Approved));
    }

    fn call(tool: &str, arguments: Value) -> ToolCall {
        ToolCall { id: None, function_name: tool.to_string(), arguments, visibility: Default::default(), presentation: Default::default() }
    }

    fn matcher(tool: &str, arguments: &[(&str, &str)]) -> ToolMatcher {
        ToolMatcher { tool: tool.to_string(), arguments: arguments.iter().map(|(a, p)| (a.to_string(), p.to_string())).collect() }
    }

    fn rule(matcher: ToolMatcher, decision: RuleDecision) -> ApprovalRule {
        ApprovalRule::new(matcher, decision, Reach::Everywhere, None).unwrap()
    }

    #[test]
    fn a_matcher_matches_the_tool_and_every_argument_it_names_as_a_whole() {
        let ls = matcher("shell", &[("command", "ls( .*)?")]);
        assert!(ls.matches(&call("shell", json!({ "command": "ls -l", "cwd": "/" }))).unwrap(), "other arguments do not matter");
        assert!(ls.matches(&call("shell", json!({ "command": "ls" }))).unwrap());
        assert!(!ls.matches(&call("shell", json!({ "command": "rm -rf / ; ls" }))).unwrap(), "the whole value, not a part of it");
        assert!(!ls.matches(&call("shell", json!({ "command": "ls\nrm -rf /" }))).unwrap(), "nor a line of it");
        assert!(!ls.matches(&call("shell", json!({}))).unwrap(), "an argument the call lacks");
        assert!(!ls.matches(&call("shell", Value::Null)).unwrap(), "a call published without its content");
        assert!(!ls.matches(&call("other", json!({ "command": "ls" }))).unwrap(), "another tool");

        let note = matcher("note", &[("id", "[0-9]+")]);
        assert!(note.matches(&call("note", json!({ "id": 42 }))).unwrap(), "a number as its JSON");
        assert!(ToolMatcher::everything().matches(&call("anything", json!({ "x": 1 }))).unwrap());
        assert!(matcher("note", &[]).matches(&call("note", Value::Null)).unwrap(), "a tool alone, whatever its arguments");
    }

    #[test]
    fn a_matcher_that_cannot_be_matched_is_refused() {
        assert_eq!(matcher(" ", &[]).check(), Err(RuleError::NoTool));
        assert!(matches!(matcher("shell", &[("command", "(")]).check(), Err(RuleError::InvalidPattern { argument, .. }) if argument == "command"));
        assert_eq!(matcher("shell", &[("command", &"a".repeat(MAX_PATTERN_CHARS + 1))]).check(), Err(RuleError::PatternTooLong("command".to_string())));
        assert!(matches!(matcher("shell", &[("command", "a{1000}{1000}")]).check(), Err(RuleError::InvalidPattern { .. })), "too large once compiled");
        assert!(ApprovalRule::new(matcher("shell", &[("command", "(")]), RuleDecision::Deny, Reach::Everywhere, None).is_err());
    }

    #[test]
    fn the_strongest_rule_that_applies_decides() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let now = Utc::now();
        let ls = call("shell", json!({ "command": "ls" }));
        let allow = rule(matcher("shell", &[]), RuleDecision::Allow);
        let ask = rule(matcher("shell", &[("command", "ls")]), RuleDecision::Ask);
        let deny = rule(matcher("*", &[]), RuleDecision::Deny);
        let other = rule(matcher("note", &[]), RuleDecision::Deny);

        assert_eq!(ApprovalRule::decide([&allow, &ask], &ls, session, channel, now).map(|r| r.id), Some(ask.id), "ask before allow");
        assert_eq!(ApprovalRule::decide([&ask, &deny, &allow], &ls, session, channel, now).map(|r| r.id), Some(deny.id), "deny before ask");
        assert_eq!(ApprovalRule::decide([&other], &ls, session, channel, now), None, "about another tool");
        let first = rule(matcher("shell", &[]), RuleDecision::Ask);
        assert_eq!(ApprovalRule::decide([&first, &ask], &ls, session, channel, now).map(|r| r.id), Some(first.id), "the first of the strongest");
    }

    #[test]
    fn a_rule_applies_within_its_reach_until_it_expires() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let now = Utc::now();
        let ls = call("shell", json!({ "command": "ls" }));
        let within = |reach| ApprovalRule::new(matcher("shell", &[]), RuleDecision::Deny, reach, None).unwrap();
        assert!(within(Reach::Session(session)).applies(&ls, session, channel, now));
        assert!(!within(Reach::Session(SessionId::new())).applies(&ls, session, channel, now), "another session");
        assert!(within(Reach::Channel(channel)).applies(&ls, session, channel, now));
        assert!(!within(Reach::Channel(ChannelId::new())).applies(&ls, session, channel, now), "another channel");

        let until = |at| ApprovalRule::new(matcher("shell", &[]), RuleDecision::Deny, Reach::Everywhere, Some(at)).unwrap();
        assert!(until(now + chrono::Duration::minutes(1)).applies(&ls, session, channel, now));
        assert!(!until(now).applies(&ls, session, channel, now), "expired");
    }

    #[test]
    fn a_broken_rule_restricts_and_never_allows() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let ls = call("shell", json!({ "command": "ls" }));
        // As it could be read from a document written by hand: never checked.
        let broken = |decision| ApprovalRule { id: ApprovalRuleId::new(), matcher: matcher("shell", &[("command", "(")]), decision, reach: Reach::Everywhere, expires_at: None };
        assert!(broken(RuleDecision::Deny).applies(&ls, session, channel, Utc::now()));
        assert!(!broken(RuleDecision::Allow).applies(&ls, session, channel, Utc::now()));
        assert!(!broken(RuleDecision::Deny).applies(&call("note", json!({})), session, channel, Utc::now()), "only for its tool");
    }

    #[test]
    fn a_call_proposes_the_rule_for_the_calls_exactly_like_it() {
        let ls = call("shell", json!({ "command": "ls -l *.rs", "depth": 2 }));
        let exact = ToolMatcher::exact(&ls);
        assert!(exact.check().is_ok());
        assert!(exact.matches(&ls).unwrap(), "itself");
        assert!(!exact.matches(&call("shell", json!({ "command": "ls -l main.rs", "depth": 2 }))).unwrap(), "a `*` is no wildcard in it");
        assert!(!exact.matches(&call("shell", json!({ "command": "ls -l *.rs", "depth": 3 }))).unwrap(), "every argument counts");
        assert_eq!(ToolMatcher::exact(&call("stop", Value::Null)), matcher("stop", &[]), "a call without arguments: its tool");
        assert!(ToolMatcher::everything().is_everything() && !exact.is_everything());
    }

    #[test]
    fn a_person_allows_or_denies_within_any_reach() {
        assert!(rule(matcher("shell", &[]), RuleDecision::Allow).check_personal().is_ok());
        assert!(rule(matcher("shell", &[]), RuleDecision::Deny).check_personal().is_ok());
        assert_eq!(rule(matcher("shell", &[]), RuleDecision::Ask).check_personal(), Err(RuleError::NotAnAnswer));
        let narrower = ApprovalRule::new(matcher("shell", &[]), RuleDecision::Allow, Reach::Channel(ChannelId::new()), None).unwrap();
        assert!(narrower.check_personal().is_ok());
    }

    #[test]
    fn a_place_only_asks_or_denies_everywhere_within_it() {
        assert!(rule(matcher("shell", &[]), RuleDecision::Deny).check_restricting().is_ok());
        assert!(rule(matcher("shell", &[]), RuleDecision::Ask).check_restricting().is_ok());
        assert_eq!(rule(matcher("shell", &[]), RuleDecision::Allow).check_restricting(), Err(RuleError::NotRestricting));
        let narrower = ApprovalRule::new(matcher("shell", &[]), RuleDecision::Deny, Reach::Session(SessionId::new()), None).unwrap();
        assert_eq!(narrower.check_restricting(), Err(RuleError::NotRestricting));
    }

    #[test]
    fn a_rule_is_written_as_briefly_as_it_can_be() {
        let written: ApprovalRule = serde_json::from_value(json!({ "matcher": { "tool": "shell", "arguments": { "command": "rm .*" } }, "decision": "deny" })).unwrap();
        assert_eq!((written.decision, written.reach, written.expires_at), (RuleDecision::Deny, Reach::Everywhere, None));
        let back = serde_json::to_value(&written).unwrap();
        assert!(back.get("reach").is_none() && back.get("expires_at").is_none(), "{back}");
        let reach: Reach = serde_json::from_value(json!({ "session": SessionId::new() })).unwrap();
        assert!(matches!(reach, Reach::Session(_)));
        assert!(serde_json::from_value::<ApprovalRule>(json!({ "matcher": { "tool": "shell" }, "decison": "deny" })).is_err(), "a misspelt key");
    }
}
