# 482 — A question waits for an actor key instead of a name

**Status:** open
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
