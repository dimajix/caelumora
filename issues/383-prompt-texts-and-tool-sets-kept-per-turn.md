# 383 — Prompt texts and tool sets are kept in memory for the record

**Status:** open
**Area:** Persistence

## The problem

An agent's `AgentChannelState` keeps every prompt text it was given in a channel (`prompts`) and
every set of tools it was offered (`offered_tools`), each once by digest (`Kept<V>`), so that the
`ContextRef` recorded with each of its messages can be resolved to what the model saw (issues 06,
47). A system prompt changes with every note, every memory and every participant, so a new text is
kept nearly every turn; the texts are large. They are loaded with the state and held whole, like
the chains of [376](376-histories-held-whole-in-memory.md), and read for two things: the next
model call, which needs the current text alone, and the context view of an old message, which needs
exactly the texts that context names.

## Why it matters

- The texts grow with the number of turns, and a system prompt is thousands of tokens.
- After [380](380-transcript-window.md) reads an old branch of the transcript from the store, the
  texts it refers to are what still has to be in memory for the context view to work - unless
  they lie in the store too.

## Considerations

- **What is kept once by digest is a key-value store already**: `Kept<V>` tells the digests added
  since it was last stored, the JSON store writes them into the journal, Postgres would make a
  table of them. A read by digest is what the `ContextResolver` does (`prompts.get(&digest)`); it
  can go to the store for a digest not in memory, if the resolver is given a way to - which it is
  not today: it resolves synchronously from `&AgentChannelState`.
- **What stays in memory**: the texts the turn under way refers to, and the last turn's, which the
  next call reuses when nothing changed. Everything older is read on request, by the context view,
  which loads the branch first (380) and can load the texts with it.
- **Where the resolver gets them**: the context view hands the resolver what it loaded - the
  branch, the texts, the tool sets - rather than the resolver fetching anything; the state in memory
  does the same with what it holds. One resolver, two sources of the same shape.

## Done when

- An agent's state holds in memory only the prompt texts and tool sets its current and last turn
  refer to; older ones are read from the store for the context view of an old message.
- The context view of a message produced long ago still shows what the model was given.
- STORAGE.md and AGENT-TURN.md say so.

## Plan: the store keeps every text, the state what its turn names

Decided with the maintainer on 2026-10-16. Built as 380 built the transcript: what the state keeps
is written by its put, apart from the document, read by the `TranscriptStore`, and let go of in
memory once it is stored and nothing in memory reads it.

### 1. Who reads the texts in memory

Only the turn under way: its next call (`Turn::full_context`), how full its context is
(`Work::token_positions`), the tools it offers (`Work::tool_sets`), and the call of an external
agent. Between turns nothing does: `next_context_tokens` resolves a reference without prompt parts
or tools, answering again begins a new turn whose `ContextProvider` assembles the texts anew, an
external agent tells what it gave by digest (`RemoteState::given_prompts`), and `context_until`
places no parts. The texts of an old message's context are read by `AgentContextView::context_of`
alone, from the store.

### 2. The texts apart from the document

`AgentChannelState::prompts` and `offered_tools` become `#[serde(skip)]`;
`AgentChannelStateChange::Prompt` and `OfferedTools` go. The put writes what `Kept` added since it
was stored *first* - before the history, the transcript and the document -, so that no message of
the history, no entry of the transcript and no stored turn names a text that is not there after a
crash. The get loads the texts and the tool sets the stored turn names (`turn.work.context`), so
that a turn cut off by a crash is resumed with them.

The JSON store keeps them in `<key>.given/` beside the state's snapshot, one file per digest
(`prompt-<digest>.json`, `tools-<digest>.json`), read by digest and deleted with the state. Writing
is idempotent: a digest already there is not written again - in memory a text is let go of between
turns (4), and the next turn keeps the same system prompt anew. A database makes a table of each,
written with `ON CONFLICT DO NOTHING`. Should looking on disk turn out too slow, the store keeps the
digests it wrote last in a small cache; that is an optimization, not part of this issue.
*Weighed and rejected:* an appended file of texts, which a read by digest would have to scan.

