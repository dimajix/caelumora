# 321 — A channel's components act in its owner's name, with the roles of when it was built

**Status:** done
**Area:** Identity and access management
**Summary:** A channel's components act in its owner's name with the roles of when the channel was built, a context nobody decided on, so what they may read or use depends on when the channel was last built.

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
- **2026-10-13** — Decided and done: components are built in their owner's name - a channel's
  components and a session's commands in the owner's, with the roles of when they are built, as an
  agent's are in the agent's -, and the name is the build's alone. What a component asks, it asks
  while it is built (a `ref`, a secret); none keeps the context, and what a channel's components
  do later - routing - they do in the context of the event, so the "per act" consideration was
  already how they work. A change of the owner's roles reaches them when they are built again, as
  it reaches an agent's components. An owner who cannot be found builds with no roles, as an agent
  whose owner is gone acts with none. The channel as an actor of its own was not taken: it would
  need rights given to channels, and nothing has any. Two things were done: whoever creates a
  channel - not necessarily its owner - names only preconfigured components they may read
  (`ComponentRegistry::require_readable`, asked before `CreateChannel`); and the owner's context
  is made in one place (`ControllerManager::owner_context`), which the session's commands now use
  too, so that they are built with the owner's roles, not without any. Recorded in
  PERMISSION-CHECKS.md under "Decided in issue 321". That a container builds components nobody
  asks for - a tool in a channel, built with its owner's secret - became issue
  [337](337-components-nobody-asks-for.md).
