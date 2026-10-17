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
