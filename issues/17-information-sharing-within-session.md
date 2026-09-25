# 17 — Sharing information within a single session

**Status:** open
**Area:** Context & knowledge

## The problem

Within a session, each agent maintains its own `AgentState` — its own history,
its own context. Agents in a channel learn about each other only through the
messages the moderator routes to them. There is no shared scratchpad: no place
where one agent can put a finding that another agent can read directly.

## Why it matters

This is what makes multiple agents in one session more than several
independent agents in adjacent windows. Without it, everything one agent knows
has to be re-derived by another or re-stated in the conversation, which is both
expensive and lossy — and it pollutes the channel with messages written for
other agents rather than for the user.

## Considerations

- The obvious shape is a session-scoped store that agents read from and write
  to through a component — a `ContextProvider` that injects shared entries,
  and a tool or processor that writes them. Reusing the existing component
  traits keeps this out of the actor core.
- Visibility needs to be explicit. If everything written is visible to every
  agent, the shared area becomes an unfiltered second conversation and
  contributes to [16](16-context-compression.md)'s problem rather than
  relieving it.
- Concurrency is real here: agents in a channel can run simultaneously, so
  the shared area needs defined semantics for two writers. Last-write-wins on
  a whole document would silently lose findings.
- Scope boundaries matter and should be decided together with
  [19](19-memory-tiers.md), which defines the tiers this would be one of:
  channel context vs. session memory is exactly the distinction at stake.
- Whether an agent may read what another agent wrote is also an access
  question — see [12](12-user-and-agent-permissions.md).

## Plan (2026-09-25)

Phases 1 and 2 of the order of work in [19](19-memory-tiers.md), whose
principles (points 2-6, 9, 11) this follows. Requires phase 0:
[47](47-prompt-parts-with-provenance.md) and [48](48-shared-digest-chain.md).

**Phase 1 - notes within a channel**
- [ ] 4. Model: a note chain in `ChannelState`, on the chain type of
  [48](48-shared-digest-chain.md). Entries are "note" or "revoke N"; a new
  version is a revocation plus a note. Each records its provenance: agent,
  triggering message, context digest. The current notes are the chain read
  from start to head.
- [ ] 5. Writing: a tool provider with `note` and `revoke_note`. Only the
  channel's own entries can be revoked.
- [ ] 6. Reading in the own channel: a `PromptProvider` contributing the
  current notes as a `BeforeTurnInput` part
  ([47](47-prompt-parts-with-provenance.md)), so changing notes do not
  invalidate the prompt cache; recorded with every answer through its
  `PromptRef`.
- [ ] 7. Web UI: a notes panel per channel, read-only, showing who wrote a
  note and in reply to which message.

**Phase 2 - across channels**
- [ ] 8. `ReadChannel(&attributes)` checked by the `SecurityManager`, with the
  strict rule: all participants of the reading channel must be allowed to
  read the source channel ([12](12-user-and-agent-permissions.md)).
- [ ] 9. A tool `read_notes(channel)` returning the active chain's notes of
  finished turns, and `KnowledgeRef { channel, source, digest }` recorded in
  `ContextRef`.
- [ ] 10. Optionally, session memory as a prompt part: the current notes of
  all channels the agent may read, with their `KnowledgeRef`s.
- [ ] 11. A tool `read_history(channel, range/search)`, also recorded as a
  `KnowledgeRef`.
- [ ] 12. The web UI's context panel ([41](41-show-message-context.md)) shows
  which notes and histories went into an answer.

## Done when

- Agents in one session can exchange structured information without routing it
  through channel messages.
- Write conflicts have defined semantics.

## History

- **2026-09-10** — Issue created from `TODO.md`.
- **2026-09-25** — Shape decided together with branching
  ([08](08-retry-and-edit-past-answers.md)); principles in
  [19](19-memory-tiers.md): the notes of a channel are a chain of entries
  that are only appended (which also settles the question of two writers)
  and branch with the channel. Other channels of the session read its active
  chain directly, only notes of finished turns. Such a read is an effect for
  the writing channel and is recorded by the reader as a structured reference
  to the state of knowledge it saw (digest of the note chain's head, e.g. in
  `ContextRef`).
- **2026-09-25** — Refined ([19](19-memory-tiers.md), points 3, 3a, 9-11):
  session memory is the union of the active note chains of all channels,
  every entry owned by the channel that wrote it; revoking and new versions
  are entries of the chain. Other channels may also read a channel's history,
  through a tool. Documents several channels edit together do not branch and
  are a separate shared workspace whose changes are effects. Reading across
  channels is checked, see [12](12-user-and-agent-permissions.md).
- **2026-09-25** — Plan for phases 1 and 2 added above, from the order of
  work in [19](19-memory-tiers.md). Notes are read as a transient prompt part
  before the turn's input ([47](47-prompt-parts-with-provenance.md)), not
  through the system prompt, because many models put the system prompt first
  and any change to it invalidates the prompt cache.
