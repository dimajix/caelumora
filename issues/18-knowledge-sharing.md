# 18 — Knowledge sharing between sessions

**Status:** done
**Area:** Context and knowledge
**Summary:** Sessions are sealed: nothing an agent learns outlives the session, and cross-agent sharing is a harder question again.

## The problem

Sessions are sealed. What an agent learned in one session is unavailable in the next: `AgentState`
is per-session, and nothing outlives the session that produced it. Every session starts from the
template.

The original TODO entry adds the harder half as a parenthesis — *"possibly between different
agents?"* — which is a materially different feature from one agent remembering its own past.

## Why it matters

Without this, an agent cannot accumulate anything. The same explanation is given to it every time,
and any expensive work it did — reading a codebase, learning a convention — is discarded at session
end.

## Considerations

- This is the retrieval problem, and it is the largest single item in the context area. It needs a
  store, a write path (what is worth keeping?), a read path (what is relevant now?), and an
  invalidation story (what is no longer true?). The last one is usually underestimated: stale
  knowledge confidently asserted is worse than no knowledge.
- Cross-*agent* sharing raises questions the same-agent case does not: whose knowledge is it, does
  it carry the access rights of the session it came from ([12](12-user-and-agent-permissions.md)),
  and what happens when two agents hold contradictory beliefs.
- Depends on [19](19-memory-tiers.md) for its scope model — "agent global knowledge" and "global
  knowledge" are the tiers this issue would populate. Doing this one first would mean inventing that
  model implicitly.
- Overlaps with [16](16-context-compression.md): a summarisation pass that runs at session end is a
  plausible write path for both.
- Should almost certainly be built as components against the existing traits rather than as a new
  subsystem.

## Done when

- The scope model from [19](19-memory-tiers.md) exists.
- An agent can retrieve relevant knowledge from its own past sessions.
- Cross-agent sharing is either implemented or explicitly deferred with the access-control reasoning
  recorded here.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Decided with branching ([08](08-retry-and-edit-past-answers.md)); principles in
  [19](19-memory-tiers.md): knowledge beyond the session is not written but committed from
  branch-bound notes ([17](17-information-sharing-within-session.md)) at a defined point, explicitly
  ("remember", optionally with approval) to begin with. Every entry records its origin and the
  knowledge it was derived from, so that it can be retracted when its origin is discarded or
  deleted. Forgetting means retracting and invalidating; knowledge is never learnt into model
  weights.
- **2026-10-15** — Closed as done; the three points of *Done when* are met or answered:
  - The scope model of [19](19-memory-tiers.md) exists, in the terms of
    [MEMORY.md](../docs/MEMORY.md#terms).
  - An agent keeps what it learnt beyond the session: agent memory (`agent_memory`), recalled in
    its other sessions where the label of each entry may be read. It does not follow principle 7 of
    19 - `remember` writes directly rather than committing from notes -, as recorded there on
    2026-09-28.
  - Knowledge shared *between* agents is not built as a tier of its own and is deferred: whose it
    is and who may read it is answered by an owner and an access list, which a workspace shared
    with others already has - a shared workspace is shared knowledge with an owner, its
    classification the question of [169](169-changing-classified-content.md). Memory an agent can
    revise and look up by topic is [166](166-memory-as-topic-documents.md).
