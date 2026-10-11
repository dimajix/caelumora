# 321 — A channel's components act in its owner's name, with the roles of when it was built

**Status:** open
**Area:** Identity and access management

## The problem

A channel's components (issue 174) - its moderator and whatever else the channel is configured
with - are built in `ChannelLoop::new` with a context of the channel's owner: the owner's
`SecurityContext` as their user controller gives it, with the roles and clearances they have at
that moment, or a bare context of the owner where none is found. Everything a component asks of
the policy, the stores or the secrets, it asks in that name for as long as the channel runs.

This was found in the map of issue [304](304-review-of-the-permission-concept.md) (F18) and left
"to be decided when components are given their context"; issue
[312](312-order-and-reach-not-everywhere.md) moved it here, since it is a question of its own
rather than one of order or reach.

- **Whose act is it?** A moderator chooses who speaks next for everyone in the channel; it does not
  act for its owner the way an agent does. Whether its questions are the owner's, the channel's or
  the runtime's has not been decided - the agents' answer (they act on behalf of their owner) was
  taken over without being asked.
- **Roles of a moment.** The roles are those of when the channel was built - at its creation, at a
  restore or when its session was started. A change of the owner's roles reaches the components
  only when the channel is built again.
- **No owner found.** Where the owner's controller cannot be found, a bare context without roles
  is taken, so that the components are cleared for less than their owner is.

## Why it matters

What a component may read or use - secrets for a model, other channels, stored entities - is
decided by this context. A context nobody chose decides it silently, and differently depending on
when the channel was built.

## Considerations

- **The owner's, refreshed.** Keep the owner's name, but give the components the owner's context
  as it is now whenever they act, as an agent's turn does.
- **The channel's own.** Components act as the channel - an actor of their own, like the approver -
  with rights given to the channel rather than taken from its owner.
- **Per act.** Components are given no context when built, but the context of the act they serve:
  the turn, the message, the request; what they do on their own runs in the system's name and asks
  nothing.

## History

- **2026-10-11** — Opened from issue 312, which took it over from issue 304 (F18) and handed it on
  as a decision of its own.
