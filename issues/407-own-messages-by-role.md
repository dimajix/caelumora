# 407 — The web UI shows every user's message as one's own

**Status:** done
**Type:** bug
**Priority:** medium
**Area:** User interaction
**Summary:** The web UI puts a message on the right, in the colour of one's own, whenever its role is `User`, so in a channel with several people the messages of all of them look like the viewer's own.

## The problem

`MessageView` chooses how a message looks by its role: `msg.role === 'User'` puts it on the right,
in blue, with the text inverted. The chat view does the same for a message that was not let in and
for one taken back. Every person's message has the role `User`, so in a channel shared by several
users all their messages stand on the right as if the viewer had written them; only agents' stand
on the left.

Whose a message is, is known: it carries its `sender_id`, the chat view knows the user, and issue
[404](404-unread-and-mentions-in-overview.md) added `isOthers` for counting what is new. The same
comparison is written out twice more in the chat view, for whether a message may be changed or
reclassified by its author.

## Why it matters

A shared channel is what sessions and channels are for; that one cannot tell at a glance what one
said oneself from what someone else said makes the conversation hard to follow, and it suggests
that the messages could be changed like one's own.

## Considerations

*   **What decides:** whether the message was said by the viewer - its `sender_id` is the user's
    actor - not its role. A message without a sender is not one's own.
*   **Where it is decided:** in one place. `MessageView` is given the viewer and decides itself,
    rather than every caller computing a flag; the chat view's other checks for "said by me" use
    the same function.
*   **Others' messages** stand on the left in the neutral colour, as agents' do; the sender's name
    above every message tells them apart. Whether people and agents should look different is a
    question of its own.
*   **Line breaks** stay a matter of the role: what a person types keeps its line breaks, whoever
    typed it.
*   **The context and history panels** show messages at full width; there only the colour follows,
    by the same rule.

## Done when

*   Only the viewer's own messages stand on the right in the colour of one's own, in the chat and
    in the panels; others' stand on the left.
*   "Said by me" is decided by one function, used for the look, for changing and reclassifying,
    and by `isOthers`.

## History

- **2026-10-17** — Opened after noticing that in a channel with several users every message of a
  person is shown as the viewer's own.
- **2026-10-17** — Done: `isOwn` in `messages.ts` says whether a message was said by the viewer,
  and `isOthers` is built on it. `MessageView` is given the viewer (`me`) and chooses side and
  colour by it, the chat view does the same for a message refused or taken back and uses `isOwn`
  for changing and reclassifying; the context and history panels pass the viewer on.
