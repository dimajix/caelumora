# 474 — The web UI tells whether an actor is a user, and which, in several ways

**Status:** done
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

## Decision

Option A, wider than the problem names it: `caelumora-web-ui/src/actor.ts` is the one place that
knows the form of an `ActorId` - reading it, and making one. Besides the user test, the form is
read for an approver (`'Approver' in sender`) and for the system (`=== 'System'`), and made by hand
as `{User: …}` and `{Agent: …}`; a change of the form would be missed there the same way. `actor.ts`
gets `userActor` and `agentActor` to make one, `isApprover` and `isSystem` beside `userIdOf`, and
`actorKey` and `actorUuid` from `mention.ts`, which have nothing to do with mentions. Single
predicates rather than one `actorKind(id)`, which would leave `kind === 'Approver'` at the callers;
only two variants are asked about.

1.  **The type mirrors the server.** `ActorId` in `src/types` was `string | Record<string, string>`
    and becomes the union of serde's variants, `'None' | 'System' | {User} | {Agent} | {Approver}`.
    A step of its own, without behaviour changes.
2.  **`actor.ts` owns the form.** The functions above, and every component and module using them.

## Done when

- No component reads the form of an `ActorId` itself.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of step 4b of issue 467.
- **2026-10-28** — The maintainer decided for option A, with making an `ActorId` included and the
  type made exact as a step before. Step 1 done: `ActorId` is the union of the server's variants;
  the four tests for a user that read `.User` narrow it with `'User' in`.
- **2026-10-28** — Step 2 done: `actor.ts` holds `userActor`, `agentActor`, `userIdOf`,
  `isApprover`, `isSystem`, `actorKey` and `actorUuid`, and `agentIdOf` beside `userIdOf`, for
  `ChatView`, which read whether a sender is an agent itself. `NewChannelDialog` lost its own
  `actorKey`; no component and no other module reads or makes the form of an `ActorId`. At the
  maintainer's request from the review, `sameActor` names what was written out as
  `actorKey(a) === actorKey(b)`; `actorKey` is left for keys that are kept.
