# 19 — Memory tiers

**Status:** open
**Area:** Context & knowledge

## The problem

`TODO.md` lists four kinds of memory without defining any of them:

- agent channel context
- agent session memory
- agent global knowledge
- global knowledge

None exist. More importantly, the boundaries between them do not exist either
— what belongs in which tier, how long each lives, who can read it, and what
happens at the boundaries (session ends, agent is deleted, channel closes).

## Why it matters

This is the scope model that [17](17-information-sharing-within-session.md)
and [18](18-knowledge-sharing.md) both need. Building either of those first
means inventing the tiers implicitly and inconsistently, and then having to
reconcile them.

It is also the issue where getting the *names* right pays off: "context",
"memory", "knowledge" and "history" are already used loosely across the
codebase and the architecture document, and a tier model that reuses those
words imprecisely will confuse every later reader.

## Considerations

- The four tiers are distinguished by two independent axes — scope (channel /
  session / agent / global) and lifetime (turn / session / permanent). Writing
  them out as a grid is likely to show that four is either too few or too
  many.
- Each tier needs an answer to: what writes it, what reads it, when is it
  invalidated, who may see it, and where is it stored (`StateStore`,
  `EntityStore`, or something new — see
  [04](04-postgres-state-store.md)).
- "Global knowledge" is the tier most in need of justification: knowledge
  shared across all agents and all users is also the tier where one bad entry
  affects everything, and where [12](12-user-and-agent-permissions.md) has the
  most to say.
- Deletion cascades from [07](07-session-deletion-audit.md) land here: does
  deleting a session remove what the agent learned from it?
- Prefer expressing the tiers through the existing component traits over
  introducing a parallel mechanism.

## Done when

- The tiers are defined here: scope, lifetime, reader, writer, invalidation,
  storage, access rules.
- The terminology is reconciled with `ARCHITECTURE.md`'s existing use of
  "context", "history" and "state".

## History

- **2026-09-10** — Issue created from `TODO.md`.
