# 408 — Who joins or leaves a channel is not recorded in its history

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** When someone is added to a channel or leaves it, neither its history nor the agents' transcripts record it, so the chat does not show who added whom, and an agent learns of the change only from the roster of its next system prompt, without knowing that something changed.

## The problem

Adding a participant to a channel and removing one are operations of the channel
(`ChannelLoop::add_participant`, `join`, `remove_participant`, issue
[46](46-channel-participants.md); taking someone into the session and the channel at once, issue
[221](221-adding-session-participants.md)). What others learn of them today:

*   **The event bus** announces the join or leave (`UserChannelJoined`/`Left`,
    `AgentChannelJoined`/`Left`); the web UI fetches the channel again and its member list follows.
*   **In the channel**, a removal is announced as `ParticipantRemoved`, in the name of whoever
    removed the participant - "shown to users and never written to the history". A join is not
    announced in the channel at all.
*   **An agent** sees the members of its channel in the roster of its system prompt
    (`channel_members_prompt_provider`, read at the start of every turn). It sees who is there now,
    not that someone just came or went, nor who brought them.

So the conversation itself has no trace of it:

*   In the chat, a new participant's first message appears without anything saying that they were
    added, by whom, or when. A reload shows the same - nothing was stored. A participant removed
    simply stops answering.
*   An agent does not react to a change: it is not told that someone new is there to be greeted or
    briefed, or that someone it was working with is gone. The roster only changes silently between
    two of its turns - and a roster that changes in the system prompt reads to a model as if it had
    always been so.
*   Afterwards, nobody can read from the history when someone joined or left. The former members
    are known with who removed them and when (issue [164](164-soft-deletion-and-former-members.md)),
    but not where in the conversation that happened.

## What is wanted

*   **The history records it**, in the order it happened: "@alice has added @bob to the channel",
    "@alice has removed @bob from the channel" - and for one who joined by themselves (`join`,
    through a `Join` entry of the access list) "@bob has joined the channel"; for one the runtime
    removed (an agent deleted, issue 164, in the name of `System`) "@bob was removed from the
    channel". Should a participant ever be able to leave by themselves - nothing offers that today -
    "@bob has left the channel".
*   **The web UI shows it** as a line of its own in the chat, at its place in the history, live and
    after a reload.
*   **An agent is given it** as part of the conversation: the entry goes into its history and its
    transcript, so that the model sees it in its next turn and can react - greet the newcomer, pick
    up what the one who left was doing. It begins no turn: like a message heard, it is taken in and
    waits for the next one.
*   Users and agents alike - who is added or removed, and who does it, may be either.

## Considerations

### How the entry is kept

*   **(a) An entry of its own** - `HistoryEntry::Joined { participant, by, at }` and
    `HistoryEntry::Left { participant, by, at }` (or one `MembershipChanged` with a kind), written
    into the channel's history by its one writer, and into each agent's history and transcript as it
    hears it. The transcript gets a matching `TranscriptEntry`, which the `ContextResolver` turns into
    what the model is given, as it applies the sender marking: the wording is not stored, and can
    follow `sender_format`/`sender_style` and the channel's language. The web UI renders it as a
    notice, as it renders a turn given up or a finished activity.
*   **(b) A message of `System`** with the text in it, stored as `Said`. Nothing new in the history,
    the UI shows it as a message. But the wording is frozen in English into the store, the UI cannot
    tell it from a message to style it, it would count as unread and could be retried, modified or
    taken back like a message - and a `system` message is what an agent hears nothing of
    (`Message::as_heard`), so it would not reach the model without an exception.

Recommended: (a). It follows how the history already records what is not a message (`TurnGivenUp`,
`ActivityFinished`, `CommandExecuted`), and keeps the wording where the model's input is built.

### What the model is given

*   **Not a `system` message.** Models move a system message in the middle of the conversation into
    the system prompt (issue [49](49-plan-step-system-messages.md)). The notice is given as a `user`
    message, marked as the runtime's and not a participant's, so that it cannot be mistaken for -
    or imitated by - a participant writing "@alice has added @mallory" (issue
    [150](150-impersonation-towards-the-agent.md)).
*   **Not through the perceive processors**, which work on what others said; the notice is the
    channel's own, appended as it is, like a message given (`Turn::give`).
*   **Names as they were.** Like a message, the entry keeps the handles and names of the participant
    and of whoever acted as of the change, beside their ids, so that a stored transcript reads the
    same after a renaming.
*   **An external agent** is told by the same rule, with its next call.
*   The roster in the system prompt stays as it is: it says who is there; the notice says what
    changed.

### Where in the history

