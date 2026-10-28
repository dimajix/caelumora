# 12 — Combining user permissions with agent permissions

**Status:** done
**Area:** Identity and access management
**Summary:** When an agent acts for a user, two sets of rights are involved and there is no rule for combining them.

## The problem

When an agent acts, two subjects are involved: the user who initiated the interaction and the agent
doing the work. Each could reasonably have its own rights — the user may read a repository the agent
may not; the agent may hold an MCP credential the user does not personally have. The system has no
rule for combining them.

`SecurityContext` already anticipates the problem: it carries `initiator_id`, `caller_id` *and* a
`delegation_ids` chain. What is missing is the policy that reads them.

## Why it matters

This is where an agent system differs from an ordinary application, and getting it wrong is
dangerous in both directions. Take only the agent's rights, and an agent becomes a way for any user
to reach anything the agent can reach — a confused deputy. Take only the user's, and agents cannot
hold credentials of their own, which is half the point of an `mcp_tool` component.

## Considerations

- The conservative rule is **intersection**: an action is permitted only if every identity in the
  delegation chain permits it. It is easy to reason about and it is the safe default, but it forbids
  the legitimate case of an agent holding a capability its user lacks.
- The alternative is an explicit **grant**: the agent holds rights of its own, and the user is
  permitted to *invoke the agent*, not to exercise the agent's rights directly. That models the
  credential case correctly and is how the `mcp_tool` secret already behaves in practice — but it
  means the user cannot audit what they just caused to happen, which loops back to
  [05](05-postgres-event-recorder.md).
- Whichever rule is chosen has to hold for the whole `delegation_ids` chain, not just its two ends —
  agent-to-agent delegation is possible in a multi-agent channel.
- `ActionType::ReadSecret` is the sharpest case to reason through: a secret resolved on the agent's
  behalf during a user-initiated turn.

## Done when

- The combination rule is decided and written down here, with the confused deputy case explicitly
  addressed.
- `SecurityManager` implements it over the full delegation chain.

## History

- **2026-09-10** — Issue created from `TODO.md`, where it was phrased as a question: *"Combining
  user permissions with agent permissions?"*
- **2026-09-25** — A concrete case from the branching and memory principles in
  [19](19-memory-tiers.md) (point 11): an agent in channel B reading the notes or history of channel
  A. If only the agent's rights are checked, it becomes a bridge through which every user in B
  learns what was said in A. Until this issue decides how agent and user rights combine, the strict
  rule applies: all participants of the reading channel must be allowed to read the source channel
  (`ReadChannel`).
- **2026-09-25** — A second case from [19](19-memory-tiers.md) (principle 12,
  [51](51-agent-private-notes.md)): an agent's private notes from channel A must not surface in
  channel B for users who may not read A. Every private note carries the security attributes of its
  origin channel and is filtered by the same rule as reading across channels, so relaxing that rule
  here relaxes both.
- **2026-09-25** — The strict rule for reading across channels ([19](19-memory-tiers.md), point 11)
  now checks the **users** of the reading channel with `ActionType::ReadChannel`, not its agents:
  under the ownership policy an agent may only read channels it takes part in, so counting agents
  would block reading whenever channels have different agents. Implemented as
  `ChannelLoop::may_read`.
- **2026-10-08** — A concrete case opened as [255](255-whose-name-agents-act-in.md): today an agent
  reads in its owner's name (its views) and acts in the name of whoever asked it (its turn), one
  tool call often in both. SECURITY.md (*In whose name an agent acts*) describes that state.
- **2026-10-08** — Decided with the maintainer, in [255](255-whose-name-agents-act-in.md)
  (*Decision*): the **grant**, not the intersection. An agent is its owner's delegate and acts with
  its owner's rights in all it does; the user is permitted to *ask* it - by being in the channel the
  owner put it into, later perhaps by an `AskAgent` - and the owner checks the request where
  `GuardedToolExecutor::admit` runs the policy, their rules for the place and the approval. The
  intersection was rejected because it is the wrong model: one turns to the person entitled to
  something, who checks the request and carries it out with their own rights. The confused deputy is
  answered by that check, not by the asker's rights; what remains - an agent without approval lets
  every member of its channel read through it, within the channel's classification - is accepted and
  written down. The audit side (05) is served by the initiator, which the context keeps for the
  record and for the questions of a call. The *done when* is narrowed with it: the `SecurityManager`
  combines two identities, the principal of the caller and the initiator, not a chain -
  `delegation_ids` records a conversation, not a commission, and goes; a chain is reconsidered with
  subagents. `ReadSecret` is the owner's: a credential belongs to the agent's configuration, and bob
  using alice's agent uses her token, as decided. Implemented with 255's plan; this issue closes
  with it.
- **2026-10-08** — Done with [255](255-whose-name-agents-act-in.md)'s plan: a `SecurityContext`
  names the caller, its principal - a user themselves, an agent its owner - and the initiator,
  and the ownership policy judges by the principal; `delegation_ids` is gone. One correction to
  the entry above: the questions a call puts to the agent's owner (`Addressee::Owner`) go to the
  principal, not to the initiator - they had gone to whoever asked, which is fixed. The initiator
  is kept for the record and for which request a turn answers.
