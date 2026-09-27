# 64 — Confidentiality has levels only, no compartments

**Status:** open
**Area:** Identity & access management

## The problem

Confidentiality is being introduced as ordered levels
(`public < session < channel < private`, see
[61](61-reasoning-leaves-the-agent.md)): a channel has a level, message parts
carry a level as their label, and the `SecurityManager` decides from both.
Levels say *how far* information may travel, not *to whom* by subject.
Information that belongs to a topic or a group - "project X", "HR",
"customer Y" - and may be shared between exactly the channels of that topic,
across sessions or within one, cannot be expressed: at `session` it reaches
every channel of the session, at `channel` none.

## Why it matters

- Real confidentiality is rarely one-dimensional: two channels of the same
  level may still not be allowed to see each other's content.
- Knowledge beyond the session ([18](18-knowledge-sharing.md)) will need
  exactly this: shared with some, not with all.

## Considerations

- **Compartments as sets beside the level** (the classic lattice): a label
  is `(level, {compartments})`; information may flow from A to B only if B's
  level is at least A's *and* B has all of A's compartments. Levels stay the
  special case with an empty set, so the step from levels to compartments
  does not change existing decisions.
- **Where compartments come from:** configured per deployment, assigned to
  channels by their owner (or inherited from template and session), and
  granted to users and agents as clearances - which makes joining a channel
  a check against its compartments (see [46](46-channel-participants.md)).
- **Who may create and assign compartments** - the owner of a session, an
  administrator, the `SecurityManager` policy ([24](24-opa-security-manager.md)
  would be the natural home for such rules).
- Derived labels (e.g. the label of reasoning, from what a turn read) take
  the union of the compartments of their sources.

## Done when

- It is decided whether confidentiality needs compartments, and how they
  are assigned and checked - or recorded as rejected, with the reason.

## History

- **2026-09-27** — Issue created at the maintainer's request while deciding
  [61](61-reasoning-leaves-the-agent.md): levels
  `public < session < channel < private` first, compartments later.
