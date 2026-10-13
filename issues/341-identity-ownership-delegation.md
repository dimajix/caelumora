# 341 — A security context cannot tell an agent's owner from the one it acts for

**Status:** open
**Area:** Identity and access management

## The problem

`SecurityContext` names three actors (issue [255](255-whose-name-agents-act-in.md)):

- `caller_id` - who acts: a user, an agent, the system;
- `principal_id` - the person whose rights the caller exercises: a user themselves, an agent its
  owner;
- `initiator_id` - who caused it, for the record and the questions of a call, never for rights.

`principal_id` answers two questions at once, because for an agent the two answers are the same
person today:

- **Ownership** - who is responsible for the agent: whose clearance bounds what it may see
  (`Member::clearances`, issue [142](142-roles.md)), whose rules and approvals admit its tool calls
  (issues 13, 14), and who it is checked against when it is taken into a session
  ([221](221-adding-session-participants.md): an agent matches a session's admission rule by its
  owner).
- **Delegation** - in whose name it acts: whose rights decide what it may read, call and create,
  whose credentials it reaches with.

255 decided that an agent is its owner's delegate, so `Agent::security_context` sets both to the
owner, and the agent does not store whose authority it acts with: it is derived from the owner
wherever a context is built. Every reader of `principal_id` - the policy, the store filters, the
roles of a clearance, the owner of what is created - takes it for whichever of the two it needs,
and nothing says which.

## Why it matters

- **Delegation will differ from ownership.** Issue [340](340-teams-as-owners.md) asks whether a
  team can be the one in whose name something is done. An agent acting for a team is still owned,
  and answered for, by a person. If `principal_id` became the team, every check that means the
  owner - clearance, admission, approval - would silently ask the team instead.
- **221 needs the owner now.** Its admission rule matches an agent by its owner. Reading
  `principal_id` for that is right today only by coincidence.
- **Persons and agents differ in kind.** For a natural person, identity and delegation are the
  same: they act in their own name, and nobody owns them. For an agent they fall apart as a rule:
  it is itself, it belongs to someone, and it acts for someone - its owner today, maybe a team or a
  commission tomorrow. The context should be able to say so instead of folding it into one field.

## Considerations

### The three, and the initiator

| | a user | an agent today | an agent acting for a team (340) |
|---|---|---|---|
| identity (`caller_id`) | the user | the agent | the agent |
| ownership | - (or themselves) | its owner | its owner |
| delegation (`principal_id`) | the user | its owner | the team |
| initiator | the user | whoever asked it | whoever asked it |

The initiator stays what 255 made it, the record of who asked - it is not the delegation: 255
deliberately rejected acting in the asker's name.

### Which question reads which

To be assigned one by one, as 255's plan did for `initiator_id`:

- **Ownership:** clearance (the owner's roles, narrowed by the personality's limiting roles),
  admission into a session (221), whose approval rules and approvers apply (13, 14), who may see an
  agent's private notes and memory (MEMORY.md).
- **Delegation:** the rights check in the policy (`OwnershipSecurityManager::check`), the store
  filters, the owner of what is created, the credentials of a call (`ReadSecret`).
- **Identity:** what is caller-bound already - `Publish`, `ModifyMessage`, `RevokeMessage` - and the
  record.

Open: whose clearance applies when delegation and ownership differ - both, as 142 intersects an
owner's and a personality's roles today, or only the owner's? 340 asks the same from the team's
side.

### Shape

- An `owner_id` beside `principal_id`, both set by `next`; `principal_id` keeps its meaning of
  "whose rights", which is what delegation is. For a user both are the user, or the owner is `None`.
  Cheap, and the policy stays as it is.
- Or renamed to say what they are (`acting_for`, `owned_by`), so that no reader keeps the old
  reading by accident. Touches every reader once, which is also the review the separation needs.
- `roles` are the principal's today. With the two apart, the context may need the owner's roles
  for clearance and the delegate's for rights.

### Where delegation comes from

As long as it equals the owner, nothing is stored. Once it can differ, something has to say for
whom an agent acts: the agent (a team's agent), the session it works in, or the template. That is
340's question "who chooses the hat", and need not be answered here: this issue only makes room for
the answer.

## Done when

- `SecurityContext` carries identity, ownership and delegation apart, documented in SECURITY.md.
- Every reader of `principal_id` is assigned to one of them; clearance and admission read the
  owner.
- Behaviour is unchanged while delegation equals ownership, and the security suite shows it.

## History

- **2026-10-13** — Opened at the maintainer's request, from the discussion of
  [221](221-adding-session-participants.md): an agent is to match a session's admission rule by its
  owner, which today is `principal_id` only because an agent always acts for its owner. Points
  towards [340](340-teams-as-owners.md).
