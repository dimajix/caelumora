# 39 — The global event stream is not filtered by what a user may see

**Status:** open
**Area:** Identity & access management

## The problem

Since [11](11-agent-ownership.md) the global stream's snapshot lists only
the sessions a user may read, and `GET /sessions/{id}` answers `404` for the
others. The events that follow are still sent to everybody:
`SessionCreated`, `ChannelCreated` and the rest name sessions a user may not
see, and `SessionUpdated(NameChanged)` carries the new name of a session the
user has no access to.

The web UI does not show them - fetching such a session answers `404`, which
it ignores, and a rename only applies to a session already in its list - but
the ids and names go over the wire to anyone logged in.

## Why it matters

A session's name is chosen by its owner and can say what it is about. Ids
alone are less telling, but they let a user learn how many sessions exist
and when others are active. Both are exactly what the ownership policy is
meant to keep to the people involved.

## Considerations

- **Filter in the stream, per user.** For each event, ask the
  `SecurityManager` whether the user may read the session it names, with
  the session's attributes. That needs the attributes at hand: a running
  session has them in memory, a stopped one in the store - a store read per
  event and subscriber would be too much, so a small cache of attributes by
  session, updated from the same events, is likely needed.
- **Events about a session one loses access to** (being removed as a
  contributor, once that is possible) should end with something that tells
  the client to drop it, e.g. a `SessionDeleted` from that user's point of
  view, or simply a fresh snapshot.
- **Events that name no session** (`UserCreated` and the like) need a rule
  of their own; today nothing in the web UI uses them.

## Done when

- A user's global stream carries only events about sessions they may read.
- The web UI's session list still follows everything it shows.

## History

- **2026-09-24** — Opened while implementing [11](11-agent-ownership.md),
  which filtered the snapshot but not the events.
