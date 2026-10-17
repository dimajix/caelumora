use crate::actor::ActorId;
use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::session::SessionId;
use crate::tool::{ArgumentLanguage, ToolCall, ToolParameters};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use regex::{Regex, RegexBuilder};

mod shell;

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
/// `*` for every tool - and, per argument named, how its value is compared
/// (`ArgumentMatcher`). An argument the call lacks does not match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolMatcher {
    pub tool: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, ArgumentMatcher>,
}

/// How the value of one argument of a call is compared (issue 247), written as a string for a
/// pattern and as an object naming the language for the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ArgumentMatcher {
    /// A regular expression the value must match as a whole: a string as it is, any other value
    /// as its compact JSON. Reliable for an argument with a structure of its own (a channel, an
    /// id, a domain), not for one that is a language (a shell command, SQL), whose meaning a
    /// pattern cannot tell: `ls.*` matches `ls; rm x`.
    Pattern(String),
    /// A bash command line, read into its simple commands (`shell::simple_commands`): a simple
    /// command matches if it begins with the words `shell`, as they run. A line that cannot be
    /// judged, or a value that is no string, is unjudgeable.
    Shell { shell: Vec<String> },
}

/// How much of a call a matcher is about, from least to most; of several arguments, the least
/// counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Coverage {
    /// Not the call.
    Nothing,
    /// A command line it compares cannot be judged.
    Unjudgeable,
    /// Some, not all, of the simple commands of a command line it compares.
    Part,
    /// The whole call.
    Whole,
}

/// The longest pattern of an argument, in characters.
pub const MAX_PATTERN_CHARS: usize = 500;

/// The most words a shell argument names.
pub const MAX_SHELL_WORDS: usize = 20;

/// The longest word a shell argument names, in characters.
pub const MAX_WORD_CHARS: usize = 500;

/// How many words of a command a rule derived from it is proposed with: the program and the first
/// word after it - `cargo test`, `git status`, `ls -la`.
pub const PROPOSED_SHELL_WORDS: usize = 2;

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
    #[error("the shell argument `{0}` must belong to a named tool, not to `*`")]
    ShellOfEveryTool(String),
    #[error("the shell argument `{0}` must name between 1 and {MAX_SHELL_WORDS} words")]
    ShellWords(String),
    #[error("the words of the shell argument `{0}` must not be longer than {MAX_WORD_CHARS} characters")]
    WordTooLong(String),
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
                    (name.clone(), ArgumentMatcher::Pattern(regex::escape(&text)))
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        Self { tool: call.function_name.clone(), arguments }
    }

    /// The calls like `call` as the parameters `parameters` of its tool tell (issue 247): every
    /// argument as `exact` takes it, but a shell line of one simple command by its first
    /// `PROPOSED_SHELL_WORDS` words, as far as they do not expand - a rule the call itself
    /// matches. A chain of commands, or a line that cannot be judged, keeps its exact pattern: a
    /// rule for that line alone.
    pub fn proposed(call: &ToolCall, parameters: &ToolParameters) -> Self {
        let mut matcher = Self::exact(call);
        for (argument, proposed) in &mut matcher.arguments {
            let shell = match (parameters.language_of(argument), call.arguments.get(argument)) {
                (Some(ArgumentLanguage::Shell), Some(Value::String(line))) => ArgumentMatcher::first_words(line),
                _ => None,
            };
            if let Some(shell) = shell.filter(|shell| shell.check(argument, &call.function_name).is_ok()) {
                *proposed = shell;
            }
        }
        matcher
    }

    /// Whether it is about every call of every tool.
    pub fn is_everything(&self) -> bool {
        self.tool == "*" && self.arguments.is_empty()
    }

    /// Nothing if it names a tool and every argument is one that can be matched.
    ///
    /// # Errors
    /// `RuleError` naming the first argument that is not, or a missing tool.
    pub fn check(&self) -> Result<(), RuleError> {
        if self.tool.trim().is_empty() {
            return Err(RuleError::NoTool);
        }
        for (argument, matcher) in &self.arguments {
            matcher.check(argument, &self.tool)?;
        }
        Ok(())
    }

    /// How much of `call` this matcher is about: `Nothing` for another tool, otherwise the least
    /// of what its arguments say.
    ///
    /// # Errors
    /// `RuleError` if an argument cannot be matched - only for a matcher
    /// that was never checked.
    pub fn matches(&self, call: &ToolCall) -> Result<Coverage, RuleError> {
        if self.tool != "*" && self.tool != call.function_name {
            return Ok(Coverage::Nothing);
        }
        let mut coverage = Coverage::Whole;
        for (argument, matcher) in &self.arguments {
            coverage = coverage.min(matcher.matches(argument, call.arguments.get(argument))?);
            if coverage == Coverage::Nothing {
                break;
            }
        }
        Ok(coverage)
    }
}

