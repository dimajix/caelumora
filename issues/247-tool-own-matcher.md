# 247 — A rule can match an argument that is a language only by its exact text

**Status:** open
**Area:** Tools & approvals

## The problem

A rule on tool calls (issue [13](13-approval-modes.md)) says which calls it is about with a
`ToolMatcher`: the tool's name and a regex per argument, matched against the whole value. For
structured arguments - a channel, an id, a domain - that is reliable. For an argument that is a
language of its own - a shell command, an SQL statement - it is not: a pattern that looks like "the
command starts with `ls`" also matches `ls; rm -rf ~` unless it is written with care few will take,
and the approval card proposes the call itself (`ToolMatcher::exact`) unless the tool proposes
something better (`ToolProvider::matcher`). No tool does so far, because no tool has such an
argument yet.

## Why it matters

The first tool that runs shell commands or queries a database is the one where "and from now on" is
used most - and where a rule that matches more than the person meant does the most harm. Claude
Code's prefix matching of shell commands was bypassed by chaining and substitution often enough
that it checks for injection on top.

## Considerations

- **The tool proposes.** `ToolProvider::matcher(name, params)` exists for this, with
  `ToolMatcher::exact` as the default; a shell tool would propose the command's prefix, a reading
  tool the path. The proposal is what the card prefills; the person can still widen it.
- **A matcher of another kind.** `ToolMatcher` is a struct today, stored as data; a kind that
  understands the language - a parsed command with its program and arguments, refusing chains,
  substitutions and redirections - would be another variant. Issue 13 left the room for it: the
  struct can become an untagged enum without changing what is stored.
- **Fail closed.** A command the matcher cannot parse, or one that chains several, matches no rule
  that allows: it asks.

## Done when

The first tool with an argument that is a language proposes a matcher of its own, and a rule of that
kind matches a call only where the person's intent and the call agree - a chained or substituted
command is not matched by a rule for its first part.

## History

- **2026-10-06** — Opened when issue [13](13-approval-modes.md) was closed: its step 3, waiting for
  a tool with such an argument.