*   **A join** is recorded before the new actor starts, after the position where its view of the
    history begins is fixed: the newcomer sees its own join as the first thing it hears ("@alice has
    added you"), whatever `HistoryAccess` it was given. Taking someone into the session and the
    channel at once records only the channel's join; joining the session alone records nothing in
    any channel.
*   **A removal** is recorded where `ParticipantRemoved` is sent today: once the actor's task has
    ended, after what it still sent. Whether `ParticipantRemoved` is then still needed as a transient
    event, or the stored entry takes its place for the frontends, is to be decided with the
    implementation.
*   **An agent deleted and restored** (issue 164) leaves and rejoins its channels in the runtime's
    name; both are recorded as such. A resting agent removed from a channel changes the document
    alone and is not running - whether that is recorded, and by whom, if the channel does not run,
    is open.
*   **A channel that does not run.** Changing the participants is done through the running channel
    today; if that changes, the entry has to be written by whoever writes the history then.

### Who may see it

*   Who is a member is visible to every member already (the member list, the former members). The
    entry is classified at the channel's floor, not above any message, so that it is seen by
    everyone who reads the channel; what it names is no more than the member list does.
*   It is subject to each participant's history boundary like everything else: a newcomer given no
    history does not see who joined before them.
*   It is not unread in the sense of issue [404](404-unread-and-mentions-in-overview.md) and
    mentions nobody: the `@bob` in it is a name, not an address, and asks no agent to respond.
*   `read_history` gives it, so that an agent reading another channel sees who came and went.

### Relation to other issues

*   [342](342-invitations.md) - invitations: once someone can accept or decline, "@bob has accepted
    the invitation" may become one more such entry.
*   [140](140-audit-log.md) - the audit log records the act for the auditor; this records it in the
    conversation for those who take part. Neither replaces the other.
*   [124](124-observer-participants.md) - an observer joining would, by this rule, be recorded too;
    whether watching should be announced is a question of that issue.

## Done when

*   Adding, joining and removing a participant writes an entry into the channel's history, naming
    the participant and whoever acted, at the place it happened.
*   The web UI shows it as "@alice has added @bob to the channel" (and the other forms above), live
    and after a reload; the REST API stays compatible with `caelumora-web-ui`.
*   Every agent in the channel has it in its history and transcript, and its model is given it in
    its next turn as a notice of the runtime; it begins no turn. An external agent is told the same.
*   A newcomer sees its own join, a removed agent's last messages come before its removal.
*   Tests cover the entries' order with the messages around them, what a newcomer with no history
    sees, and what the model is given.
*   CHANNELS.md, AGENT-TURN.md and WEB-UI.md describe it.

## History

- **2026-10-17** — Issue created.
- **2026-10-17** — Decided with the maintainer: a change of membership is told in the channel as
  a pair of `ActorEvent`s, `ParticipantAdded { participant }` beside the existing
  `ParticipantRemoved { participant }`, sent by the channel itself in the name of whoever acted -
  the same pattern as `ActivityFinished` or `TurnGivenUp`: the event is the live form, the channel
  records it in its history (`HistoryEntry::ParticipantAdded`, `HistoryEntry::ParticipantRemoved`),
  and these two are what the web UI shows. This settles option (a) and the open question about
  `ParticipantRemoved`: it stays, and is now stored. No kinds for "joined" or "left": the wording
  follows from the sender and the participant - another sender "has added"/"has removed", the
  participant itself "has joined"/"has left", `System` "was removed" -, in the web UI as in the
  `ContextResolver`. What changes against today:
  - Both events reach the agents as well, not only the users: into their history and transcript,
    beginning no turn. "Never written to the history" no longer holds for `ParticipantRemoved`.
  - The stored entry carries the time and the handles and names of the participant and of whoever
    acted, as of the change, beside their ids; who acted is the sender of the envelope.
  - `ParticipantRemoved` keeps its other role - the frontend ends what the removed actor left under
    way - and its moment: once the actor's task has ended, after what it still sent.
    `ParticipantAdded` is sent once the newcomer's history boundary is fixed and before its actor
    starts, so that the newcomer hears it first.
  - The event bus's `UserChannelJoined`/`Left`, `AgentChannelJoined`/`Left` stay, for subscribers
    outside the channel - a user's list of channels, say.
- **2026-10-17** — Naming settled with the maintainer, deviating from the entry before: what
  happened to someone's taking part is named *joined* and *left*, whoever caused it - who did is
  the sender or `by` -, as the event bus (`channel::JOINED`, `channel::LEFT`) and the views
  (`FormerParticipant.left`) already say; *added* and *removed* stay the names of the acts
  (`add_participant`, `remove_participant`). No payload type of its own: "membership" names groups
  in this code base. The naming that is still uneven - participant and member, the session's
  `ParticipantAdded` - is issue [409](409-participant-and-member.md).
- **2026-10-17** — Implemented. `ActorEvent::ParticipantJoined` and `ParticipantLeft` (the latter
  in place of `ParticipantRemoved`) carry the participant and its name; the channel announces them
  itself (`ChannelLoop::announce`) in the name of whoever made the change - a join as the
  newcomer's actor starts, a leave once its task has ended - and `admission` refuses them from any
  actor (`ActorEvent::is_announcement`). The history records them as
  `HistoryEntry::ParticipantJoined`/`ParticipantLeft { participant, name, by, by_name, at }`
  (`Envelope::announced`); the moderator tells everybody, also whose name it is told in, which
  `route_event_impl` no longer holds back from its sender. Agents note them into history and
  transcript without beginning a turn (`AgentChannelState::note_announced`), also as they catch up
  after joining again, in order with what was said; the `ContextResolver` gives the model "Channel
  notice: @alice added @bob to the channel." and the other forms, the agent itself as "you", and
  an external agent's remote agent is given the same (`ModelProfile::given_messages`). REST:
  `participant_joined`/`participant_left` items; the web UI shows them as a line of their own in
  the chat and in an agent's history panel, the TUI prints them. Not done: a resting agent removed
  from a channel is not announced (nothing of it runs); `read_history` still gives messages alone;
  an external agent with a `changes_only` roster is told a join twice - by the roster's "Since you
  were last told who is here" and by the notice (issue [410](410-roster-changes-told-twice.md)).
  Stored data need not be deleted: the entries are new variants. Status set to done.
