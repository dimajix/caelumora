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

## Done when

- Agents in one session can exchange structured information without routing it
  through channel messages.
- Write conflicts have defined semantics.

## History

- **2026-09-10** — Issue created from `TODO.md`.
