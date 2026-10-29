# 503 — The event bus asks the policy of every event for every stream

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Identity and access management
**Summary:** The event bus decides every event for every open stream of `GET /events` by a full policy check, reading a group from the store each time, so its cost grows with events times streams although an event reaches only a few of them.

## The problem

The dispatcher of the `EventBus` (`start_dispatcher`, `caelumora-runtime/src/event.rs`) goes
through every party made by `subscribe_as` - one per open stream of `GET /events` - for every event
published, and asks `may_reach`: the `AccessPolicy`'s `check` and the clearance. For the
`OwnershipPolicy`, `check` is decided by an `Admission` (issue
[332](332-policy-as-a-predicate.md)), and where nothing but a group an entry names may admit, it
reads that group from the store (`StoreMemberships::group`).

A channel is read by its owner and its group, so every event of a channel - every message stored
(`channel::SAID`), a join, a leave, an agent's state - reads the channel's group once for every
stream but the owner's; those who are no member read it too, since nothing else admits them. The
same group is read as often as there are streams, one after the other on the dispatcher's one task,
and every event behind it waits. Most events reach a handful of readers among all streams.

## Why it matters

- The cost grows with events × streams × a store read: a file read with the JSON store, a round
  trip with Postgres. With 20 users logged in, a conversation of agents of 100 messages reads the
  same group some 2 000 times (estimated, not measured).
- The dispatcher's queue is unbounded: when it falls behind, memory grows and nothing tells.
- It is the main cost issue [287](287-memberships-cache.md) names; a cache of memberships would make
  each question cheaper, not fewer.

## Considerations

- **The admission of a party, once.** The admission the `OwnershipPolicy` gives depends on the
  context and the verb, not on the kind. The bus can take it for `Get` and `HearDeleted` when a
  party subscribes, and decide an event by it: owner, actors, roles and everyone in memory, each
  group the event's attributes name read once per event and asked of every party, the clearance as
  now. A policy that gives no admission (`None`) - the system, `AllowAllPolicy`, an external policy
  ([24](24-opa-security-manager.md)) - is asked party by party as today.
  - It needs `Admission::admits`, `pub(crate)` in `caelumora-model`, with memberships kept for
    one event - or the policy deciding several contexts at once.
  - Issue 332 decided the admission for lists alone (decision 8); deciding a single question by
    it is the same rule (332, decision 2: `check` is decided by it), and a test of the bus can hold
    both together.
- **The readers of an event, indexed.** Beyond that, parties indexed by their actors and roles:
  an event goes to the owner's and the named actors' parties, those with a named role, the members
  of a named group, all for an entry of everyone - then the check as above. The cost follows the
  event's readers, not the streams. Worth it only with many streams; the first point comes first.
- **The event as `Arc<GlobalEvent>`.** Every party and every topic subscriber gets a clone of the
  event (`tx.send(event.clone())`), and an event holds JSON values - a `SAID` the whole message.
  One allocation per event instead.
- **Then 287.** With a group read once per event, the bus no longer needs a cache of memberships;
  287 would be about single reads by the views alone, and back to low.

## Done when

- The bus reads every group an event names at most once per event, a test shows it; a policy
  without admission is asked as before.
- A test shows the bus delivering to the same parties as `may_reach` decides, for owners, actors,
  roles, groups, deleted groups and clearances.
- `docs/EVENTS.md` says how the bus decides who receives an event.

## History

- **2026-10-29** — Opened from a review of the event bus with the maintainer, following the
  reassessment of issue 287 the same day.
- **2026-10-29** — Decided with the maintainer: the bus asks one question of all parties at once,
  rather than taking the admission of each party when it subscribes:
  `AccessPolicy::check_each(contexts, action)`, context by context by default, beside `check`, as
  `admission` is; and `confidentiality::may_reach_each` beside `may_reach`. The ownership policy
  decides a verb on entities for every context by its admission, over memberships read once for
  the question (`ReadOnce`, private, living for that one call - no cache, so nothing to go stale);
  any other action context by context. `Arc<GlobalEvent>` is a second step of this issue. Not
  part of it: the index of the parties by actor and role, and the unbounded queue of the
  dispatcher.
- **2026-10-29** — Step 1 done, branch `issue-503-bus-asks-every-stream`: the dispatcher asks
  `may_reach_each` once per event, and of an event about a part `check_each` with the part's
  attributes. The `Recorder` of the security suite keeps the default `check_each`, so that every
  question is still recorded. Tests: `check_each` answers every context as `check` does - owner,
  member, member of a deleted group, role, a member's agent, stranger, the system; reading and
  being told of a deletion and changing - and reads each named group once (failing with the
  memberships asked directly); `may_reach_each` answers as `may_reach`, clearances included; the
  bus reads a channel's group once for six parties.
- **2026-10-29** — After the review of step 1: a group is read once per question, not once per
  event as *Done when* says - an event about a part asks twice, the whole and the part, so a
  version of an artifact published reads the channel's and the project's group twice, both named
  in the artifact's list and the version's. Sharing the reads across both questions would change
  the agreed interface; left as it is unless the maintainer decides otherwise. Two differences
  from asking context by context, both refusing rather than granting: a group changed during the
  question is answered for every context as first read, and a group whose reading failed
  (`StoreMemberships` answers `None`) admits nobody for the rest of the question.
