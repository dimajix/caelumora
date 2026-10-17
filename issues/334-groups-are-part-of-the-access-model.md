# 334 — Groups are part of the access model, teams a feature beside it

**Status:** rejected
**Area:** Code structure
**Summary:** Groups are the participants of sessions and channels and the teams users make in one kind, which the policy reads, so only teams could become a plugin.

## The problem

Issue [323](323-kinds-enumerated-in-the-core.md) foresees an issue for groups as a plugin, as for
workspaces and artifacts. Groups (issue [286](286-groups-as-subjects.md)) are two things in one
kind:

- **The participants of a session or channel**, a group per session and per channel
  (`GroupId::of_session`, `of_channel`), kept by the core while participants join and leave
  (`group::keep_participants`, called by the session manager, the session's channels and the
  channel controller), and named in the access lists of sessions, channels and what lies in them.
- **Teams** a user makes (`Group::team`), kept by the `GroupManager`, managed through `/groups`
  and the web UI.

Both are `Group`, both are what `Subject::Group` in `SecurityAttributes` names, and the policy
reads both through `StoreMemberships`, which is bound to the store of groups when the runtime is
built. The scope rules delete a scope's groups last, apart from the members of the scope
(`ScopedEntities`).

## Why it matters

The policy - the core's - cannot decide an access list without knowing who belongs to a group.
Groups cannot leave the core without the access model leaving with them; only the part users
manage could.

## Considerations

- **A. Groups stay in the core, teams too.** The access model is the core's, and a team is a
  group like any other to it; the `GroupManager` and the routes are small. Nothing to do.
- **B. Teams as a plugin, participant groups in the core.** The plugin brings the `GroupManager`'s
  team operations, its routes and the web UI's part; the kind stays the core's, since the policy
  reads it. A plugin that owns operations on a kind it does not define - a pattern there is none
  of yet, and one with little gain.
- **C. Memberships as a facet.** `Memberships` answered by whoever registers it, so that the
  store of groups could be a plugin's. It moves the question, not the dependency: the policy
  still needs an answer, and the participant groups are kept by the core's sessions.

Recommendation: A - recorded so that the question is not asked again with every plugin. If it is
taken, the issue is closed as `rejected` with this reasoning.

## Done when

- The maintainer has decided, and the decision is recorded here and in `docs/SECURITY.md` (groups)
  if it changes what the document says.

## History

- **2026-10-12** — Opened as a follow-up of issue 323 (step 11): groups are a subject of the
  access model, and teams the only part of them a plugin could take.
- **2026-10-12** — Decided by the maintainer: A. Groups stay in the core, teams with them - the
  policy reads both, and a plugin that owns operations on a kind it does not define would be a
  pattern with little gain. Closed as `rejected`; nothing in `docs/SECURITY.md` changes.
