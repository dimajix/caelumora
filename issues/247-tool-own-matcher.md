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

## Open questions

Each with the option recommended first.

1. **A chain of commands** (`a && b`, `a; b`, `a | b`):
   - **(a) Split into simple commands, and let the decision say how many must match.** A rule that
     allows applies if *every* part matches it; one that asks or denies, if *any* part does. A rule
     for `git` allows `git status && git diff`; one that denies `rm` catches `ls && rm x`. This
     narrows *Fail closed* above, which counted every chain as unparsed: a chain is judged where
     each of its parts can be.
   - (b) A chain is never judged: it matches no rule that allows and every rule that asks or
     denies - the fallback `ApprovalRule::applies` already has for a matcher that fails. Simpler,
     but a rule that denies `rm` would deny every chain, whatever it runs.
2. **What the approval card proposes** for `run_command`:
   - **(a) The program and its first word that is no option** - `cargo test` of
     `cargo test -p x --release`, `ls src` of `ls -la src`. The person shortens or lengthens it on
     the card.
   - (b) The program alone - wide, easily wider than meant.
   - (c) Every word - hardly wider than `ToolMatcher::exact`.
3. **The parser:**
   - **(a) A small scanner of our own** that knows quoting, escapes and the separators of 1, and
     calls everything else it meets unjudgeable. It fails closed by construction, and what it
     understands is what this issue needs.
   - (b) A crate - `shell-words` splits into words but knows no chains or substitutions; a full
     grammar (`conch-parser`, tree-sitter's bash) understands more than we would act on, and
     everything it understands must then be judged.
4. **The shape of the matcher:**
   - **(a) `ToolMatcher` becomes an enum**, untagged: today's struct as one variant, the command
     as another. Stored rules and configured ones keep their form, nothing is to be deleted.
   - (b) An optional `command` field on today's struct - one type, but a matcher with both
     argument patterns and a command whose meaning nobody asked for.

## Decisions

Taken on 2026-10-08 by the maintainer, answering the questions above: the recommended option of
each.

1. **Chains** are split into simple commands; a rule that allows applies if every part matches it,
   one that asks or denies if any part does.
2. **The card proposes** the program and its first word that is no option.
3. **The parser** is a small scanner of our own that calls whatever it does not know unjudgeable.
4. **`ToolMatcher` becomes an untagged enum**, today's form one of its variants. Should a
   configured rule's error read too vaguely, a `Deserialize` of its own that tells the variants
   apart by their fields is the remedy.

## Plan

Names are proposals; what is fixed is where a responsibility lies.

### A. The command line, in the model

The rules are the model's (`caelumora-model`), so is reading a command line for them:
`approval.rs` becomes `approval/mod.rs`, and `approval/command.rs` holds the scanner.

- `CommandLine::parse(text)` returns the simple commands of a line, each as its words after
  quoting and escapes are removed - or `Unjudgeable`.
- **Separators** split it into simple commands: `;`, `&&`, `||`, `|` and a line break.
- **Unjudgeable** is everything that makes the words that run differ from the words written, or
  that acts besides running them: `$` and backticks (substitution, expansion), `<` and `>`
  (redirection, also `2>&1`), `&` alone (a background job), `(`, `)`, `{`, `}` (subshells and
  groups), a leading `NAME=value`, an unclosed quote, a trailing backslash, a line longer than a
  limit. A glob (`*`, `?`, `[`) or a `~` unquoted is fine in a word no rule compares, and makes a
  word that is compared unequal.
- **What it does not see**, said in the docs rather than solved: a program reached by another
  path (`/bin/rm`) or through another (`env rm`, `xargs rm`, `sh -c "rm"`, `find -exec rm`). A
  rule that allows is safe against all of them - the words must start with what it names; a rule
  that denies a program is a hint, not a boundary. The boundary is where the command runs, the
  workspace's storage (issue [251](251-open-terminal.md)): a `local` shell reaches what the
  server's user reaches, a container what is in it.

### B. The matcher

- `ToolMatcher` gets a second variant, `Command { tool, argument, prefix }`: the tool, the
  argument that holds the command line, and the words a simple command must begin with.
  `check` requires a named tool (not `*`), an argument, and a prefix of at least one word, with
  limits on their number and length.
- `matches` reports, for a command, whether no part, some or every part of the line begins with
  the prefix - or that the line is unjudgeable. `ApprovalRule::applies` turns that into an answer
  by the rule's decision (open question 1a); an unjudgeable line takes the fallback that exists for
  a matcher that fails: a rule that allows does not apply, one that asks or denies does.
- Today's variant answers as before: every part, or none.

### C. `run_command` proposes it

`TerminalTool::matcher` proposes, for `run_command`, a `Command` matcher of the call's first two
words (open question 2a) when the line is one simple command; for a chain or an unjudgeable line
it keeps `ToolMatcher::exact`, a rule for that line alone.

### D. REST and the web UI

- The REST API carries `ToolMatcher` as it is; the TypeScript types gain the variant.
- `describeMatcher` shows a command matcher as the command it is about (`run_command: git status
  …`).
- `ApprovalRuleOffer` offers, for a command matcher, the words of the call to choose how many of
  them the rule is about; `ApprovalRulesDialog`, where a place's rules are written, can write one
  too. The profile lists it as it lists the others.

### E. Tests

- The scanner, as a table: lines and the simple commands or `Unjudgeable` they give.
- The matcher and `applies`, by decision: a rule that allows `ls` allows `ls -la` and
  `ls && ls src`, not `ls; rm x`, `ls $(rm x)`, `ls > f`, `FOO=1 ls`; one that denies `rm`
  denies `ls && rm x` and every unjudgeable line.
- The security suite: a person's rule derived from an approved `ls` lets the next `ls -la` pass and
  asks again for `ls; rm -rf ~`; a place's rule that asks for a command asks for an unjudgeable
  line.

### F. Docs

TOOLS-AND-COMMANDS.md (rules and what `run_command` proposes), CONFIGURATION.md (a command matcher
in `approvals.rules`; the example there denies `rm .*` with a pattern, which `ls; rm x` passes),
SECURITY.md (what a rule on commands does not bound).

### G. Steps

1. `approval.rs` to `approval/mod.rs`, without changes.
2. A and B in the model, with their tests.
3. C, with the security suite.
4. D.
5. F, and the issue closed.

## Done when

The first tool with an argument that is a language proposes a matcher of its own, and a rule of that
kind matches a call only where the person's intent and the call agree - a chained or substituted
command is not matched by a rule for its first part.

## History

- **2026-10-06** — Opened when issue [13](13-approval-modes.md) was closed: its step 3, waiting for
  a tool with such an argument.
- **2026-10-08** — Its first tool exists: `run_command` of `terminal_tool` (issue
  [251](251-open-terminal.md)), whose `command` is a shell command line. It proposes
  `ToolMatcher::exact` today, so a rule derived from an approved command covers that command
  alone. The file tools of the same component propose a matcher of their own already - a change of
  a file by its path alone (`ToolProvider::matcher`) - the structured case this issue names as
  reliable. Next is the shell matcher for `run_command`.
- **2026-10-08** — A plan, with the maintainer: four open questions with a recommendation each,
  and the steps from the scanner in the model to the web UI.
- **2026-10-08** — The open questions decided: the recommended option of each.
