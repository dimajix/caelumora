# 289 — Attaching a workspace shares it with one session at a time, however its list already reaches

**Status:** rejected
**Area:** Identity and access management
**Summary:** Attaching a workspace to a session shares it with that one session by an entry of its own, even where its access list already reaches everyone the session may take in.

## The problem

A workspace of a user's, attached to a session, shares nothing by itself (issue
[23](23-acls-in-security-attributes.md), decision 7): its owner shares it with the session's
members by an entry `MemberOf(session)` in its access list, offered where it is attached. Every
session it is attached to adds one such entry. A workspace used in many sessions carries a long
list of entries for single sessions - many of them long over, all of them to be read and cleaned
up by hand.

Often none of them is needed: the workspace's list already reaches everyone the session may take
in. If the workspace is granted to a role, and the session admits only holders of that role
(decision 5, the admission rule), every member of the session can already reach it.

## Why it matters

- Lists that grow by one entry per session are what makes access lists unreadable, and what nobody
  cleans up.
- The owner should be told whether attaching changes who may reach the workspace at all - and not
  be asked to add an entry that adds nothing.

## Considerations

- **Compare the two rules when attaching**: the session's admission rule (who may be taken in) and
  its current members against the workspace's access list. If every subject the session admits is
  covered by the workspace's list - the same role, a group containing them (issue
  [286](286-groups-as-subjects.md)), each member named -, the UI says so and offers no new entry.
  If not, it says who would not reach the workspace, and offers `MemberOf(session)`, or an entry
  for what is missing.
- **Covered is a question of the subjects, not of the people**: a role covers a role; whether a
  member happens to hold it today is weaker, since membership and roles change. Which comparison
  counts is to be decided - by subjects only, or by the members as they are with a warning.
- **The same for a session that changes**: a session whose admission is widened later, or that
  takes in someone the workspace does not reach, could say so where the workspace is attached.
- **Where it lives**: a question the views answer for the owner (`SessionView`, `WorkspaceView`),
  shown in the attach dialog of the web UI; the policy decides nothing new.

## Done when

- Attaching a workspace tells its owner whether the session's members already reach it, and offers
  an entry only where they do not.
- WEB-UI.md and SECURITY.md describe it.

## History

- **2026-10-10** — Opened at the maintainer's request while step 2 of issue 23 was planned:
  attaching offers an entry `MemberOf(session)` for now; comparing the rules is to spare lists of
  single sessions.
- **2026-10-11** — With groups (issue [286](286-groups-as-subjects.md)) the entry attaching
  offers is `Group(session)`, the group of the session's participants, which has the session's id.
  That does not resolve this issue, as the plan of issue 304 assumed: every attachment still adds
  an entry of its own. Comparing the rules stays to be done; a team that holds the session's
  members is now one more subject that may already cover them.
- **2026-10-14** — Rejected. Its premise went with issue
  [221](221-adding-session-participants.md): whom a session takes in is no longer bounded by an
  admission rule - whoever may change it takes in anyone they may name -, so no entry covers its
  future members but `Everyone` or the session's own group. Comparing with its members as they are
  would spare the entry only by trading one that follows who takes part (issue
  [286](286-groups-as-subjects.md)) for a coincidence of today. What makes the lists grow - entries
  for sessions long purged - is solved by issue [347](347-entries-naming-purged-groups.md), which
  drops them as a list is next stored.
