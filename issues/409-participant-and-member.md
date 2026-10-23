# 409 — Taking part in a channel is named "participant" in some places and "member" in others

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** Who takes part in a session or channel is a participant in the model and its operations, but a member in the views, the REST API and the web UI, and "member" also names who belongs to a group; and that taking part began or ended is told as joined/left on the event bus but as added on the session.

## The problem

One concept has two names, and one name has two concepts.

**Participant.** The model and the operations say participant: `channel::Participant`,
`FormerParticipant`, `Channel::participants`, `add_participant`, `remove_participant`,
`SessionUpdateEvent::ParticipantAdded`, `ActorEvent::ParticipantJoined`/`Left` (issue
[408](408-membership-changes-in-history.md)), `ResolvedParticipant`, `Participation`.

**Member, for the same thing.** The views and everything built on them say member:
`ChannelMember`, `MemberView`, `MemberSnapshot`, `ChannelMemberResponse`, the fields `members` and
`former_members` of a channel response, `channel_members_prompt_provider`, "Former members" in the
web UI and in CHANNELS.md, WEB-UI.md, REST-SERVER.md and ARCHITECTURE.md.

**Member, for something else.** Who belongs to a group is a member as well: `Memberships`,
`members_of`, `GroupUpdateEvent::MembersChanged`, `GroupMemberResponse`. The participants of a
session or a channel are kept as such a group (issue [286](286-groups-as-subjects.md)), so both
meanings meet in the same code - "the members of the channel's group" are its participants, but
`ChannelMember` is not a member of that group in the sense of `Memberships`.

**Joined and left beside added.** Issue 408 settled the rule: what happened to someone's taking part
is told as *joined* or *left*, whoever caused it - who did is the sender or `by` - as the event bus
already does (`channel::JOINED`, `channel::LEFT`) and as the views keep it
(`FormerParticipant.left`, `MemberView::left`). *Added* and *removed* name an act of someone on a
document: `add_participant`, `remove_participant`. `SessionUpdateEvent::ParticipantAdded` sits
between the two: it is an update event of the session's document, but it is also how the session
tells that someone joined it - by themselves, too (`SessionLoop::join`).

## Why it matters

*   AGENTS.md: "The same concept has the same name everywhere." Whoever reads `ChannelMember` next
    to `Participant` has to find out that they are the same, and whoever reads `members` next to
    `Memberships` that they are not.
*   The names reach the REST API and the web UI, where renaming later costs more.

## Considerations

*   **Participant everywhere for taking part**, member for groups alone: `ChannelMember` becomes
    `ChannelParticipant` (or the runtime's view type takes the name), `MemberView` and
    `MemberSnapshot` likewise, the REST fields `participants` and `former_participants`, the web UI
    "Participants" and "Former participants". The largest change - it reaches the REST API, the web
    UI and the TUI -, and it leaves "member" with one meaning.
*   **Member everywhere for taking part**: fewer places in the model to rename, but the clash with
    groups stays and gets worse.
*   **Leave the views as they are**, and say in the documents that a member is a participant as
    seen by a viewer. Cheapest; the clash with groups stays.
*   `SessionUpdateEvent::ParticipantAdded`: keep it as an update of the document, or make it
    `ParticipantJoined` as the channel's event, so that a session and a channel tell the same
    thing in the same word. Whether the session also tells a leave is open - nobody can leave a
    session today.

Recommended, to be decided: participant for taking part throughout, member for groups alone, and
`ParticipantJoined` for the session as well. Since stored data may be deleted for now, stored
names need no compatibility; the REST API and the web UI change together.

## Done when

*   One word names taking part in a session or channel, in the model, the runtime, the REST API, the
    web UI, the TUI and the documents, and "member" is left for groups, or the decision says why
    not.
*   The session and the channel tell that someone joined in the same word.

## History

- **2026-10-17** — Issue created, from the naming discussion of issue
  [408](408-membership-changes-in-history.md), which settled joined/left for what happens to
  someone's taking part and added/removed for the acts on a document.
- **2026-10-17** — Decided with the maintainer: participant for taking part throughout, in the
  model, the runtime, the REST API, the web UI, the TUI and the documents; member for groups alone -
  and for the members of a scope (`ScopeMember`), a third meaning left as it is. The session tells a
  join as the channel does: `SessionUpdateEvent::ParticipantJoined`.
- **2026-10-17** — Done. Renamed: `ChannelMember` to `ChannelParticipant`, `MemberView` to
  `ParticipantView` (module `view/participant.rs`, with `ParticipantState`, `AgentParticipant`,
  `UserParticipant`, `ApproverParticipant`, `GoneParticipant`), `MemberSnapshot` to
  `ChannelParticipantSnapshot` (`ParticipantSnapshot` is the session's), `ChannelMemberResponse` to
  `ChannelParticipantResponse`, `ViewError::MemberNotFound` to `ParticipantNotFound`,
  `Detail::MEMBERS` to `PARTICIPANTS`; `members()`, `former_members()`, `member()` and
  `member_ids()` to `participants()`, `former_participants()`, `participant()` and
  `participant_ids()`; the channel response's and snapshot's `members` and `former_members` to
  `participants` and `former_participants`; `channel_members_prompt_provider` to
  `channel_participants_prompt_provider`; `SessionUpdateEvent::ParticipantAdded` to
  `ParticipantJoined`. The web UI follows with the REST API ("Former participants"), and the
  documents, comments and test names with the code. Left as group terms: `Memberships`,
  `members_of`, `GroupUpdateEvent::MembersChanged`, `GroupMemberResponse`,
  `/groups/{id}/members`, and wherever something is shared with or read by the members of a
  session's or channel's group. Stored data to delete: personalities that name the component kind
  `channel_members_prompt_provider` (the seed names the new one), and session journals holding a
  `ParticipantAdded`.
