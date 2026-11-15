# 166 — Agent memory is a flat list of short entries, all of it in every prompt

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** Agent memory is a flat list of one-line entries, all of them in every prompt, with nothing to group, revise or fetch them by topic.

## The problem

Agent memory ([62](62-json-store-concurrent-writes.md), [68](68-notes-in-agent-state.md)) is a
ledger of short texts: `remember(text)` adds one, `forget(id)` removes one, and the prompt part
lists every entry that may be recalled in the channel, in full, in every turn. That shape has three
consequences:

- **No room for detail.** An entry is a line. What an agent learns about a subject - a user's
  preferences, how a project is set up, what was decided and why - is either squeezed into one line
  or spread over many unrelated ones that nothing groups.
- **No revising.** An entry cannot be changed, only forgotten and written anew. Knowledge that grows
  over time is a trail of half-outdated lines, and keeping it tidy is left to the model
  ([55](55-notes-by-reflection.md)).
- **Everything, always.** The prompt part grows with every entry and is paid for in every turn,
  whether the turn needs it or not - the problem of [58](58-notes-in-every-prompt.md), for memory.

Wanted, from the maintainer: memory organised the way Claude Code's is.

- There is an **index** of all topics.
- For each topic there is a **detailed document**.
- The index is **always in the context**.
- Single documents are **fetched by the agent through a tool** when it needs them.

### How Claude Code does it

For reference, as Claude Code's own memory works:

- A memory directory per project holds one Markdown file per memory, each with a front matter of
  `name` (a slug), `description` (one line, used to decide relevance) and `type` (`user`,
  `feedback`, `project`, `reference`). The body holds the fact, for feedback and project memories
  followed by a
  **Why:** and a **How to apply:** line; memories link each other with `[[name]]`.
- `MEMORY.md` is the index: one line per memory, a link and a short hook, no content. It is loaded
  into the context at the start of every session (cut off beyond some length); the memory files are
  not.
- There are no dedicated memory tools. The model reads a memory file with the generic file-read tool
  when the index line suggests it is relevant, and writes and edits memories with the generic file
  tools - and then has to update the index line by hand, which can drift from the files.
- Rules that come with it: check for an existing memory before writing a new one and update it
  instead; delete memories that turned out to be wrong; do not save what can be derived elsewhere
  (the code, the history); recalled memories are background, as true as when they were written, to
  be checked before being relied on.

Besides this, Claude Code loads `CLAUDE.md` / `AGENTS.md` files in full - instructions, not memory.
And Claude Code's *skills* follow the same pattern as its memory: only their name and description
are in the context, the body is loaded when the skill is used.

## Why it matters

- Memory that outlives sessions ([18](18-knowledge-sharing.md)) is only useful if it can hold more
  than a line per fact and still be paid for in every turn. An index is bounded by the number of
  topics, not by the amount of knowledge.
- It is a middle way between "everything in the prompt" and "the model must think of searching"
  ([59](59-retrieval-of-notes.md)): the index tells the model *that* it knows something about a
  topic, so it knows to look it up.
- Documents that are revised in place are what consolidation ([55](55-notes-by-reflection.md)) needs
  as its target.

## What exists

