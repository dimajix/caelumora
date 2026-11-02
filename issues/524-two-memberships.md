# 524 — Two types are named `Memberships`: the groups' and a channel's participants'

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `caelumora_security::Memberships`, the groups an access list names, and `caelumora_store::history::Memberships`, who took part in a channel when, share a name, although "membership" was decided to be the groups' word and a channel's the participant's.

## The problem

- **`caelumora_security::Memberships`** (`caelumora-security/src/memberships.rs:14`) is the trait
  through which a policy learns of the group an access list names (issues
  [23](23-acls-in-security-attributes.md), [286](286-groups-as-subjects.md)), with `NoMemberships`
  and `StoreMemberships` (`caelumora-store/src/memberships.rs:19`).
- **`caelumora_store::history::Memberships`** (`caelumora-store/src/history.rs:118`) records who
  was a participant of a channel in which ranges of its chain (issue
  [417](417-user-history-from-its-actor.md)), derived from the chain and kept beside it.

Issue [517](517-the-model-offers-libraries-what-only-the-kernel-may-decide-by.md) noted the clash:
in different crates the two no longer collide, but one name means two things. Issues
[354](354-names-follow-several-schemes.md) and
[356](356-member-questions-threaded-with-their-services.md) decided that "membership" stays the
groups' word and a channel's is "participant" (`Participation`); the history's `Memberships`, from
issue 417, breaks that rule.

## Considerations

- **Rename the history's.** It follows the rule decided: a channel's word is "participant". Names:
  `Participations` - plural of one participation each, but a letter from the kernel's
  `Participation`, which owns the questions about participants -, or `Attendance`, a word of its
  own that reads apart from it. The type is also stored (kept beside the chain, by a database as
  well, issue [04](04-postgres-state-store.md)): whether a stored name or field changes with it is
  to be looked at; if so, the maintainer is told which data to delete.
- **Rename the groups'.** The trait answers only `group(id)`; a name such as `Groups` would say that
  more plainly. But "membership" would then be left to a channel's participants, against the rule
  decided, and the change touches the security policies and their construction.
- Its method names and doc comments follow the type renamed.

Recommended: rename the history's, to `Attendance` - the name of its own keeps it apart from
`Participation`.

## Done when

- One name means one concept: "membership" is the groups', and the history's record of who took
  part when has a name of its own, in code and in the documents.

## History

- **2026-11-02** — Opened at the maintainer's request, from the finding of issue
  [517](517-the-model-offers-libraries-what-only-the-kernel-may-decide-by.md).
- **2026-11-02** — Done: the history's `Memberships` is `Attendance` (`caelumora_store::history`),
  `of_member` is `of_participant`, `HistoryStore::memberships` is `HistoryStore::attendance`; what
  stores keep of it is named alike - `attendance.json` beside a history of the JSON files, the
  column `histories.attendance` in Postgres, changed in its migration `0002_states.sql`. Doc
  comments and the documents speak of a channel's attendance where they said membership. The
  stored histories of the JSON files and the Postgres schema are to be deleted. Left for later: that
  what went to all of a channel's participants is still what went to "every member" - in
  `Recipients::members`, its doc comments and the documents.
- **2026-11-02** — What was left is done too: what goes to all of a channel's participants is
  `Recipients::participants`, no longer `Recipients::members`, and doc comments, tests and the
  documents speak of a channel's participants where they said members. Members of a group, of a
  project and of a scope keep the word. No stored data changes.
