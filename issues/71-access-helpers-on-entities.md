# 71 — Access-check helpers bloat the interfaces of the entities

**Status:** done
**Area:** Maintenance

## The problem

Over the last issues, the runtime's entities gained public methods whose
only job is to ask the `SecurityManager` one question or to apply a stored
limit on behalf of someone:

- `ChannelLoop::may_be_read_by(ctx)`: `GetChannel` with the channel's
  attributes (issue 70).
- `ChannelLoop::may_be_changed_by(ctx)`: `UpdateChannel`, so that a client
  offers changes only where they work (issue 46).
- `ChannelLoop::history_visible_to(participant)`: the history, cut at the
  participant's `HistoryLimit` (issue 46). It is a thin wrapper around
  `Channel::visible_history`.
- `ChannelLoop::may_read(source)`: `ReadChannel` between two channels, asked
  in the name of the reading channel's owner, with "a channel may always read
  itself" (issues 17, 61).
- `Actor::may_read_context(caller)`: `ReadAgentContext`; only `AgentActor`
  answers anything but `false` (issue 41).

Each of these is one or a few lines, but together they make the interface of
`ChannelLoop` read like a policy catalogue. Every new client-side question
("may this user do X here?") would add another one. The entity then carries
knowledge that is not about what it does, only about who may ask it.

## Why it matters

- `ChannelLoop` is already large. Its public surface should say what a
  channel does - routing, participants, cancelling, waking - not every way
  someone can be checked against it.
- The questions are asked from outside the entity: by REST (lists,
  snapshots, `can_change`, `can_read_context`) and by the `SessionReader`.
  Where they live decides how easily a policy question can be found, and
  whether the same question is phrased the same way everywhere.

## What is *not* meant

These operations check a permission as part of doing something:
`cancel` (`CancelChannel`), `wakeup` and `wakeup_actor` (`WakeChannel`),
`add_participant` (`JoinChannel`), `update_name` and the controllers
(`UpdateChannel` when storing). The check belongs to the operation and stays
with it. This issue is about the *pure questions*: methods that only answer
"may they?" or "what may they see?".

## Considerations

The five helpers are not all the same kind of thing:

- `may_be_read_by` and `may_be_changed_by` are nothing but
  `check(ctx, ActionType::X(&attributes))`. The only knowledge in them is
  which `ActionType` goes with which question.
- `may_read` holds a rule of its own (a channel reads itself; the owner is
  the one asking) and is used by the `SessionReader` and the private notes.
- `history_visible_to` asks no one. It applies data stored on the channel
  (`history_limits`) and is a projection, not a permission.
- `may_read_context` is polymorphic: whether there is a context at all
  depends on the kind of actor.

### Option A: free functions in the entity's file

For example `channel::may_be_read_by(security, ctx, &channel)` next to
`ChannelLoop`.

- **For:** the knowledge stays next to the entity it concerns, and the type's
  method list is clean.
- **Against:** hardly more than cosmetics. The functions are still part of
  the module's public API. In Rust, free functions are harder to discover
  than methods (no `.` completion). They need the `SecurityManager` passed
  in, which the entity used to have itself.

### Option B: one helper type per entity

For example `ChannelAccess<'a> { security, channel }` with `readable_by`,
`changeable_by`, `may_read(source)`, obtained through `channel.access()` or
built by the caller.

- **For:** everything about access to a channel is in one place, and it
  grows there rather than on the entity. It can be documented as a unit, and
  REST can build its `can_*` flags from it.
- **Against:** a second type per entity (channel, actor, and later session,
  agent...). `channel.access().readable_by(ctx)` is a detour for what is one
  `check` call. If it is obtained through `channel.access()`, the entity's
  interface still has an entry point, only a single one.

### Option C: one global helper in the security module

For example extension methods on `dyn SecurityManager`, or a struct
`Access` in `caelumora_model::security`:
`security.may_read_channel(ctx, &attributes)`,
`security.may_change_channel(ctx, &attributes)`.

