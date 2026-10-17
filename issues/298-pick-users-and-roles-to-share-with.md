# 298 — The web UI shares a workspace or artifact only with the members of a session or channel

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The web UI shares a workspace or an artifact only with the members of a session or channel, not with a user or a role by name, since the REST API lists neither.

## The problem

An access list (issue [23](23-acls-in-security-attributes.md)) names a user, a role, or the members
of a session or channel. The owner of a workspace or an artifact adds entries in the web UI with
`AccessList` (issue [291](291-share-artifacts-in-the-web-ui.md)), which offers only the members of
a session the user may read, or of one of its channels: those it can pick from `GET /sessions` and
`GET /sessions/{id}/channels`. A user or a role by name cannot be added, because the REST API lists
neither - `PUT …/access` accepts them, written by hand.

## Why it matters

- Sharing a result with one colleague who takes part in none of the owner's sessions - the most
  direct way to share - needs a hand-written request.
- A role ("everyone in finance") is the way to share with people who change over time until groups
  of one's own exist (issue [286](286-groups-as-subjects.md)).

## Considerations

- **Who may see whom**: a list of every user tells who works here, and who is called what. Whether
  an owner may look users up at all, by a prefix of their name only, or only among those they share
  a session with, is a decision of the security model, not of the UI - an `AccessQuestions` method,
  and a test in the security suite.
- **Roles** are the administrator's, in the configuration (`SecurityManager::role_name`); listing
  their names is a smaller question, but which roles a user may name in a list is the same one.
- **One picker**: `AccessList`'s for both pages, the session and channel choice kept beside it.
- Issue 286's groups would be a third kind of subject to pick, by the same picker.

## Done when

- An owner adds a user or a role to the list of a workspace or an artifact in the web UI, from
  what the server lets them find.
- SECURITY.md says who may find whom; WEB-UI.md describes the picker.

## History

- **2026-10-10** — Opened while building issue 291: its picker offers the members of a session or
  channel alone, decided with the maintainer, since nothing lists users or roles.
