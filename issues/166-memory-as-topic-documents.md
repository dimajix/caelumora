# 166 — Agent memory is a flat list of short entries, all of it in every prompt

**Status:** open
**Area:** Context & knowledge

## The problem

Agent memory ([62](62-json-store-concurrent-writes.md),
[68](68-notes-in-agent-state.md)) is a ledger of short texts: `remember(text)`
adds one, `forget(id)` removes one, and the prompt part lists every entry
that may be recalled in the channel, in full, in every turn. That shape has
three consequences:

- **No room for detail.** An entry is a line. What an agent learns about a
  subject - a user's preferences, how a project is set up, what was decided
  and why - is either squeezed into one line or spread over many unrelated
  ones that nothing groups.
- **No revising.** An entry cannot be changed, only forgotten and written
  anew. Knowledge that grows over time is a trail of half-outdated lines,
  and keeping it tidy is left to the model ([55](55-notes-by-reflection.md)).
- **Everything, always.** The prompt part grows with every entry and is paid
  for in every turn, whether the turn needs it or not - the problem of
  [58](58-notes-in-every-prompt.md), for memory.

Wanted, from the maintainer: memory organised the way Claude Code's is.

- There is an **index** of all topics.
- For each topic there is a **detailed document**.
- The index is **always in the context**.
- Single documents are **fetched by the agent through a tool** when it needs
  them.

### How Claude Code does it

For reference, as Claude Code's own memory works:

- A memory directory per project holds one Markdown file per memory, each
  with a front matter of `name` (a slug), `description` (one line, used to
  decide relevance) and `type` (`user`, `feedback`, `project`, `reference`).
  The body holds the fact, for feedback and project memories followed by a
  **Why:** and a **How to apply:** line; memories link each other with
  `[[name]]`.
- `MEMORY.md` is the index: one line per memory, a link and a short hook, no
  content. It is loaded into the context at the start of every session (cut
  off beyond some length); the memory files are not.
- There are no dedicated memory tools. The model reads a memory file with
  the generic file-read tool when the index line suggests it is relevant,
  and writes and edits memories with the generic file tools - and then has
  to update the index line by hand, which can drift from the files.
- Rules that come with it: check for an existing memory before writing a new
  one and update it instead; delete memories that turned out to be wrong;
  do not save what can be derived elsewhere (the code, the history); recalled
  memories are background, as true as when they were written, to be checked
  before being relied on.

Besides this, Claude Code loads `CLAUDE.md` / `AGENTS.md` files in full -
instructions, not memory. And Claude Code's *skills* follow the same pattern
as its memory: only their name and description are in the context, the body
is loaded when the skill is used.

## Why it matters

- Memory that outlives sessions ([18](18-knowledge-sharing.md)) is only
  useful if it can hold more than a line per fact and still be paid for in
  every turn. An index is bounded by the number of topics, not by the amount
  of knowledge.
- It is a middle way between "everything in the prompt" and "the model must
  think of searching" ([59](59-retrieval-of-notes.md)): the index tells the
  model *that* it knows something about a topic, so it knows to look it up.
- Documents that are revised in place are what consolidation
  ([55](55-notes-by-reflection.md)) needs as its target.

## Considerations

- **The index is derived, not written.** In Claude Code the index is a file
  of its own, kept in step by the model. Here the index line should be part
  of the document (name and description, as in the front matter), and the
  index the list of the documents that may be recalled. Then it cannot
  drift, and it is filtered by the same rule as the documents.
- **Confidentiality is the hard part** - decided on the basis of
  [167](167-origin-and-releases.md) (origin and releases). Today every
  entry carries the classification of the channel it was written in and is
  recalled only where that may be read ([131](131-agent-memory-bypasses-policy.md),
  [133](133-classification-model.md), [134](134-session-privacy.md)). A
  document written to from several channels mixes their classifications.
  Options:
  - The document's classification is the join of everything written to it,
    raised by every write from a higher channel. Simple and safe, but a
    single confidential sentence makes the whole topic disappear from
    everywhere below it - and the agent may then start a second document on
    the same topic there.
  - Documents per topic *and* classification: the index shows, per channel,
    the documents readable there. Two documents of one name are confusing.
  - Sections inside the document, each with its own classification; reading
    filters them. Most precise, most complex, and the description in the
    index needs its own classification too.
  - Writing to a document from a channel lower than the document is refused,
    or forks the document. The index line itself leaks the topic's existence
    and has to be classified like the document (at least).
  `reclassify_memory` has to work on whatever unit is chosen.
