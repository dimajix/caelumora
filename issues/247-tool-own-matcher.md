# 247 — A rule can match an argument that is a language only by its exact text

**Status:** done
**Type:** bug
**Priority:** high
**Area:** Tools and approvals
**Summary:** A rule on tool calls can match an argument that is a language - a shell command, SQL - only by its text, which a chained command slips past.

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

Taken on 2026-10-17 by the maintainer, after the plan was checked against the code and against how
Claude Code matches shell commands (https://code.claude.com/docs/en/permissions): the recommended
option of each. Decision 5 replaces 4, decision 6 corrects 2.

5. **The kind of comparison is the argument's, not the matcher's.** `ToolMatcher` stays the struct
   it is; the value of each argument becomes an untagged enum, `ArgumentMatcher`: a string is
   today's pattern, an object names the language its argument is in
   (`{ "shell": ["git", "status"] }`). A rule can so name a structured argument and one that is a
   language together - an SQL tool's `database` and `query` -, which a kind of the whole matcher
   could not: its command variant knew one argument and repeated what the matcher does for the
   tool. Stored rules keep their form; the worry of decision 4 about vague errors is gone, since a
   string and an object are told apart by their form. Claude Code goes half this way: a tool's
   main argument by a syntax of the tool's own, the others only in rules that ask or deny, and
   only by their value.
6. **The card proposes the first words as they are written**: `cargo test` of `cargo test -p x`,
   `ls -la` of `ls -la src`. Decision 2 took the program and its first word that is no option -
   `ls src`, which a comparison of the first words does not match even against the call it was
   derived from. The person shortens the proposal on the card. Claude Code saves `npm test *` the
   same way.
7. **bash's reserved words make a line unjudgeable** where a simple command begins with one:
   split at its `;`, `while true; do rm -rf x; done` gives `do rm -rf x`, which a rule denying
   `rm` would not see - bash's grammar, not another program reaching `rm`. So do a leading
   `NAME+=value` and `|&`.
8. **A rule that denies asks for an unjudgeable line** instead of denying it. Denying would refuse
   every line with a `$` or a `>` once any rule denies a command - the effect for which option 1b
   was rejected; asking opens nothing either. A rule that allows does not apply to such a line,
   one that asks does, as before.

Taken on 2026-10-17 by the maintainer as well:

9. **A tool says which of its arguments are in a language**: `ToolParameters::String` gets an
   optional `language` (`shell`, later perhaps `sql`) beside its description - not `format`,
   which means something else in JSON Schema and which some model providers refuse unknown values
   of; it is the same word as the key of `ArgumentMatcher`. What the model is told does not
   change. The language serves where the tool is known: the proposal for a rule is derived from
   it (`ToolProvider::matcher`), and the web UI offers a shell argument where it knows the tool.
   It is not carried to where rules are judged: a pattern on an argument in a language, written
   by hand, stays as unreliable as today, and the docs say so. Weighed and not taken: the agent
   stamping the languages onto the `ToolCall`, as it stamps `visibility` and `presentation`, so
   that `applies` could keep a pattern on such an argument from allowing - more than this issue
   needs, for a rule nobody is offered.

## Plan

Names are proposals; what is fixed is where a responsibility lies.

### A. The command line, in the model

The rules are the model's (`caelumora-model`), so is reading a command line for them:
`approval.rs` becomes `approval/mod.rs`, and `approval/shell.rs` holds the scanner.

- `CommandLine::parse(text)` returns the simple commands of a line, each as its words after
  quoting and escapes are removed - or `Unjudgeable`.
- **Separators** split it into simple commands: `;`, `&&`, `||`, `|` and a line break.
- **Unjudgeable** is everything that makes the words that run differ from the words written, or
  that acts besides running them: `$` and backticks (substitution, expansion), `<` and `>`
  (redirection, also `2>&1`), `&` alone and `|&`, `(`, `)`, `{`, `}` (subshells and groups), a
  leading `NAME=value` or `NAME+=value`, a simple command beginning with one of bash's reserved
  words (`if`, `then`, `else`, `elif`, `fi`, `for`, `while`, `until`, `do`, `done`, `case`, `esac`,
  `select`, `function`, `time`, `!`, `[[`), an unclosed quote, a trailing backslash, a line longer
  than a limit. A glob (`*`, `?`, `[`) or a `~` unquoted is fine in a word no rule compares, and
  makes a word that is compared unequal.
- **What it does not see**, said in the docs rather than solved: a program reached by another
  path (`/bin/rm`) or through another (`env rm`, `xargs rm`, `sh -c "rm"`, `find -exec rm`). A
  rule that allows is safe against all of them - the words must start with what it names; a rule
  that denies a program is a hint, not a boundary. Neither is a rule that allows narrower than it
  reads: GNU programs take options after their operands, so one for `rm x` allows `rm x -rf /`.
  The boundary is where the command runs, the workspace's storage (issue
  [251](251-open-terminal.md)): a `local` shell reaches what the server's user reaches, a
  container what is in it.

### B. The matcher

- `ToolMatcher` keeps its form; its `arguments` map each name to an `ArgumentMatcher`, untagged:
  `Pattern(String)`, today's regular expression, and `Shell { shell: Vec<String> }`, the words a
  simple command of the line must begin with. `check` requires, for a shell argument, a named
  tool (not `*`) and at least one word, with limits on their number and length.
- Each argument answers whether **every** part of its value matches and whether **some** part
  does: a pattern both or neither; a shell line by its simple commands; an unjudgeable line
  neither every nor some, but unjudgeable. `ToolMatcher::matches` joins its arguments - every
  argument for every, every argument for some, and unjudgeable where one is and the others
  match at least in part.
