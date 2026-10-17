# 144 — What a runtime answers about its entity, and what the entity answers itself

**Status:** done
**Area:** Code structure
**Summary:** Runtimes forward attributes of their own and of the entity above in several ways, with no rule for what the runtime answers and what the entity answers itself.

## The problem

The running objects - `SessionLoop`, `ChannelLoop`, `ActorRuntime` - hold their stored entity
(`Session`, `Channel`) and hand out what callers need of it. How they do it, and how a caller
reaches an attribute of the entity *above* its own, differs from place to place without a rule:

- **Attributes of the own entity** are sometimes single getters mirroring a field
  (`ChannelLoop::name`, `moderator`, `security_attributes`, `classification`, `SessionLoop::name`),
  sometimes the whole document (`SessionLoop::session()`; the `ChannelLoop` has no such accessor).
- **Derived values** live on the entity in one case (`Channel::channel_ref`, forwarded by
  `ChannelLoop::channel_ref`) and in the model beside it in another (`SessionPrivacy::of(&Session)`,
  forwarded by `SessionLoop::privacy`).
- **Attributes of the entity above** are forwarded under a prefixed name in one direction -
  `ActorRuntime::channel_classification` and `channel_attributes` for the actor's channel,
  `ChannelLoop::session_privacy` for the channel's session (issue 134) - and navigated to in
  another: the `ChannelSessionReader` goes `own.session()` and asks the session loop itself.
- A forwarder decides what holds when the entity above is missing: `channel_classification` answers
  `None`, `session_privacy` answers "private". Where that decision belongs is not written down.
- The same name means different things: `SessionLoop::session()` is the running session's document,
  `ActorRuntime::session(security, id)` (issue 134, step 2) loads any stored session.

## Why it matters

- Every new attribute - a classification (133), a privacy policy (134) - raises the question again,
  and the answer so far depended on whoever wrote the code that day.
- Forwarders add up: each level that forwards an attribute of the level above grows by one method
  per attribute, and the fallbacks for a missing parent are spread among them.
- Where a value derived from an entity is computed decides whether the stored document and the
  running object can disagree about it.

## Considerations

- **The entity answers, the runtime navigates.** Derived values are methods of the entity
  (`Session::privacy()` next to `Channel::channel_ref()`); a loop hands out its document as a
  snapshot (`ChannelLoop::channel()` like `SessionLoop::session()`) instead of mirroring fields; a
  caller reaches the level above through its parent (`channel.session()`) and decides itself what a
  missing parent means. Few methods, one rule; the price is longer call chains and the
  missing-parent case at every caller.
- **The runtime forwards, with one naming rule.** A loop has a getter for whatever callers need,
  attributes of the level above prefixed with that level (`channel_*`, `session_*`), and the
  fallback for a missing parent is the forwarder's. Callers stay short; the loops grow with every
  attribute.
- **Mixed, by live state.** What only the running object knows - members, actors, the live
  classification during a reclassification - is the runtime's; what follows from the stored document
  alone is the entity's. Needs a clear line where the two overlap (a classification changed live and
  stored).
- Whatever is decided, the existing accessors are to be aligned to it, and the rule written down in
  CODING-GUIDELINES.md or ARCHITECTURE.md.

## Done when

- It is decided and written down what a runtime answers about its entity, what the entity answers
  itself, and how the level above is reached.
- The accessors of `SessionLoop`, `ChannelLoop` and `ActorRuntime` follow it.

## History

- **2026-09-30** — Opened at the maintainer's request while implementing step 2 of
  [134](134-session-privacy.md): `ChannelLoop::session_privacy` forwards an attribute of the
  session, `ActorRuntime::channel_classification` one of the channel, and the maintainer wants to
  discuss what belongs to the runtime and what to the entity.
- **2026-09-30** — Correction: the `ChannelLoop` does hand out its document
  (`ChannelLoop::channel()`); both loops offer the snapshot and single getters alike.
- **2026-09-30** — Decided and done, as three rules in
  [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#runtimes-and-their-entities): the model
  derives; a runtime answers about its own entity - getters for what is needed often, since they
  return owned values and hold no `arc_swap::Guard` across an `await`, the snapshot for the rest -
  and the entity above is reached through the parent, whose absence the caller decides about; one
  name means one kind of thing. Accordingly `ActorRuntime::channel_classification`,
  `channel_attributes` and `ChannelLoop::session_privacy` are gone (the `SessionReader` decides that
  a channel of no session is private); `ActorRuntime::channel` is `channel_loop`,
  `ChannelLoop::session` is `session_loop`, and `ActorRuntime::session(security, id)` is
  `load_session`. Keeping only snapshots was weighed and not taken: the getters are used in many
  places, each of which would then have to handle a guard.
- **2026-10-15** — The rule that the entity above is reached through the parent has one exception
  since [370](370-agent-logic-in-a-crate-of-its-own.md), by the maintainer's decision: the
  `ActorRuntime` is the actor's interface to its channel, and offers what an actor needs of it -
  `channel_classification` and `channel_attributes` again, `address_questions`,
  `workspace_terminal`, `workspace_shell` - each answering `None` or `NotUsableInChannel` without
  a channel, so that the caller still decides what that means. The reason is one this issue did
  not have to weigh: across a crate boundary, the way to the parent is its whole public API, and
  that of `ChannelLoop` is the frontends', which acts in a user's name too. The rule in
  CODING-GUIDELINES.md says so.
