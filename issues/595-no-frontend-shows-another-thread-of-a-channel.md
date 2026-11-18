# 595 — No frontend shows another thread of a channel

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The web UI and the terminal show a channel's line alone (issue 557): what a sub-agent says and does in its thread cannot be followed, nobody can write into it, wake it or stop it alone, and since neither a snapshot's participants nor the items of a history page say their thread, a frontend cannot even tell what of the line belongs to another thread.

## The problem

Step 6 of 2b of issue [557](557-a-conversation-cannot-go-on-two-ways-at-once.md) made the web UI
and the terminal show a channel's line: what happens in it and in the whole channel, and of
another thread its approvals, its questions and what mentions the viewer. A view of a thread was
left for later. Missing with it:

- **Following a thread.** What a sub-agent ([558](558-no-agent-hands-a-task-to-a-sub-agent.md))
  does - its messages, its calls, its plan - is read only over REST (`?thread=`).
- **Writing into a thread** - an open point of 558: whether users may write in a sub-agent's thread
  or it is a closed circle.
- **Waking or stopping one thread.** Cancelling stops every thread of a channel, and a thread
  opened after it starts awake (557, step 4); the frontends cannot cancel or wake one thread.
- **The thread of what is shown.** A participant in a snapshot carries no thread, so a sub-agent in
  a turn as a client connects is shown as answering in the line until its turn ends; the items of
  a history page carry no thread, so the terminal cannot tell a message of another thread that
  mentions the user from one of the line - and may take it as the answer `/retry` asks for again.
- **Calls in a message shown from another thread**: their results, which mention nobody, are left
  out, so such a call may stay shown as running.

## Considerations

- **Where a thread is shown**: beside the line, under the call that opened it, or as a channel of
  its own in the list.
- **The thread on what is given to frontends**: `ParticipantSnapshot::thread`, a `thread` on a
  history page item (the store's `KeptItem::thread` is there already), so that a frontend sorts by
  thread instead of guessing.
- **Writing into a thread**: who may, and whether the sub-agent is then asked to respond as by its
  delegate.

## Done when

- A frontend shows a thread of a channel, and the decisions on writing into it and on waking and
  stopping it alone are recorded here.
- What a frontend is given says its thread where it needs it.

## History

- **2026-11-18** — Opened from issue 557, step 6 of 2b, and 558's open points: the view of a thread
  left for later, and what its review found missing.