- `ApprovalRule::applies` turns that into the decision the rule takes of the call, or none:
  a rule that allows applies where every part matches, one that asks or denies where some part
  does (decision 1); of an unjudgeable call a rule that allows takes none, one that asks asks, and
  one that denies asks (decision 8). `decide` compares rules by what they take of the call, not
  by what they are written to decide; its callers learn the rule and that decision.
- A pattern that cannot be matched keeps the fallback it has: a rule that allows does not apply,
  one that asks or denies does.

### C. A tool proposes it by its parameters

- `ToolParameters::String` gets its `language` (decision 9); `run_command` declares `command` as
  `shell`.
- The default of `ToolProvider::matcher` derives the proposal from the tool's parameters,
  argument by argument: one in `shell` as a shell argument of its first two words as written
  (decision 6) when the line is one simple command, every other as `ToolMatcher::exact` does. For
  a chain or an unjudgeable line the argument keeps its exact pattern, a rule for that line alone.
- `TerminalTool::matcher` keeps its proposal for the files and, for `run_command`, leaves out
  `wait_secs`, which the derived proposal would have taken in.

### D. REST and the web UI

- The REST API carries `ToolMatcher` as it is; the TypeScript types gain `ArgumentMatcher`.
- `describeMatcher` shows each argument by its kind: a pattern as today, a shell argument as the
  command it is about (`run_command (command: git status …)`).
- `ApprovalRuleOffer` offers, for a shell argument, the words of the call to choose how many of
  them the rule is about; `ApprovalRulesDialog`, where a place's rules are written, can write one
  too, for an argument whose tool it knows to declare it `shell`. The profile lists it as it lists
  the others.

### E. Tests

- The proposal derived from a tool's parameters: a shell argument of a simple command, the exact
  pattern of a chain, the other arguments exact.
- The scanner, as a table: lines and the simple commands or `Unjudgeable` they give, reserved
  words among them.
- The matcher and `applies`, by decision: a rule that allows `ls` allows `ls -la` and
  `ls && ls src`, not `ls; rm x`, `ls $(rm x)`, `ls > f`, `FOO=1 ls`; one that denies `rm` denies
  `ls && rm x`, asks for an unjudgeable line, and leaves `ls` alone; a shell argument together
  with a pattern on another argument, of which an unjudgeable line does not make the other match.
- The security suite: a person's rule derived from an approved `ls` lets the next `ls -la` pass and
  asks again for `ls; rm -rf ~`; a place's rule that denies a command asks for an unjudgeable line.

### F. Docs

APPROVALS.md (rules with shell arguments, how a chain and an unjudgeable line are judged, what a
rule on commands does not bound, that a pattern on a shell argument is not judged as a command
line), TOOLS.md (the `language` of a parameter, the proposal derived from it, what `run_command`
proposes), CONFIGURATION.md (a shell argument in `approvals.rules`), SECURITY.md where it speaks
of what bounds a command.

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
- **2026-10-17** — The plan checked against the code and against how Claude Code matches shell
  commands; four further decisions (5 to 8): the kind of comparison per argument instead of per
  matcher, the first words as written proposed, bash's reserved words unjudgeable, and a rule that
  denies asks for an unjudgeable line. The plan follows them; its docs are those of today's
  `docs/`.
- **2026-10-17** — Decision 9: a tool declares the language of an argument in its parameters, for
  the proposal and the web UI; it is not carried to where rules are judged.
- **2026-10-17** — Steps 1 and 2: `approval.rs` is `approval/mod.rs`; `approval/shell.rs` reads a
  command line into its simple commands, `ArgumentMatcher` compares an argument as a pattern or as
  a shell line, `ToolMatcher::matches` answers a `Coverage`, and `ApprovalRule::decision_for`
  takes the place of `applies`, answering what a rule decides of a call. `ApprovalRule::decide`
  returns the rule with that decision; the `ToolExecutor` and the `UserActor` act on it - a
  person's rule that only asks answers nothing. Besides the reserved words of decision 7, a
  simple command whose first word expands is unjudgeable, so is `coproc`.
- **2026-10-17** — Step 3: `ToolParameters::String` has its `language` (`ArgumentLanguage`,
  `ToolParameters::shell`), which `run_command` declares for `command`. `ToolMatcher::proposed`
  derives the proposal from a tool's parameters; the default of `ToolProvider::matcher` uses it
  (`proposed_matcher`), and `TerminalTool` leaves out `wait_secs`. A word that expands ends the
  proposal before it, since the call itself would not match it. The agents' security suite has a
  tool of its own whose argument is a shell line, a person's rule derived from an approved `ls`,
  and a rule that denies a command asking for an unjudgeable line - of a place, and of a person,
  whose rule then does not answer.
- **2026-10-17** — Steps 4 and 5, and done. The web UI knows `ArgumentMatcher`: `describeMatcher`
  shows a shell argument by its words, the approval card offers how many of the proposed words the
  rule keeps, and the dialog of a place's rules compares each argument as a pattern or by the
  words a command begins with. Two narrowings of the plan: the card shortens the proposal but
  cannot lengthen it, since which words a line has is the server's to read and the UI does not
  mirror the scanner; and the dialog offers both kinds for every argument, since it does not know
  the tools of a place's agents. APPROVALS.md has a section on rules on commands - how a chain and
  an unjudgeable line are judged, and what such a rule does not bound -, TOOLS.md, CONFIGURATION.md
  and WEB-UI.md say what changed; SECURITY.md has no passage on what bounds a command, so nothing
  was added there.
