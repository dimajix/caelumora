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

## History

- **2026-10-16** — Opened as part of the epic [378](378-resource-management-epic.md), noticed while
  weighing options on 376.
