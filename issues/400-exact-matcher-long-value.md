# 400 — A rule derived from a call with a long argument cannot be kept

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** Tools and approvals
**Summary:** `ToolMatcher::exact` escapes every argument's whole value into a pattern, which for a value longer than `MAX_PATTERN_CHARS` fails the check, so the rule an approval card offers for such a call is refused - only after the call was answered.

## The problem

A rule derived from a call (issue [13](13-approval-modes.md)) is offered with the matcher the call's
tool proposes; unless the tool knows better, that is `ToolMatcher::exact`: every argument as
`regex::escape` of its whole value. `ToolMatcher::check` refuses a pattern longer than
`MAX_PATTERN_CHARS` (500 characters), and escaping only lengthens a value. So for a call with an
argument of more than about 500 characters - a text an MCP tool is given, a chain of shell commands,
which `run_command` keeps exact (issue [247](247-tool-own-matcher.md)) - the card offers a rule the
server will not keep.

The person notices late: the card prefills the long pattern, they answer the call with "and from
now on", the answer is sent, and adding the rule fails with `The call was answered, but the rule
could not be added` and the pattern's length as the reason (`ChatView`, `UserController`).

## Why it matters

The calls with long arguments are those a person least wants to read twice; the rule fails exactly
where it would save the most. And a proposal the server refuses is the core offering something it
does not accept - the card has no way to tell beforehand.

## Considerations

- **Leaving the argument out** of the proposal would make the rule wider than the call the person
  saw - for a rule that allows, an opening nobody chose. Not an option.
- **A comparison by equality.** An `ArgumentMatcher` variant that compares the whole value as it
  is, without a regular expression, needs no bound against a costly automaton - but a stored rule
  would hold the value, and a person keeps up to `MAX_PERSONAL_RULES` of them.
- **A comparison by digest.** The same, holding the value's digest (`{ "sha256": "…" }`): bounded
  in size whatever the value, but not readable or editable on the card - which a 500-character
  pattern hardly is either.
- **No rule offered** where the proposal cannot be kept: `suggested` left out, or the card told so.
  The least change, but the function is missing where it helps most.

A first recommendation: `exact` keeps the escaped pattern for a value that fits, and compares a
longer one by its digest; `describeMatcher` and the card show such an argument as "exactly this
value". To be decided before it is built.

## Decision

Taken on 2026-10-17 by the maintainer: a comparison by equality for a long value, and no rule
offered beyond its bound. Looked at closer, two things changed the recommendation above:

- **A digest is unreadable.** A person could no longer read in their profile what they allowed -
  the wrong signal for a rule that allows.
- **No proposal is not yet "no rule".** Without `suggested`, the card offers every call of the
  tool (`initialChoice`): a fallback from before there were proposals, wider than the call. It goes;
  stored data may be deleted.

Raising `MAX_PATTERN_CHARS` was weighed and not taken: every pattern is compiled anew at every
evaluation, for every rule, and hand-written patterns would grow as well.

## Plan

### A. The model

- `ArgumentMatcher` gets a third variant, `Equals { equals: String }`: the whole value as it is - a
  string as itself, any other value as its compact JSON -, compared without a regular expression.
  `check` bounds it by `MAX_EQUALS_CHARS` (10 000 characters, as a command line).
- `ToolMatcher::exact` keeps the escaped pattern for a value whose pattern fits
  `MAX_PATTERN_CHARS` - editable on the card - and takes `Equals` for a longer one.
- `ToolCallStatus::WaitingApproval::suggested` becomes an `Option`: none where no rule can be
  derived.

### B. The agents

`ToolProviders::matcher` offers a tool's proposal only where it passes `ToolMatcher::check`,
whoever made it: `Admission::Ask` carries `None` otherwise.

### C. The web UI

- The types gain `{equals}`; `describeMatcher` shows it as `name = "…"`, shortened.
- The card offers "and from now on" only with a proposal; it shows an `equals` argument as "exactly
  this value", which the person keeps or drops ("any").
- `ApprovalRulesDialog` compares an argument also as "is exactly".

### D. Tests and docs

- The model: `exact` of a long value, `Equals` matched and bounded, its form.
- The agents: a call whose proposal cannot be kept is asked without one.
- APPROVALS.md (the third form, the proposal of a long value), CONFIGURATION.md, WEB-UI.md.

## Done when

The rule an approval card offers for any call can be kept, and is never wider than that call.

## History

- **2026-10-17** — Opened while issue [247](247-tool-own-matcher.md) was worked on: its proposals
  keep a chain of commands exact, which made the bound visible.
- **2026-10-17** — Decided: equality for a long value, no rule beyond its bound, the card's
  fallback to every call of the tool removed. A plan in four parts.
- **2026-10-17** — Done as planned. `ArgumentMatcher::Equals`, bounded by `MAX_EQUALS_CHARS`;
  `ToolMatcher::exact` takes it where the escaped pattern would exceed `MAX_PATTERN_CHARS`;
  `ToolProviders::matcher` offers only a proposal that passes `check`, and the waiting call's
  `suggested` is an `Option`. The card offers a rule only with a proposal and shows a value
  compared by equality as "exactly this value"; the dialog of a place's rules writes one as "is
  exactly". Stored data: a turn waiting for an approval keeps its proposal as before; nothing has
  to be deleted.