- **For:** the policy vocabulary lives where `ActionType` lives. Every
  question can be found in one place, and the entities know nothing of it. It
  fits the existing pattern that the `EntityStore` asks the `SecurityManager`
  for everything it stores and loads.
- **Against:** the model crate does not know `ChannelLoop`, so the helpers
  take attributes or classifications, not runtime objects. The caller has to
  fetch them (`channel.security_attributes()`, `channel.classification()`).
  A catalogue of one-line wrappers around `check(ActionType::…)` could also
  just be the `ActionType` itself, asked directly.

### Option D (for comparison): no helper at all

Callers ask `security_manager.check(ctx, ActionType::GetChannel(&attrs))`
themselves.

- **For:** no new API. `ActionType` already is the catalogue.
- **Against:** the attributes to pass (and for `ReadChannel`, whose context
  to use) are repeated at every call site. A rule like "a channel reads
  itself" would be copied or lost.

### Tentative recommendation

Treat the kinds separately rather than force one answer on all five:

- The pure one-line questions (`may_be_read_by`, `may_be_changed_by`) go to
  **C**. Pick extension methods on the `SecurityManager` over a separate
  struct, so that there is no second object to pass around.
- `may_read` goes to C as well, taking both classifications and the reader's
  owner, because its rule is policy and not channel behaviour.
- `history_visible_to` goes away. Callers use `Channel::visible_history` on
  `channel.channel()` and `history()`. It is not an access check.
- `Actor::may_read_context` stays on the trait. Whether there is a context
  is the actor's business, and the policy question inside it is a one-liner
  that can call the C helper.

To be decided with the maintainer before anything is changed.

## Done when

- It is decided where pure access questions live, and the existing helpers
  are moved there.
- `ChannelLoop`'s public interface no longer holds methods that only answer
  "may they?".
- AGENTS.md or ARCHITECTURE.md says where such a question goes next time.

## History

- **2026-09-28** — Issue created at the maintainer's request, after
  `may_be_read_by` (issue 70), `history_visible_to` and `may_be_changed_by`
  (issue 46) were added to `ChannelLoop`. The options the maintainer named
  (free functions in the entity's file, a helper type per entity, a global
  helper in the security module) are laid out above, with a tentative
  recommendation. Nothing is implemented yet.
- **2026-09-28** — Decided with the maintainer and implemented: option C,
  as the extension trait `AccessQuestions` in `caelumora_model::security`
  with a blanket implementation for every `SecurityManager` (so no policy
  can answer a question differently from its `ActionType`):
  `may_read_channel`, `may_change_channel`, `may_channel_read` (with the
  rule that a channel reads itself, asked in the reader's owner's name) and
  `may_read_agent_context`. `ChannelLoop::may_be_read_by`,
  `may_be_changed_by` and `may_read` are gone; REST asks through
  `state.runtime_manager.security_manager()`, the `SessionReader` through
  its runtime. `Actor::may_read_context` stays on the trait - only an agent
  has a context - and uses the trait inside. The rule is recorded in
  AGENTS.md and ARCHITECTURE.md. `history_visible_to` stays on `ChannelLoop`
  for now: the maintainer wants to think about it again, which is why the
  issue stays in progress.
- **2026-09-28** — `history_visible_to` decided with the maintainer: it stays
  on `ChannelLoop`, which owns the history and its limits, as the one place
  the limits are applied - for REST, for `read_history` (through the new
  `history_with_digest_visible_to`) and for the transcript of a joining
  agent. Putting it into the `ChannelSessionReader` was considered and
  rejected: users over REST (the owner may read without an actor in the
  channel) and the seeding at join time are not readers of an actor. The
  model keeps `HistoryAccess`, `HistoryStart` and `HistoryLimit` as data
  only; `HistoryAccess::resolve`, `HistoryStart::visible` and
  `Channel::visible_history` moved into private functions of the runtime's
  channel module. That the digest recorded by `read_history` still names
  the whole history is [72](72-history-digest-ignores-limit.md). Status set
  to done.