impl ArgumentMatcher {
    /// The shell argument of the first `PROPOSED_SHELL_WORDS` words of `line`, as far as they do
    /// not expand; `None` unless it is one simple command.
    fn first_words(line: &str) -> Option<Self> {
        let [words]: [Vec<shell::Word>; 1] = shell::simple_commands(line).ok()?.try_into().ok()?;
        let shell = words.iter().take(PROPOSED_SHELL_WORDS).map_while(shell::Word::literal).map(str::to_string).collect();
        Some(ArgumentMatcher::Shell { shell })
    }

    /// Nothing if it can be matched as the argument `argument` of the tool `tool`: a pattern that
    /// compiles, a shell argument of a named tool with as many and as long words as allowed.
    ///
    /// # Errors
    /// `RuleError` saying why not.
    fn check(&self, argument: &str, tool: &str) -> Result<(), RuleError> {
        match self {
            ArgumentMatcher::Pattern(pattern) => compile(argument, pattern).map(drop),
            ArgumentMatcher::Shell { shell } => {
                if tool == "*" {
                    return Err(RuleError::ShellOfEveryTool(argument.to_string()));
                }
                if shell.is_empty() || shell.len() > MAX_SHELL_WORDS {
                    return Err(RuleError::ShellWords(argument.to_string()));
                }
                if shell.iter().any(|word| word.chars().count() > MAX_WORD_CHARS) {
                    return Err(RuleError::WordTooLong(argument.to_string()));
                }
                Ok(())
            }
        }
    }

    /// How much of the value `value` of the argument `argument` it is about.
    ///
    /// # Errors
    /// `RuleError` for a pattern that cannot be matched, or a shell argument without words, which
    /// would be about every command - only for a matcher that was never checked.
    fn matches(&self, argument: &str, value: Option<&Value>) -> Result<Coverage, RuleError> {
        let matched = match (self, value) {
            (_, None) => false,
            (ArgumentMatcher::Pattern(pattern), Some(value)) => {
                let regex = compile(argument, pattern)?;
                match value {
                    Value::String(text) => regex.is_match(text),
                    value => regex.is_match(&value.to_string()),
                }
            }
            (ArgumentMatcher::Shell { shell }, _) if shell.is_empty() => return Err(RuleError::ShellWords(argument.to_string())),
            (ArgumentMatcher::Shell { shell }, Some(Value::String(line))) => {
                let Ok(commands) = shell::simple_commands(line) else { return Ok(Coverage::Unjudgeable) };
                let beginning = commands.iter().filter(|words| shell::begins_with(words, shell)).count();
                return Ok(match beginning {
                    0 => Coverage::Nothing,
                    n if n == commands.len() => Coverage::Whole,
                    _ => Coverage::Part,
                });
            }
            (ArgumentMatcher::Shell { .. }, Some(_)) => return Ok(Coverage::Unjudgeable),
        };
        Ok(if matched { Coverage::Whole } else { Coverage::Nothing })
    }
}

