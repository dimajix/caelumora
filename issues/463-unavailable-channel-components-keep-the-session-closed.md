# 463 — A channel whose components cannot be read keeps its whole session from opening

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Sessions and channels
**Summary:** A channel whose own components name a bundle that is gone or no longer its owner's to read fails to be built, and with it the session it belongs to, all its other channels included - though a third party must not keep someone's channel from starting.

## The problem

A participant that cannot be run is left out when its channel starts, and the channel runs with
everyone else: an agent whose personality, or a component bundle it includes, was deleted or
withdrawn (issue [270](270-personality-inheritance.md), `cannot_run` in
`caelumora-runtime/src/channel/mod.rs`). `docs/CHANNELS.md` gives the reason: "A third party - the
owner of a personality or a component - must not keep someone's channel from starting."

A channel's own components are not treated so. They are built in the name of the channel's owner
(issue [321](321-channel-components-in-the-owners-name.md)), and may name a bundle by a `ref` -
also one that someone else shares, or that an operator keeps (issue
[403](403-decision-model-policy.md)). That it can be read is checked when the channel is created
(`ComponentRegistry::require_acceptable`), not afterwards. If the bundle is
deleted later, or its share withdrawn:

- `ChannelLoop::new` fails with `SessionError::InvalidComponents`, whatever
  `ComponentError::is_unavailable` says;
- `SessionLoop::new` builds every channel of the session with `?`
  (`caelumora-runtime/src/session/mod.rs`, `add_channel_loop`), so the session does not open -
  none of its channels, also those whose components are all readable;
- `SessionManager` answers the request that opened it with that error.

So the owner of a bundle - a third party - can close every session that has a channel which
includes it, by deleting it or withdrawing the share.

## Why it matters

- It breaks the rule `docs/CHANNELS.md` states for exactly this case, one level up: what is
  withheld for one participant is left out, what is withheld for one channel takes the session.
- The other channels of the session are not at fault, and may belong to the same owner only by
  chance of how the session was made.
- A session can then neither be opened nor - by its participants - be looked at running, until
  its channel is changed; whether that channel can be changed while the session is closed is
  itself to be checked.

## Considerations

- **The channel left out, as a participant is.** A channel whose components are unavailable
  (`is_unavailable`) is not started, with a warning, and the session opens with its other
  channels. The channel is still there, and shown as not running; it starts once its components
  can be read again - when its session opens next, or when its components are changed. A failing
  store stays an error of the whole session, as it is for a participant.
- **What a channel that is not running answers.** Messages sent to it, a participant added, its
  state asked for: refused with a reason that names the components, rather than "not found" -
  which needs a state of a channel the runtime does not have yet.
- **Built without what is missing.** The channel starts without the components of the bundle that
  cannot be read. A moderator is required (one, `components.one::<dyn Moderator>()`); if it was in
  the bundle, the channel would run with the default moderator or not at all - a channel that
  behaves otherwise than its owner configured, without anyone having changed it. Rejected for
  that reason unless the missing components are optional.
- **The rule narrowed.** `docs/CHANNELS.md` says that a channel's own components are its owner's
  responsibility, and that a bundle it includes is a dependency the owner accepted. That keeps the
  code, but leaves the session closed for the other channels, and the bundle's owner with the power
  to close it.

## Done when

- A channel whose components cannot be read does not keep the other channels of its session, nor
  the session, from opening.
- What such a channel answers, and how it starts again, is decided and described in
  `docs/CHANNELS.md`.

## History

- **2026-10-23** — Opened from a finding of the review of the follow-up of
  [457](457-store-answers-misread-by-seed-and-ref.md), which made a failing store tell itself
  apart from a bundle that cannot be read.
