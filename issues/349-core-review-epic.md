# 349 — Refactoring epic: the review of agents, sessions and channels of 2026-10-14

**Status:** open
**Area:** Maintenance

## The problem

A review of the core of `caelumora-runtime` - `channel/`, `session/`, `actor/` with both kinds of
agent, `controller/`, `runtime.rs`, `agent/state.rs`, `agent/manager.rs`, `group.rs`, `scoped.rs`
and `template.rs` - looked for what can be simpler: complexity, patterns and names that differ
without reason, methods that do the same, and responsibilities that have no owner. The structure
that [74](74-refactoring-epic.md), [151](151-agent-refactoring-epic.md), [211](211-turn-and-agent-work-structure.md)
and [229](229-services-fetched-through-the-runtime.md) established holds: one writer per document,
admission in one place, permissions before anything is stored. What is left is repetition - the same
five services and fields held or threaded again and again, three types carrying the same half of an
agent, construction sequences written twice - and a few rules of the model that live in the runtime.
Each group of findings is its own issue, 350-361. This issue is the bracket.

## Why it matters

The findings interlock. The duplicated fields of the two agent actors (355) are the parameters of
`intake.rs` and the fields of `AgentCommands`; the questions on `Member` (356) are what both loops
hold four services for; the construction sequences (357) and the facades (359) touch the same
constructors. Doing them in the wrong order means moving code twice.

## The goals and where they conflict

- **Fewer types where one bundles only parameters** - but a type that owns a responsibility is
  worth its definition: 355 and 356 each add one and remove more fields and parameters than they
  cost. Both are architecture decisions and are to be discussed before they are built.
- **Earlier decisions stand:** the delegators of `ChannelRuntime` stay ([89](89-channel-runtime-delegators.md),
  rejected); a channel holds its session through `attach` ([123](123-session-handle-in-channel-runtime.md));
  `Turn` is split by files, not types, and `Answering` is the one way to publish
  ([211](211-turn-and-agent-work-structure.md)); services are held, not fetched
  ([229](229-services-fetched-through-the-runtime.md)). 357 and 359 are cut so that they do not
  reopen them; the first two are raised again all the same, in 360 and 361, since the code they
  were decided on has changed - to be discussed, not assumed.
- **The model holds data and rules** (ARCHITECTURE.md): 353 and 358 move rules there, at the price
  of the model knowing the update events' application.

## Recommended order

1. [350](350-lookups-written-out-again.md), [351](351-channel-events-announced-by-hand.md),
   [352](352-envelopes-built-by-hand.md) - small, independent, no behaviour change.
2. [359](359-actor-runtime-hands-out-the-runtime-manager.md) - one accessor, before 355 builds on
   `ActorRuntime`.
3. [356](356-member-questions-threaded-with-their-services.md), its first step (`Member` knows its
   place).
4. [361](361-session-before-its-channels.md) and [360](360-channel-runtime-forwarders-again.md) -
   architecture decisions first, together, since both decide what a channel's loop is built with;
   then [357](357-actor-start-and-channel-construction-written-twice.md) in the shape they give it.
5. [356](356-member-questions-threaded-with-their-services.md), its second step (`Membership`) - an
   architecture decision first.
6. [355](355-two-agent-actors-share-half-their-fields.md) - an architecture decision first; the
   channel tests of both kinds of agent are the net. Then
   [363](363-run-control-built-twice.md), what 355 left with each actor.
7. [353](353-rules-of-the-model-computed-in-the-runtime.md).
8. [358](358-controllers-apply-their-events-each-on-its-own.md) - after 353, whose `apply` is one of
   its parts.
9. [364](364-managers-in-different-places.md), then [366](366-runtime-rs-holds-four-types.md) -
   files moved, nothing renamed; before the renames, which then find the files where they belong.
10. [365](365-security-manager-is-an-access-policy.md) - a rename across every crate, in a commit
    of its own.
11. [354](354-names-follow-several-schemes.md) - last, in a commit of its own: the renames touch the
    REST server and the tests, and every refactoring before changes which names there are.

## Considered and not taken up

- **Merging `cancel`/`cancel_actor` and `wakeup`/`wakeup_actor` into one method each with an
  `Option<ActorId>`:** they differ on purpose - cancelling one actor does not keep the channel
  quiet (CHANNELS.md) - and the pairs read better than a flag.
- **A generic compensation helper for "stored, but failed: taken back":** the seven places
  (`take_in`, `create_channel`, `restore_channel`, `establish`, `delete_session`, `keep_channel`,
  `take_back`) each undo something else; a helper would only hide which. The names are aligned in
  354 instead.

## Done when

Every issue of the order above is `done` or `rejected`, and this one records what changed in the
order along the way.

## History

- **2026-10-14** — Created from the review at the maintainer's request.
- **2026-10-14** — [360](360-channel-runtime-forwarders-again.md) and
  [361](361-session-before-its-channels.md) added at the maintainer's request: what 89 and 123
  decided is to be discussed again on the code as it is now, before 357.
- **2026-10-14** — [363](363-run-control-built-twice.md) added to step 6: where an agent's actor
  stands was built twice, which kept the command loop in both actors after 355.
- **2026-10-15** — [364](364-managers-in-different-places.md),
  [365](365-security-manager-is-an-access-policy.md) and [366](366-runtime-rs-holds-four-types.md)
  added at the maintainer's request, as steps 9 and 10, before 354; 364 taken up first.