/// `pattern` compiled to match a whole value.
fn compile(argument: &str, pattern: &str) -> Result<Regex, RuleError> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(RuleError::PatternTooLong(argument.to_string()));
    }
    RegexBuilder::new(&format!("^(?:{pattern})$"))
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

    /// What the rule decides of `call`, made in the channel `channel_id` of session `session_id`
    /// at `now`; `None` where it does not apply - outside its reach, expired, or not about the
    /// call (`ToolMatcher::matches`). A rule that allows applies to a call it is about as a whole;
    /// one that asks or denies also to one it is about in part - a simple command of a command
    /// line. Where a command line cannot be judged, a rule that allows does not apply, and one
    /// that asks or denies asks: it opens nothing, and refuses nothing unseen. A rule whose
    /// matcher cannot be matched restricts and never allows: a broken rule never opens anything.
    pub fn decision_for(&self, call: &ToolCall, session_id: SessionId, channel_id: ChannelId, now: DateTime<Utc>) -> Option<RuleDecision> {
        if !self.reach.covers(session_id, channel_id) || self.is_expired(now) {
            return None;
        }
        let coverage = match self.matcher.matches(call) {
            Ok(coverage) => coverage,
            Err(_) if self.decision == RuleDecision::Allow => return None,
            Err(_) => return Some(self.decision),
        };
        match (coverage, self.decision) {
            (Coverage::Nothing, _) => None,
            (Coverage::Whole, decision) => Some(decision),
            (_, RuleDecision::Allow) => None,
            (Coverage::Part, decision) => Some(decision),
            (Coverage::Unjudgeable, _) => Some(RuleDecision::Ask),
        }
    }

    /// The rule that decides `call`, made in the channel `channel_id` of
    /// session `session_id` at `now`, with what it decides of it
    /// (`decision_for`): of those in `rules` that apply, the one that decides
    /// the strongest (`Deny` before `Ask` before `Allow`), the first of them
    /// if several decide as strongly. `None` if none applies.
    pub fn decide<'r>(rules: impl IntoIterator<Item = &'r ApprovalRule>, call: &ToolCall, session_id: SessionId, channel_id: ChannelId, now: DateTime<Utc>) -> Option<(&'r ApprovalRule, RuleDecision)> {
        rules.into_iter()
            .filter_map(|rule| Some((rule, rule.decision_for(call, session_id, channel_id, now)?)))
            .reduce(|strongest, decided| if decided.1 > strongest.1 { decided } else { strongest })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::UserId;
    use serde_json::json;
    use chrono::Duration;

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
        ToolMatcher { tool: tool.to_string(), arguments: arguments.iter().map(|(a, p)| (a.to_string(), ArgumentMatcher::Pattern(p.to_string()))).collect() }
    }

    /// The calls of `tool` whose `command` is a command line of simple commands beginning with
    /// `words`.
    fn shell(tool: &str, words: &[&str]) -> ToolMatcher {
        let shell = ArgumentMatcher::Shell { shell: words.iter().map(|word| word.to_string()).collect() };
        ToolMatcher { tool: tool.to_string(), arguments: BTreeMap::from([("command".to_string(), shell)]) }
    }

    fn run(command: &str) -> ToolCall {
        call("run_command", json!({ "command": command }))
    }

    fn rule(matcher: ToolMatcher, decision: RuleDecision) -> ApprovalRule {
        ApprovalRule::new(matcher, decision, Reach::Everywhere, None).unwrap()
    }

    #[test]
    fn a_matcher_matches_the_tool_and_every_argument_it_names_as_a_whole() {
        let ls = matcher("shell", &[("command", "ls( .*)?")]);
        assert_eq!(ls.matches(&call("shell", json!({ "command": "ls -l", "cwd": "/" }))).unwrap(), Coverage::Whole, "other arguments do not matter");
        assert_eq!(ls.matches(&call("shell", json!({ "command": "ls" }))).unwrap(), Coverage::Whole);
        assert_eq!(ls.matches(&call("shell", json!({ "command": "rm -rf / ; ls" }))).unwrap(), Coverage::Nothing, "the whole value, not a part of it");
        assert_eq!(ls.matches(&call("shell", json!({ "command": "ls\nrm -rf /" }))).unwrap(), Coverage::Nothing, "nor a line of it");
        assert_eq!(ls.matches(&call("shell", json!({}))).unwrap(), Coverage::Nothing, "an argument the call lacks");
        assert_eq!(ls.matches(&call("shell", Value::Null)).unwrap(), Coverage::Nothing, "a call published without its content");
        assert_eq!(ls.matches(&call("other", json!({ "command": "ls" }))).unwrap(), Coverage::Nothing, "another tool");

        let note = matcher("note", &[("id", "[0-9]+")]);
        assert_eq!(note.matches(&call("note", json!({ "id": 42 }))).unwrap(), Coverage::Whole, "a number as its JSON");
        assert_eq!(ToolMatcher::everything().matches(&call("anything", json!({ "x": 1 }))).unwrap(), Coverage::Whole);
        assert_eq!(matcher("note", &[]).matches(&call("note", Value::Null)).unwrap(), Coverage::Whole, "a tool alone, whatever its arguments");
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
    fn a_shell_argument_matches_the_simple_commands_beginning_with_its_words() {
        let ls = shell("run_command", &["ls"]);
        let covered = |command| ls.matches(&run(command)).unwrap();
        assert_eq!(covered("ls"), Coverage::Whole);
        assert_eq!(covered("ls -la src"), Coverage::Whole);
        assert_eq!(covered("ls && ls src | ls"), Coverage::Whole, "every part");
        assert_eq!(covered("ls; rm x"), Coverage::Part);
        assert_eq!(covered("rm x"), Coverage::Nothing);
        assert_eq!(covered("lsof"), Coverage::Nothing, "a word, not a prefix of letters");
        assert_eq!(covered("ls $(rm x)"), Coverage::Unjudgeable);
        assert_eq!(ls.matches(&call("run_command", json!({ "command": 42 }))).unwrap(), Coverage::Unjudgeable, "no line");
        assert_eq!(ls.matches(&call("run_command", json!({}))).unwrap(), Coverage::Nothing, "an argument the call lacks");
        assert_eq!(ls.matches(&call("other", json!({ "command": "ls" }))).unwrap(), Coverage::Nothing, "another tool");

        let git_status = shell("run_command", &["git", "status"]);
        assert_eq!(git_status.matches(&run("git status -s")).unwrap(), Coverage::Whole);
        assert_eq!(git_status.matches(&run("git -C x status")).unwrap(), Coverage::Nothing, "word by word");
        assert_eq!(git_status.matches(&run("git")).unwrap(), Coverage::Nothing);
    }

    #[test]
    fn of_several_arguments_the_least_counts() {
        let mut in_main = shell("run_command", &["git"]);
        in_main.arguments.insert("cwd".to_string(), ArgumentMatcher::Pattern("main".to_string()));
        let covered = |command, cwd| in_main.matches(&call("run_command", json!({ "command": command, "cwd": cwd }))).unwrap();
        assert_eq!(covered("git status", "main"), Coverage::Whole);
        assert_eq!(covered("git status; rm x", "main"), Coverage::Part);
        assert_eq!(covered("git $(x)", "main"), Coverage::Unjudgeable);
        assert_eq!(covered("git $(x)", "other"), Coverage::Nothing, "an unjudgeable line does not make the other argument match");
        assert_eq!(covered("git status", "other"), Coverage::Nothing);
    }

    #[test]
    fn a_rule_on_commands_decides_by_how_much_of_the_line_it_is_about() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let now = Utc::now();
        let decided = |rule: &ApprovalRule, command| rule.decision_for(&run(command), session, channel, now);

        let allow_ls = rule(shell("run_command", &["ls"]), RuleDecision::Allow);
        assert_eq!(decided(&allow_ls, "ls -la"), Some(RuleDecision::Allow));
        assert_eq!(decided(&allow_ls, "ls && ls src"), Some(RuleDecision::Allow), "every part");
        for line in ["ls; rm x", "ls $(rm x)", "ls > f", "FOO=1 ls", "rm x"] {
            assert_eq!(decided(&allow_ls, line), None, "{line}");
        }

        let deny_rm = rule(shell("run_command", &["rm"]), RuleDecision::Deny);
        assert_eq!(decided(&deny_rm, "ls && rm x"), Some(RuleDecision::Deny), "a part");
        assert_eq!(decided(&deny_rm, "ls"), None);
        assert_eq!(decided(&deny_rm, "echo $HOME"), Some(RuleDecision::Ask), "what it cannot judge it asks for");
        let ask_rm = rule(shell("run_command", &["rm"]), RuleDecision::Ask);
        assert_eq!(decided(&ask_rm, "ls | rm x"), Some(RuleDecision::Ask));
        assert_eq!(decided(&ask_rm, "ls > f"), Some(RuleDecision::Ask));

        let deny_all = rule(matcher("run_command", &[]), RuleDecision::Deny);
        let decide = |rules: &[&ApprovalRule], command| ApprovalRule::decide(rules.iter().copied(), &run(command), session, channel, now).map(|(rule, decision)| (rule.id, decision));
        assert_eq!(decide(&[&deny_rm, &deny_all], "ls > f"), Some((deny_all.id, RuleDecision::Deny)), "by what a rule decides of the call");
        assert_eq!(decide(&[&allow_ls, &deny_rm], "ls > f"), Some((deny_rm.id, RuleDecision::Ask)));
        assert_eq!(decide(&[&allow_ls, &deny_rm], "ls -la"), Some((allow_ls.id, RuleDecision::Allow)));
    }

    #[test]
    fn a_shell_argument_that_cannot_be_matched_is_refused() {
        let command = || "command".to_string();
        assert!(shell("run_command", &["git", "status"]).check().is_ok());
        assert_eq!(shell("*", &["ls"]).check(), Err(RuleError::ShellOfEveryTool(command())));
        assert_eq!(shell("run_command", &[]).check(), Err(RuleError::ShellWords(command())));
        assert_eq!(shell("run_command", &["x"; MAX_SHELL_WORDS + 1]).check(), Err(RuleError::ShellWords(command())));
        assert_eq!(shell("run_command", &[&"x".repeat(MAX_WORD_CHARS + 1)]).check(), Err(RuleError::WordTooLong(command())));
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

        assert_eq!(ApprovalRule::decide([&allow, &ask], &ls, session, channel, now).map(|(r, _)| r.id), Some(ask.id), "ask before allow");
        assert_eq!(ApprovalRule::decide([&ask, &deny, &allow], &ls, session, channel, now).map(|(r, _)| r.id), Some(deny.id), "deny before ask");
        assert!(ApprovalRule::decide([&other], &ls, session, channel, now).is_none(), "about another tool");
        let first = rule(matcher("shell", &[]), RuleDecision::Ask);
        assert_eq!(ApprovalRule::decide([&first, &ask], &ls, session, channel, now).map(|(r, _)| r.id), Some(first.id), "the first of the strongest");
    }

    #[test]
    fn a_rule_applies_within_its_reach_until_it_expires() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let now = Utc::now();
        let ls = call("shell", json!({ "command": "ls" }));
        let within = |reach| ApprovalRule::new(matcher("shell", &[]), RuleDecision::Deny, reach, None).unwrap();
        assert!(within(Reach::Session(session)).decision_for(&ls, session, channel, now).is_some());
        assert!(within(Reach::Session(SessionId::new())).decision_for(&ls, session, channel, now).is_none(), "another session");
        assert!(within(Reach::Channel(channel)).decision_for(&ls, session, channel, now).is_some());
        assert!(within(Reach::Channel(ChannelId::new())).decision_for(&ls, session, channel, now).is_none(), "another channel");

        let until = |at| ApprovalRule::new(matcher("shell", &[]), RuleDecision::Deny, Reach::Everywhere, Some(at)).unwrap();
        assert!(until(now + Duration::minutes(1)).decision_for(&ls, session, channel, now).is_some());
        assert!(until(now).decision_for(&ls, session, channel, now).is_none(), "expired");
    }

    #[test]
    fn a_broken_rule_restricts_and_never_allows() {
        let (session, channel) = (SessionId::new(), ChannelId::new());
        let ls = call("shell", json!({ "command": "ls" }));
        // As it could be read from a document written by hand: never checked.
        let broken = |matcher, decision| ApprovalRule { id: ApprovalRuleId::new(), matcher, decision, reach: Reach::Everywhere, expires_at: None };
        for unmatchable in [matcher("shell", &[("command", "(")]), shell("shell", &[])] {
            assert_eq!(broken(unmatchable.clone(), RuleDecision::Deny).decision_for(&ls, session, channel, Utc::now()), Some(RuleDecision::Deny), "{unmatchable:?}");
            assert_eq!(broken(unmatchable.clone(), RuleDecision::Allow).decision_for(&ls, session, channel, Utc::now()), None, "{unmatchable:?}");
            assert_eq!(broken(unmatchable, RuleDecision::Deny).decision_for(&call("note", json!({})), session, channel, Utc::now()), None, "only for its tool");
        }
    }

    #[test]
    fn a_call_proposes_the_rule_for_the_calls_exactly_like_it() {
        let ls = call("shell", json!({ "command": "ls -l *.rs", "depth": 2 }));
        let exact = ToolMatcher::exact(&ls);
        assert!(exact.check().is_ok());
        assert_eq!(exact.matches(&ls).unwrap(), Coverage::Whole, "itself");
        assert_eq!(exact.matches(&call("shell", json!({ "command": "ls -l main.rs", "depth": 2 }))).unwrap(), Coverage::Nothing, "a `*` is no wildcard in it");
        assert_eq!(exact.matches(&call("shell", json!({ "command": "ls -l *.rs", "depth": 3 }))).unwrap(), Coverage::Nothing, "every argument counts");
        assert_eq!(ToolMatcher::exact(&call("stop", Value::Null)), matcher("stop", &[]), "a call without arguments: its tool");
        assert!(ToolMatcher::everything().is_everything() && !exact.is_everything());
    }

    #[test]
    fn a_command_line_proposes_the_rule_for_the_commands_beginning_as_it_begins() {
        let parameters = ToolParameters::object([("command", ToolParameters::shell("a line")), ("cwd", ToolParameters::string("where"))], ["command"]);
        let running = |command: &str| call("run_command", json!({ "command": command, "cwd": "main" }));
        let proposed = |command: &str| ToolMatcher::proposed(&running(command), &parameters);
        let shell = |words: &[&str]| ArgumentMatcher::Shell { shell: words.iter().map(|word| word.to_string()).collect() };

        let ls = proposed("ls -la src");
        assert_eq!(ls.arguments["command"], shell(&["ls", "-la"]));
        assert_eq!(ls.arguments["cwd"], ArgumentMatcher::Pattern("main".to_string()), "the others exactly");
        assert!(ls.check().is_ok());
        assert_eq!(ls.matches(&running("ls -la src")).unwrap(), Coverage::Whole, "itself");
        assert_eq!(proposed("ls").arguments["command"], shell(&["ls"]));
        assert_eq!(proposed("ls *.rs -l").arguments["command"], shell(&["ls"]), "up to a word that expands");
        assert_eq!(proposed("'my tool' x y").arguments["command"], shell(&["my tool", "x"]), "words as they run");
        for line in ["ls && rm x", "ls $(rm x)"] {
            assert_eq!(proposed(line).arguments["command"], ArgumentMatcher::Pattern(regex::escape(line)), "{line}: that line alone");
        }
        let plain = ToolParameters::object([("command", ToolParameters::string("plain"))], ["command"]);
        assert_eq!(ToolMatcher::proposed(&running("ls"), &plain), ToolMatcher::exact(&running("ls")), "a string in no language");
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

    #[test]
    fn an_argument_is_written_as_a_pattern_or_as_its_language() {
        let written = json!({ "tool": "run_command", "arguments": { "command": { "shell": ["git", "status"] }, "cwd": "main" } });
        let matcher: ToolMatcher = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(matcher.arguments["command"], ArgumentMatcher::Shell { shell: vec!["git".to_string(), "status".to_string()] });
        assert_eq!(matcher.arguments["cwd"], ArgumentMatcher::Pattern("main".to_string()));
        assert_eq!(serde_json::to_value(&matcher).unwrap(), written);
        let misspelt = json!({ "tool": "run_command", "arguments": { "command": { "shell": ["git"], "shel": ["x"] } } });
        assert!(serde_json::from_value::<ToolMatcher>(misspelt).is_err(), "a misspelt key");
    }
}
