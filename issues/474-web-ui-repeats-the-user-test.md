# 474 — The web UI tells whether an actor is a user, and which, in several ways

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** Whether an `ActorId` names a user, and the user, is written out by hand in several components beside the one function for it.

## The problem

Issue [467](467-projects-hold-what-outlasts-a-session.md) moved `userIdOf` into
`caelumora-web-ui/src/actor.ts`, used by `GroupPage` and `AddUser`. The same test is still written
out in `NewChannelDialog.tsx:23` (`typeof id !== 'string' && id.User === user.id`),
`AgentPage.tsx:60` and `ChatView.tsx:457`.

## Why it matters

`ActorId` is a serialized enum whose form the client has to know; spread over components, a change
of its form - another variant, another shape - is met in some places and not in others.

## Considerations

- **A. Use `userIdOf(id) === userId`** in every place, and gather the other helpers on `ActorId`
  (`actorKey` in `mention.ts`) in `actor.ts` beside it.

## Done when

- No component reads the form of an `ActorId` itself.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 4b of issue 467.
