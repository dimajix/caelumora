# 74 — Refactoring epic: the structure review of 2026-09-28

**Status:** done
**Area:** Maintenance

## The problem

A structure review of `caelumora-model`, `caelumora-runtime` and
`caelumora-rest-server` (`work/2026-09-28-code-review.md`) found eight
behavioural errors and a number of structural weaknesses: three state
machines for one fact, turn logic sitting on the actor instead of the turn,
orchestration in the model, duplicated shapes (chains, stores, run loops,
lookups, error mappers), and a wider public surface than the code uses. Each
finding is its own issue, 75–118. This issue is the bracket: it lists them
in the recommended order and records what the open issues forbid.

## Why it matters

The findings interlock. Telling a crash from a deliberate stop
([75](75-agent-resumes-half-turn.md)) falls out of a `TurnState`
([85](85-turn-state-in-agent-channel-state.md)); moving turn logic onto `Turn`
([83](83-turn-logic-on-agent-actor.md)) is easier once the state machine
has fewer states ([84](84-agent-state-machine.md)). Doing them in the wrong
order means doing some twice.

## Constraints from the open issues

The review's section 2 lists what a refactoring must not touch because an
open issue relies on it. The short form:

- `Recorder` stays (05, 07). `SecurityContext::delegation_ids`/`caller_id`
  stay (12). `User.roles`, `Scope::GroupScope` only go with an entry in 23.
- `AgentChannelChange`/`AgentStateChange` and `Turn::stage_*` are the seam
  for 65 and 55; `Chain<E>` stays a type for 08; `Reflecting` and the order
  store → announce → answer stay for 55.
- `Publication`, `ChannelClassification`, `KnowledgeSource` get finer, not
  coarser (64, 58, 59). The turn's classification snapshot is for 67.
- REST DTOs never serialise model types directly (42, 24).
- `CommandExecutor` (struct) vs. `ToolExecutor` (trait) is deliberate (28).

## Recommended order

1. The errors, independent and small:
   [75](75-agent-resumes-half-turn.md) (the deliberate-stop half; the rest with 85),
   [76](76-channel-cancel-flag.md),
   [77](77-template-swallows-store-errors.md),
   [78](78-mcp-requires-approval-ignored.md),
   [79](79-user-actor-exits-on-send-error.md),
   [80](80-stored-channel-response.md),
   [81](81-approver-double-id.md),
   [82](82-tool-parameter-parsing.md).
2. [76](76-channel-cancel-flag.md) — one state machine less.
3. [85](85-turn-state-in-agent-channel-state.md) — the turn's working state as
   `Option<TurnState>`, a plain `ActorState` in the document. Makes "resume
   after a crash" one value (75) and prepares 65 and 08.
4. [83](83-turn-logic-on-agent-actor.md), [84](84-agent-state-machine.md).
5. [90](90-message-path-wake-and-queue.md) — waking in the channel, no
   `UserActor` queue, `perceive` without a turn start.
6. [100](100-tool-executor-table-and-result.md).
7. [86](86-controllers-in-the-model.md), [87](87-security-context-in-entities.md),
   [88](88-template-instantiation-in-the-model.md), [89](89-channel-runtime-delegators.md),
   [91](91-double-actor-table.md).
8. [93](93-entity-store-dialects.md), [94](94-users-unchecked-in-store.md) — before 03/04.
9. [92](92-ledger-for-notes-and-memory.md) — the first step of 65.
10. REST: [102](102-rest-channel-lookup.md), [103](103-rest-error-mappers.md),
    [104](104-sse-stream-and-subscribe.md), [106](106-rest-small-findings.md).
11. [98](98-context-provider-facet.md) — an architecture decision first; then
    [99](99-component-factory-and-tags.md).
12. [95](95-require-for-security-manager.md), [96](96-config-unification.md),
    [97](97-small-model-unifications.md), [101](101-chain-component-helpers.md),
    [111](111-tool-parameter-reading.md), [112](112-id-types.md),
    [113](113-error-shapes.md), [114](114-moderator-details.md),
    [115](115-filtering-event-sender.md), [117](117-message-prev-id.md),
    [118](118-malformed-call-put-up-for-approval.md);
    [116](116-session-loop-channel-tables.md) together with 91.
13. [107](107-public-interface.md), [108](108-serde-defaults-for-legacy.md),
    [109](109-code-hygiene.md), [110](110-docs-contradicting-code.md) in one pass each.
14. [105](105-seed-data-twice.md) — planned separately, it pulls the TUI along.

## Done when

Every issue 75–118 is `done` or `rejected`, and this one records what
changed in the order along the way.

## History

- **2026-09-28** — Created from the structure review
  (`work/2026-09-28-code-review.md`).
- **2026-09-30** — Status: all of 75-118 are done or rejected (89) except
  [92](92-ledger-for-notes-and-memory.md), which still needs its
  architecture decision. [117](117-message-prev-id.md) was done last,
  after a discussion of what `prev_id` was meant for.
- **2026-09-30** — Done: every issue 75-118 is done or rejected.
  [92](92-ledger-for-notes-and-memory.md) came last, taken into the epic
  of the agent review ([151](151-agent-refactoring-epic.md)): notes and
  memory are one generic `Ledger<K>` with one change enum, not an enum
  per kind behind a trait. Its follow-up [156](156-notes-and-memory-staged-scans.md)
  and the navigation it led to ([163](163-navigable-views.md)) are
  their own issues.
