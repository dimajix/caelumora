# 313 — The documents in `docs/` retell the code instead of explaining it

**Status:** open
**Area:** Maintenance

## The problem

The 17 documents in `docs/` have about 6900 lines and 520 KB, about 130,000 tokens. The largest are
`AGENT-TURN.md` (914 lines, 64 KB), `CHANNELS.md` (879, 63 KB), `TOOLS-AND-COMMANDS.md` (828,
63 KB), `WEB-UI.md` (618, 48 KB), `CONFIDENTIALITY.md`, `SECURITY.md` and `STORAGE.md` (500 to 580
lines each). An agent that changes a turn reads `CODING-GUIDELINES.md`, `ARCHITECTURE.md` and
`AGENT-TURN.md` first, as AGENTS.md asks: about 95 KB, some 24,000 tokens, before it has looked at
any code.

What makes them long is less the concepts than four other kinds of content:

- **Code retold.** Every variant of an enum with what it carries, the fields of a type, which
  method calls which, the names of constants and in which order things happen inside a method.
  `AGENT-TURN.md` lists every entry of `Transcript` and `MessageHistory` with the issue that added
  it; `CHANNELS.md` describes `ChannelLoop::stop_actors`, its `TaskTracker` and `STOP_GRACE`. This
  is what the code and its doc comments say already, and it has to be changed with every change.
- **History.** About 650 references to issues, mostly inline - "(issue 187)", "issue 263,
  recorded by the channel only" - and phrases such as "until issue 286", "no longer", "as of". A
  document read to understand how things are tells how they came to be.
- **Density.** Long paragraphs under few headings (`CHANNELS.md`: 879 lines, 29 headings), so a
  reader looking for one rule reads the section around it.
- **Snapshots.** `PERMISSION-CHECKS.md` is a map of every place that asks a permission question
  "as of 2026-10-11", made for the review of issue [304](304-review-of-the-permission-concept.md).
  It is an inventory of code, not a description of a concept.

The names of types and methods themselves are not the problem. A rough check found the methods and
fields named as `Type::name` in `docs/` still in the code: the documents are kept up to date, and
keeping them so is part of what they cost.

The documents grow by the workflow: step 4 of AGENTS.md asks every change to update the document on
its subject, and nothing says what does not belong there. So every feature adds a paragraph about
how it was built.

## Why it matters

- Tokens: every task pays for the whole document on its subject, most of which it does not need.
- Attention: the rules that matter - stored first, then announced; one writer per document; a turn's
  error ends the turn - are between details, and a reader can miss them.
- Maintenance: each detail retold is a second place to change and a chance to contradict the code
  (issue [110](110-docs-contradicting-code.md) found such contradictions once).

## Considerations

**What a document in `docs/` should hold.** Proposed rule:

- concepts and their responsibilities - what a session, a channel, a turn is, and who owns what;
- principles and rules - invariants, orders, who checks what, what is never done and why;
- the architecture - how the parts fit together, the flow of a message, the boundaries of crates;
- names of types and methods as anchors, where they help to find the code: a name, not a retelling.

What does not belong there:

- what a single type or method does in detail: its doc comment, or the `//!` docs of its module, so
  it is changed with the code and reviewed with it;
- how something came to be: the issue. A document links an issue only where a decision would
  otherwise look wrong and the issue says why;
- inventories of code as of a date: they belong to the issue or the review that made them, or are
  generated.

**Whether code references are useful.** Yes, as anchors: an agent can find the code from a name in
one search, and a concept without a name to look for has to be found by reading. No, as a retelling:
an agent that changes the code reads the code anyway, and a description of it in a second place only
costs tokens and goes out of date. So: keep names, drop what is said about them beyond the concept.

**Ways to get there.**

- **Rework each document** by the rule above, moving the details that are worth keeping into doc
  comments and module docs, and the history into the issues where it is already recorded.
  Recommended. Costs a careful pass over 6900 lines: what is only written in a document - a rule
  nobody wrote into the code - must not be lost, and what moves to a doc comment must arrive there.
- **Split each document** into a short concept part and a reference part read only on demand.
  Keeps everything, but the reference part still has to be maintained and goes out of date.
- **Only trim**: remove the issue references and the history phrasing. Cheap, saves perhaps a
  fifth, leaves the retold code.

**`PERMISSION-CHECKS.md`**: either moved to issue 304 as the result of its review, with the security
suite as the place that keeps what each action asks, or kept as the one inventory, generated or
checked by a test. To be decided with 304.

**Order.** One document per commit, the largest first, so each can be reviewed against the code.
Before that, the rule is written into `CODING-GUIDELINES.md` and step 4 of AGENTS.md is changed to
point to it, so the documents do not grow back while they are reworked.

## Done when

- `CODING-GUIDELINES.md` says what belongs in `docs/`, in doc comments and in issues, and AGENTS.md
  points to it.
- Every document in `docs/` has been reworked by that rule, and its size before and after is
  recorded under History.
- No rule was lost on the way: what a document said and the code does not show is still in a
  document or a doc comment.

## History

- **2026-10-11** — Opened.
