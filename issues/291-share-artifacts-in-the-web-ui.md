# 291 — An artifact's owner cannot see or change whom it is shared with in the web UI

**Status:** done
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
- **2026-10-10** — Done, decided with the maintainer before it was built:
  - First a step of its own without changes of behaviour: what is shown in the place of a channel
    is one `Page` in `Layout`, not a selected workspace and a selected skill side by side
    (`onSelectPage` replaces `onSelectSkill`); the sidebar's lists are one `PageList` with
    `usePageList`; `AccessList` is a component of its own.
  - `ArtifactPage`: the artifact, its owner's menu - describe, keep, delete -, its list as a whole
    (`AccessList`) and its versions, newest first, each with its download and, for the owner, whom
    it reaches besides. A version's own list is shown, not changed: nothing asked for it, and REST
    does not offer it. The sidebar lists the artifacts `GET /artifacts` lists, with their trash;
    the card in the chat opens the page ("Open") for whoever may read the artifact.
  - **Adding entries**: the members of a session the user may read, or of one of its channels -
    for workspaces and artifacts alike, in `AccessList`. Users and roles are not offered, since
    the REST API lists neither; that is issue
    [298](298-pick-users-and-roles-to-share-with.md).
  - **Where and by whom**, by name, comes from the server, not from what the UI knows of the
    user's sessions: `ArtifactView::snapshot` with `Detail::PUBLICATIONS` reaches each version's
    channel as every channel is reached (`Views::channel_of`) and names it, its session and the
    publisher only for whoever may read it - a version shared beyond its channel names nothing of
    it (security suite). `GET /artifacts/{id}` and what answers like it carry
    `session_name`, `channel_name` and `published_by_name`; lists do not.
  - A page is closed when its list loses the entry, no longer when the list lacks it: an artifact
    read only as a member of a channel is not listed, yet opened from the chat.
  - The card said "Kept in your artifacts." to every reader of a kept artifact since issue 23
    lets readers fetch it; it says so to its owner alone now.
