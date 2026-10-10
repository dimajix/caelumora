# 291 — An artifact's owner cannot see or change whom it is shared with in the web UI

**Status:** open
**Area:** User interaction

## The problem

With issue [23](23-acls-in-security-attributes.md), step 3, an artifact is reached by access lists:
its own, for the whole of it, and one on each version, which publishing gives the members of the
channel it is published in. Its owner sets the artifact's list over REST
(`PUT /artifacts/{id}/access`). The web UI offers none of it: an artifact appears only as a card at
the message that published it - name, version, size, a download, "Keep" -, and there is no page of
an artifact at all, nor a list of the user's artifacts.

## Why it matters

- Sharing a result beyond the channel it was published in - with a colleague, with the members of
  another session - is what an artifact is kept for, and its owner can do it only by hand-written
  requests.
- Who may read which version - drafts kept to a private channel, the final one to the team - is
  invisible: the owner cannot tell what they shared.

## Considerations

- **Where**: a page of an artifact, in the place of a channel as a workspace's (`WorkspacePage`),
  reached from its card and from a list of the user's artifacts in the sidebar (their own, and
  those shared with them, as the workspaces are listed). The card stays as it is.
- **What it shows**: its versions, each with where it was published and who may read it - by the
  artifact's list or its own -, the downloads of those the viewer may read; for its owner the
  artifact's list (`AccessList`, as the workspace page has it) to change or take away entries.
- **Adding entries**: a user, a role, the members of a session or channel - which the workspace's
  page cannot do yet either; one way of picking a subject for both.
- **A version's own list**: changing it after publishing is not offered over REST yet (issue 23,
  step 3); whether the page needs it - sharing one version further without the others - decides
  whether it is added.

## Done when

- An artifact has a page in the web UI with its versions and, for its owner, whom it is shared with,
  changeable; the user's artifacts are listed.
- WEB-UI.md describes it.

## History

- **2026-10-10** — Opened at the maintainer's request with step 3 of issue 23: artifacts are shared
  over REST; the web UI follows here.