A text that nothing names - that of a turn cut off before it said anything - stays in the store, as
it does today in `Kept`; clearing such texts away is not part of this issue.

### 3. Read through the `TranscriptStore`

Two methods beside `path`, `message` and `given`:

- `prompts(agent, channel, digests) -> BTreeMap<Digest, String>`: the texts of those digests that
  are kept.
- `offered_tools(agent, channel, digest) -> Option<Vec<ToolSet>>`: the tool sets of one offer.

The context view is the only reader, and it reads all three for one `ContextRef`; the parts are
placed in the transcript (`PromptRef::after`). *Weighed and rejected:* a trait of its own, which
would be handed through the `RuntimeBuilder`, the `Views` and the `StateRef` as the
`TranscriptStore` was, for one reader that needs both.

### 4. What stays in memory

`AgentChannelState::keep_window` keeps of `prompts` and `offered_tools` only what the turn under
way names (`turn.work.context.prompts`, `turn.work.context.tools`); between turns nothing.
`Kept` lets go only of what is stored, never of what is pending, whenever it is asked to. The
issue's frame - the texts of the current *and the last* turn - is narrowed: the last turn's texts
were to be kept only so that an unchanged system prompt is not written again, which the store's
idempotent write (2) takes care of.

A turn given up leaves its messages in the history, whose contexts name its texts; they were
written with the put that wrote the messages, and are let go of in memory with the next put after
the turn ended.

### 5. The resolver

`ContextResolver::on_path(state, path)` becomes `on_path(state, path, prompts, offered_tools)`:
the context view loads the texts and the tool set the reference names and hands them over in the
shape the state holds them (`Kept`). `ContextResolver::of(state)` stays as it is. One resolver, two
sources of the same shape.

### 6. Debt cleared on the way

- `TurnState::tools` is a flat copy of the offered tool sets, and the whole turn is journaled with
  every put (`AgentChannelStateChange::Turn`), every tool's schema with it. It goes:
  `Turn::tools()` reads the offered sets the state holds, which it holds as long as the turn names
  them (4).
- `RecordedToolSets`, the `alias = "tool_sets"` and `Kept::map` read stored data of an older form,
  which the rules no longer allow; they go with the texts leaving the document.

### What is documented

STORAGE.md: the texts and tool sets beside the transcript, written first, idempotent by digest.
AGENT-TURN.md, *What is held in memory*: the texts the turn under way names; those of an old
message's context read from the store.

### Steps

1. Without a change of behaviour: `TurnState::tools` goes, `Turn::tools()` reads the offered sets;
   `RecordedToolSets`, the alias and `Kept::map` go.
2. The texts and tool sets apart from the document, written first and idempotently in both
   backends, `prompts` and `offered_tools` on the `TranscriptStore`, their tests in the store
   suite; the get loads what the stored turn names.
3. The window: `keep_window` keeps what the turn under way names.
4. `context_of` loads the texts and the tool set from the store; `ContextResolver::on_path` takes
   them.
5. The documents; this issue done.

Every stored state has to be deleted afterwards.

## History

- **2026-10-16** — Opened as part of the epic [378](378-resource-management-epic.md), noticed while
  weighing options on 376.
- **2026-10-16** — [380](380-transcript-window.md) done: the context of an older message is resolved
  against a path of the transcript the view loads from the store before
  (`ContextResolver::on_path`); the prompt texts and tool sets it names are still taken from the
  agent's state as loaded. This issue loads them the same way, by the digests the context names.
- **2026-10-16** — The plan decided with the maintainer: the texts and tool sets leave the
  document and are written first by the put, idempotently by digest; the `TranscriptStore` reads
  them for the context view; in memory the state holds what the turn under way names, not the last
  turn's as well, since the store's idempotent write is all that was for; `TurnState::tools` and the
  reading of older stored forms go on the way.
