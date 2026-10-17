# 400 — A rule derived from a call with a long argument cannot be kept

**Status:** open
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

## Done when

The rule an approval card offers for any call can be kept, and is never wider than that call.

## History

- **2026-10-17** — Opened while issue [247](247-tool-own-matcher.md) was worked on: its proposals
  keep a chain of commands exact, which made the bound visible.