- **Changing a document.** The ledger knows `Add`, `Remove` and
  `Reclassify`. Documents need replacing the text (or a section, or an edit
  by search and replace, like a file-edit tool) - a `Revise` change, or the
  ledger's "remove and add again" with the topic as stable identity instead
  of a number. The chain stays append-only, so earlier versions remain
  (principle 8 of [19](19-memory-tiers.md)); concurrent writes from turns in
  several channels need the same care as `reserve_memory_id` today - last
  writer wins on one document is the case to decide.
- **Naming a document.** A slug the model chooses (`user-preferences`)
  instead of a number: readable in the index, stable across revisions,
  unique per agent. Numbers were chosen for notes because the model repeats
  them; a slug serves that as well.
- **Tools.** Along the lines of `read_memory(topic)`,
  `write_memory(topic, description, text)` (create or replace),
  `edit_memory(topic, old, new)` perhaps, `forget(topic)`,
  `reclassify_memory(topic, ...)`. All `ToolVisibility::HIDDEN`, as now.
  Dedicated tools rather than generic file tools: Caelumora has no file
  system for agents, and the tools are where classification and staging are
  enforced.
- **Recording what was read.** `KnowledgeSource::Memory { agent, digest }`
  records the whole chain. With an index and fetched documents, the index
  shown in the prompt and each document read are separate reads; the
  reference has to name the document (and its version) - the same
  refinement [58](58-notes-in-every-prompt.md) and
  [59](59-retrieval-of-notes.md) need.
- **Commit, don't write.** A written document is staged and taken over with
  the turn, like every change to memory now. Within the turn,
  `read_memory` should see the turn's own staged writes
  ([156](156-notes-and-memory-staged-scans.md)).
- **The same pattern as skills.** Skills ([22](22-skill-management.md))
  will want exactly this: an index of names and descriptions in the context,
  the body loaded by a tool. Worth building the "index in the prompt, body by
  tool" part once - a prompt part plus a fetch tool over documents with
  name, description and classification - and using it for both.
- **Replace or add.** Either `agent_memory` is changed to documents (its
  entries need not be migrated, see [108](108-serde-defaults-for-legacy.md)),
  or a second component is offered beside it and chosen per personality, as
  private notes and memory are ([68](68-notes-in-agent-state.md)). Two
  memories side by side for the same purpose is the less clean option.
- **Guidance for the model.** Much of how well Claude Code's memory works
  is in the instructions: when to write, update rather than duplicate,
  delete what is wrong, do not store what is derivable, treat recalled
  memory as possibly stale. These belong in the component's prompt part or
  tool descriptions ([57](57-prompt-catalogue.md)).
- **Bounded index.** The index grows with the topics; a cap with "n more,
  use `list_memory`" keeps it bounded, as [58](58-notes-in-every-prompt.md)
  proposes for notes.

## Done when

- An agent's memory consists of documents with a name, a one-line
  description and a text, each revised in place, with earlier versions kept
  in the chain.
- The prompt part shows only the index of the documents that may be recalled
  in the channel; a tool fetches a document, and both are recorded as
  knowledge, naming the document read.
- Confidentiality is enforced per the option decided above, for the index
  as well as the documents.
- `docs/MEMORY.md` describes it.

## History

- **2026-10-01** — Opened at the maintainer's request, modelled on Claude
  Code's memory (index always in context, one document per topic, documents
  fetched on demand). The unit of classification and replacing versus adding
  to `agent_memory` are to be decided before implementing.
- **2026-10-01** — The classification of documents depends on
  [167](167-origin-and-releases.md): a write has exactly one origin, the
  label of the turn that writes; releases are separate. With it, the
  options above reduce to "a document's origin is that of its newest
  version, written where it could be read", plus releases.
- **2026-10-01** — [167](167-origin-and-releases.md) was rejected. A
  document keeps the one classification it was given; who may change it
  and what a write must respect are decided in
  [169](169-changing-classified-content.md), which this issue depends on.
  Left here: what is special to memory documents - which classification a
  new one gets, and who owns it.
