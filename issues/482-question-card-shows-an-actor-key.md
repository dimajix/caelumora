# 482 — A question waits for an actor key instead of a name

**Status:** done
**Type:** bug
**Priority:** low
**Area:** User interaction
**Summary:** When an actor a question is put to is no participant of the channel - one that has left, say -, the question card says it waits for `User:<uuid>` instead of a name.

## The problem

`ChatView` gives the `QuestionCard` the names of whoever a question is put to, found among the
channel's participants; for one it does not find, it falls back to `actorKey(id)`, the key the UI
tells actors apart by (`ChatView.tsx:1813`). The card then reads "Waiting for User:1f3e… to
answer".

## Why it matters

The key is an internal of the client (`actor.ts`, issue
[474](474-web-ui-repeats-the-user-test.md)), never meant to be read; the history shows an issuer it
cannot name as "someone" (`HistoryPanel.tsx:201`).

## Considerations

- **A. Name the unknown as the history does**, "someone".
- **B. Have the server send the names** with the question, as it does with a participant's join
  (`ParticipantJoined.name`), so that one who left is still named.

## Done when

- A question card never shows an actor's key.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 474.
- **2026-10-28** — Looking into it showed that one asked who left can no longer answer: their
  actor no longer runs in the channel, and the agent, which takes only answers and decisions while
  it waits, never saw that they left - a turn whose questions were put to them alone waited until
  it was cancelled. The maintainer chose to name only those asked who still take part, neither A
  nor B, and to fix the waiting here too. Leaving is no answer and no decline: declining in the
  name of whoever left - by the agent, or by the user's actor as it leaves, as a message declines -
  was weighed and rejected, since it would record a decline nobody gave. The card names those asked
  who are still participants (`ChatView`), falling back to "whoever was asked". The agent waiting
  for a person takes `ParticipantLeft` too and takes whoever left off the questions of the step
  (`ToolCalls::left`, `Questions::remove_asked`): while others asked are left, the questions stay
  open for them; once nobody asked is left, the call goes unanswered - answered as not executed,
  with a text of its own and no record of answers -, also one still waiting for its approval, whose
  request is withdrawn. The leave is noted at once, but put aside with the turn and written after
  the step's results like what is given (`TurnState::after_step`, generalised from
  `given_after_step`), since nothing may stand between calls and their results (issue 389). That
  it is noted ahead of what was queued before it, and whatever the turn hears, is a named exception
  (AGENT-TURN.md); avoiding it would need a way to wait for an event without taking it.
