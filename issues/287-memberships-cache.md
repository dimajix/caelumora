# 287 — Every check of a membership reads the store

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Identity and access management
**Summary:** Every check of a membership or a role in an access list reads the entity store, for every event of a session or channel and every subscriber.

## The problem

The ownership policy learns whether someone is a member of a session or channel, and which roles
count for them, from a `Memberships` source (issue [23](23-acls-in-security-attributes.md), step
1). `StoreMemberships` reads the entity store for every question: the session or channel named, a
user's roles, an agent and its personality.

A session and a channel are read by their members through an entry `MemberOf` themselves, so every
read of them asks: every view, and above all the event bus, which asks the policy for every event
of a session or channel and every subscriber - one or two store reads each. With the JSON store,
each is a file read.

## Why it matters

- Correctness came first; the cost grows with sessions, channels, subscribers and events, and the
  event bus pays it on its one dispatcher task, which delays every event behind it.
- With a database ([03](03-postgres-entity-store.md)) every question is a round trip.

## Considerations

- **A cache of its own implementing `Memberships`**, in front of `StoreMemberships`: who takes part
  in which session and channel, the roles of a user, the roles that count for an agent.
- **Kept in step by events**: a participant joining or leaving (`UserChannelJoined`, …Left,
  `AgentChannel…`), a session's participants, a user's roles changing (at a login; today no event
  says so - issue [181](181-where-roles-decide.md) noted it), an agent's personality changing.
  What no event announces cannot be cached, or only for a short time.
- **Bounded**: by size, least recently used first; a miss reads the store as today.
- **Several instances** ([122](122-several-instances.md)): a cache per process is stale where
  another process changed membership; the events between processes are 122's question.
- **No change of decisions**: a cache that answers differently from the store is a security bug,
  so the store suite of memberships runs against it as well.

## Done when

- A cached `Memberships` answers as `StoreMemberships` does, kept in step by events, bounded, and
  is what the binaries use.
- A test shows a joining and a leaving participant reflected at once, and a stale answer impossible
  for what events announce.

## History

- **2026-10-10** — Opened as issue 23 planned it (decision 6): a cache of memberships is an
  optimisation, apart from the correctness of step 1.
- **2026-10-11** — Since groups (issue [286](286-groups-as-subjects.md)) replaced `MemberOf`,
  a membership is read from the one group an entry names - a small document, never the session or
  channel itself (`StoreMemberships::members_of`). Every check still reads the store; what a cache
  would hold is now groups.
- **2026-10-29** — Reviewed with the maintainer after issue [332](332-policy-as-a-predicate.md):
  still a problem, narrower in what a cache holds, wider in what it costs. Priority raised from low
  to medium.
  - **What 332 did not change.** The admission reaches a store's scan alone (332, decision 8:
    lists only). Every decision on one entity - `check`, and so every view and the event bus -
    still asks `Admission::admits_members_of`, which reads the group through
    `StoreMemberships::group`, once a group an entry names is all that may admit. In a list, every
    candidate the storage hands out is decided so once more (`Access::readable`), even where
    Postgres admitted it by its join already.
  - **What a cache would hold now.** Groups alone: the roles that count come from the
    `SecurityContext` (`counting_roles`), not from the store, and no agent's personality is read.
    The events of *Considerations* - `UserChannelJoined`, a change of roles nobody announces - are
    gone; a group changes as an entity, announced by the events of its kind. A cache has to answer
    for a deleted group too, with the members it had: `HearDeleted` asks after a session's or
    channel's groups went (332, decision 3). A cache in front of `StoreMemberships` does not
    contradict 332, decision 4, which only rejected the groups in the `SecurityContext`.
  - **Where it costs.** The one place a group is read is `Memberships::group`, reached through the
    `OwnershipPolicy` alone. Groups are named in the access lists of sessions, channels, projects,
    workspaces and artifacts. The event bus's dispatcher (`caelumora-runtime/src/event.rs`) asks the
    policy of every event for *every* party, every open stream of `GET /events`, not only for
    those the event concerns. A channel is readable by its owner and its group, so for every event
    of a channel - every message stored (`channel::SAID`), a join, a leave, an agent's state - each
    party but the owner reads the channel's group, those who are no member most of all, since
    nothing else admits them and nothing stops early. That is a read per open stream per message,
    of the same group, one after the other on the dispatcher's one task: with 20 users logged in, a
    conversation of agents of 100 messages reads it some 2 000 times. Not measured yet.
  - **Kept in step how - to be decided.** *Considerations* proposed events. The event bus is itself
    the cache's heaviest user, and an event arrives later than the change it announces: in between,
    a participant who left could still receive the channel's events, which this issue counts as a
    security bug. The alternative is to drop an entry synchronously where a group is written - only
    the store of groups writes them -; where that hook lives is an architecture question for the
    maintainer. Either way a cache per process stays stale where another process changes a group,
    which remains [122](122-several-instances.md)'s question.
  - **The bus asks every party.** That every event is judged for every open stream, rather than
    for those who could receive it, is a cost of its own beside the read per question: a cache
    makes each question cheaper, not fewer. Recorded here as a finding, not yet an issue of its own.
  - **Who gains.** The memory store little, the JSON files and Postgres much - a file read or a
    round trip per question.
- **2026-10-29** — Issue [503](503-bus-asks-every-stream.md), step 1: the event bus asks one
  question of all parties at once, and the ownership policy reads a group the event names once for
  all of them. The main cost this issue named is gone; what is left are the single reads of the
  views and of the stores' lists, one group per question. Priority back to low.
