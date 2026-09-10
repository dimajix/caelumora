# 12 — Combining user permissions with agent permissions

**Status:** open
**Area:** Identity & access management

## The problem

When an agent acts, two subjects are involved: the user who initiated the
interaction and the agent doing the work. Each could reasonably have its own
rights — the user may read a repository the agent may not; the agent may hold
an MCP credential the user does not personally have. The system has no rule
for combining them.

`SecurityContext` already anticipates the problem: it carries `initiator_id`,
`caller_id` *and* a `delegation_ids` chain. What is missing is the policy that
reads them.

## Why it matters

This is where an agent system differs from an ordinary application, and
getting it wrong is dangerous in both directions. Take only the agent's
rights, and an agent becomes a way for any user to reach anything the agent
can reach — a confused deputy. Take only the user's, and agents cannot hold
credentials of their own, which is half the point of an `mcp_tool` component.

## Considerations

- The conservative rule is **intersection**: an action is permitted only if
  every identity in the delegation chain permits it. It is easy to reason
  about and it is the safe default, but it forbids the legitimate case of an
  agent holding a capability its user lacks.
- The alternative is an explicit **grant**: the agent holds rights of its own,
  and the user is permitted to *invoke the agent*, not to exercise the agent's
  rights directly. That models the credential case correctly and is how the
  `mcp_tool` secret already behaves in practice — but it means the user cannot
  audit what they just caused to happen, which loops back to
  [05](05-postgres-event-recorder.md).
- Whichever rule is chosen has to hold for the whole `delegation_ids` chain,
  not just its two ends — agent-to-agent delegation is possible in a
  multi-agent channel.
- `ActionType::ReadSecret` is the sharpest case to reason through: a secret
  resolved on the agent's behalf during a user-initiated turn.

## Done when

- The combination rule is decided and written down here, with the confused
  deputy case explicitly addressed.
- `SecurityManager` implements it over the full delegation chain.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it was phrased as a
  question: *"Combining user permissions with agent permissions?"*