As of 2026-11-15, since this issue was opened, the memory has changed in ways that settle some of
the questions below and sharpen others (see [MEMORY.md](../docs/MEMORY.md#agent-memory)):

- **A system service of its own.** The memory is the crate `caelumora-memory`, a plugin that keeps
  and decides and a library with the `agent_memory` component
  ([518](518-memory-is-reached-through-the-agents-state.md)). An agent's memory is one document of
  the kind `memory` (`AgentMemory`), kept under the agent's id, owned by its owner, removed when the
  agent is purged. A work reads what it may recall through the `MemoryReader` the plugin binds to
  it; a turn reserves numbers through its `MemoryWriter` and stages its changes, which the plugin
  checks again and commits to the memory as stored, under a lock per agent.
- **Entries are revised in place.** `update_memory(id, text)` ([302](302-update-note.md)) replaces
  the text of an entry, which keeps its number; the ledger knows `Add`, `Update`, `Remove` and
  `Reclassify`. "No revising" above no longer holds - what is still missing is room for more than
  a line, grouping by topic, and fetching on demand.
- **Every version carries the label of the turn that wrote it.** A new entry and an updated one are
  classified as `Situation::label` - the join of what the turn has seen, at most its channel
  ([168](168-turn-label-as-high-water-mark.md)) -, and every change records where it was made: the
  channel and its session's privacy then (`MemoryOrigin`, [134](134-session-privacy.md)). An entry
  is recalled only where its label may be read, decided by the plugin, asked of the policy
  ([131](131-agent-memory-bypasses-policy.md)); it is changed only where it is recalled, or in the
  turn that remembered it. For entries the question of
  [169](169-changing-classified-content.md) is thereby answered by a label per version, not by a
  fixed container - which works because the memory's prompt part shows every entry, so the turn
  that updates one has seen it and its label is at least the entry's.
- **Its texts are in the prompt catalogue** ([57](57-prompt-catalogue.md)),
  `caelumora-memory/src/prompts.yaml` - where the guidance for the model belongs.
- **What is read is recorded** as `KnowledgeSource::entity::<AgentMemory>(id, digest)`: the memory
  as a whole by its digest, once per channel its recalled entries were written in, marked private.
- **Skills have the shape this issue wants** ([22](22-skill-management.md)): a catalogue of names
  and descriptions as a prompt part, `load_skill(name)` giving the body into the agent's transcript
  (`Turn::give`, as a `Given` entry naming the skill), `list_skills` for a catalogue grown since the
  prompt was built.

## Considerations

- **The index is derived, not written.** In Claude Code the index is a file of its own, kept in step
  by the model. Here the index line should be part of the document (name and description, as in the
  front matter), and the index the list of the documents that may be recalled. Then it cannot drift,
  and it is filtered by the same rule as the documents.
- **Confidentiality is the hard part.** Today every version of an entry carries the label of the
  turn that wrote it and is recalled only where that may be read (see *What exists*). The History
  below leans the other way - a document keeps the one classification it was given, as
  [169](169-changing-classified-content.md) proposes -, while `update_memory`, decided after it
  ([302](302-update-note.md)), gives each version its writer's label. Documents have to settle this
  one way: carried over as it is, the label per version is the first option below. A document
  written to from several channels mixes their classifications. Options:
  - The document's classification is the join of everything written to it, raised by every write
    from a higher channel. Simple and safe, but a single confidential sentence makes the whole topic
    disappear from everywhere below it - and the agent may then start a second document on the same
    topic there.
  - Documents per topic *and* classification: the index shows, per channel, the documents readable
    there. Two documents of one name are confusing.
  - Sections inside the document, each with its own classification; reading filters them. Most
    precise, most complex, and the description in the index needs its own classification too.
  - Writing to a document from a channel lower than the document is refused, or forks the document.
    The index line itself leaks the topic's existence and has to be classified like the document (at
    least). `reclassify_memory` has to work on whatever unit is chosen.
  - **A blind write lowers.** With the label per version, an entry's update is safe today only
    because the prompt part shows the entry, which raises the turn's label to it. With only the
    index in the prompt, a turn that replaces a document it has not fetched writes it at its own,
    possibly lower, label - and what the document said before is lost from where it was readable,
    or the new version is readable where the old one was not. A write to a document the turn has
    not read in its current version should be refused, as the file tools write a file only as the
    agent last knew it (`AgentChannelState::last_known`,
    [MEMORY.md](../docs/MEMORY.md#recording-what-was-read)).
- **Changing a document.** The ledger knows `Add`, `Update`, `Remove` and `Reclassify`; `Update`
  replaces an entry's whole text, which for a document of some length means the model repeats all
  of it. Wanted besides: replacing a section, named by its heading, or an edit by search and
  replace, like a file-edit tool - either a change of its own in the ledger, or resolved by the
  component into the new whole text before it is staged. The chain stays append-only, so earlier
  versions remain (principle 8 of [19](19-memory-tiers.md)). Concurrent writes: the plugin commits
  a turn's changes to the memory as stored, under a lock per agent, so nothing is lost on the
  level of the chain - but a whole text staged in one turn overwrites what another turn wrote to
  the same document meanwhile. An edit staged as such (a section, a search and replace) can be
  applied to the current version instead, and refused where it no longer fits; since the plugin
  commits a turn's changes all or none, one such conflict would discard all of them. If
  [582](582-a-soul-the-agent-writes-itself.md) chooses the same editing operation for the soul, it
  is written once, as a function over a Markdown text in `caelumora-domain`.
- **Naming a document.** A slug the model chooses (`user-preferences`) instead of a number: readable
  in the index, stable across revisions. Numbers were chosen for notes because the model repeats
  them; a slug serves that as well. But a slug unique per agent leaks: a turn in a public channel
  that creates `project-x` and is told the name is taken learns that a document exists it may not
  recall - the probing that reading other channels rules out. Either names are unique only among
  the documents recalled in a place - so that two documents of one name may exist, the second
  option above -, or the identity is a reserved number as now and the slug only what the index
  shows.
- **Tools.** Along the lines of `read_memory(topic)`, `write_memory(topic, description, text)`
  (create or replace), `edit_memory(topic, old, new)` perhaps, `forget(topic)`,
  `reclassify_memory(topic, ...)`. All `ToolVisibility::HIDDEN`, as now. Dedicated tools rather than
  generic file tools: Caelumora has no file system for agents, and the tools are where
  classification and staging are enforced.
- **How a fetched document reaches the model.** As the tool's result, or - as a skill does - given
  into the transcript (`Turn::give`) as a `Given` entry naming the document. Either way it stands
  in the transcript as it was when fetched; a later revision from another channel is not seen
  until it is fetched again, and a compaction has to give again what the agent knew
  (`KnowledgeRestorer`, [AGENT-TURN.md](../docs/AGENT-TURN.md#compaction)).
- **Recording what was read.** `KnowledgeSource::entity::<AgentMemory>(id, digest)` records the
  memory as a whole, by the digest of its chain. With an index and fetched documents, the index
  shown in the prompt and each document read are separate reads; the reference has to name the
  document (and its version) - the same refinement [58](58-notes-in-every-prompt.md) and
  [59](59-retrieval-of-notes.md) need.
- **Commit, don't write.** A written document is staged and taken over with the turn, like every
  change to memory now. Within the turn, `read_memory` should see the turn's own staged writes
  ([156](156-notes-and-memory-staged-scans.md)).
- **The same pattern as skills.** Skills ([22](22-skill-management.md)) now have exactly this: a
  catalogue of names and descriptions as a prompt part, the body loaded by `load_skill`. The
  memory's index and fetch tool should be built the same way - the same form of the index, the
  same way the body reaches the transcript. What they share beyond that is small: a skill is an
  entity of its own, managed by people, while a memory document is part of the agent's memory,
  written by the agent, with a classification per version. Whether a common type for "an index
  line and a body" earns its place is to be judged once both exist side by side.
- **Replace or add.** Either `agent_memory` is changed to documents (its entries need not be
  migrated, see [108](108-serde-defaults-for-legacy.md)), or a second component is offered beside it
  and chosen per personality, as private notes and memory are ([68](68-notes-in-agent-state.md)).
  Two memories side by side for the same purpose is the less clean option.
- **Not the agent's character.** What the agent is to be - its manner, its instructions to itself -
  is not memory: it is in every prompt, everywhere, under rules of its own. It is the soul of
  [582](582-a-soul-the-agent-writes-itself.md), a component of its own, independent of the memory.
- **Guidance for the model.** Much of how well Claude Code's memory works is in the instructions:
  when to write, update rather than duplicate, delete what is wrong, do not store what is derivable,
  treat recalled memory as possibly stale. These belong in the component's prompt part or tool
  descriptions ([57](57-prompt-catalogue.md)).
- **Bounded index.** The index grows with the topics; a cap with "n more, use `list_memory`" keeps
  it bounded, as [58](58-notes-in-every-prompt.md) proposes for notes.

## Done when

- An agent's memory consists of documents with a name, a one-line description and a text, each
  revised in place, with earlier versions kept in the chain.
- The prompt part shows only the index of the documents that may be recalled in the channel; a tool
  fetches a document, and both are recorded as knowledge, naming the document read.
- Confidentiality is enforced per the option decided above, for the index as well as the documents.
- `docs/MEMORY.md` describes it.

## History

- **2026-10-01** — Opened at the maintainer's request, modelled on Claude Code's memory (index
  always in context, one document per topic, documents fetched on demand). The unit of
  classification and replacing versus adding to `agent_memory` are to be decided before
  implementing.
- **2026-10-01** — The classification of documents depends on [167](167-origin-and-releases.md): a
  write has exactly one origin, the label of the turn that writes; releases are separate. With it,
  the options above reduce to "a document's origin is that of its newest version, written where it
  could be read", plus releases.
- **2026-10-01** — [167](167-origin-and-releases.md) was rejected. A document keeps the one
  classification it was given; who may change it and what a write must respect are decided in
  [169](169-changing-classified-content.md), which this issue depends on. Left here: what is special
  to memory documents - which classification a new one gets, and who owns it.
- **2026-11-15** — Brought up to date with the memory as it stands after
  [302](302-update-note.md) and [518](518-memory-is-reached-through-the-agents-state.md) (*What
  exists*): entries are revised in place, each version labelled as the turn that wrote it, and the
  memory is a plugin and a library of its own. The considerations name what that changes: the label
  per version stands against the fixed classification leaned to above, a blind write to a document
  not fetched would lower it, a slug unique per agent would reveal documents that may not be
  recalled, and concurrent edits meet the all-or-none commit. The agent's character is split off
  into [582](582-a-soul-the-agent-writes-itself.md), independent of the memory.
