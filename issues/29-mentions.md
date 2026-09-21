# 29 — Addressing participants with @-mentions

**Status:** done
**Area:** User interaction

## The problem

In a channel with several agents the user has no way to say who should answer.
Every message is forwarded to all other participants (`forward_routes` in
`caelumora-runtime/src/moderator_impl/mod.rs`), and who is then asked to respond
(`ActorEvent::ResponseRequested`) is decided by the moderator alone — the next
one in round-robin order, a random one, or all of them for `broadcast`. "Sassy,
what do you think?" is a message like any other; whether Sassy answers is luck.

The feature: an `@`-mention in a message — `@sassy` — addresses the participant
named "Sassy", and the moderator honours it. Several mentions address several
participants, and every one of them is asked to respond. The UIs support the
notation: mentions are rendered as such, and typing `@` offers a selection of
the channel's participants.

## Why it matters

- It is the missing control over multi-agent channels. Without it, the only
  way to reach a particular agent is a channel with only that agent in it, and
  the moderators beyond `broadcast` are hard to use in practice.
- It is a notation users already know from every chat system, so it needs no
  explanation — as long as it behaves the way they expect, which is mostly a
  question of the edge cases below.

## Considerations

### What a mention changes: who responds, not who receives

Today every participant already *receives* every message; the moderator only
chooses who is *asked to respond*. A mention therefore should not narrow
delivery — the other agents still see the message and keep a consistent
history — but override the moderator's choice of responders: every mentioned
participant gets a `ResponseRequested`, nobody else does. Narrowing delivery
as well (a private message) is a different feature with different
consequences for [17](17-information-sharing-within-session.md) and for what an
agent can later be asked about, and is out of scope.

This has to hold for every moderator, not just one. The natural place is the
shared `ModeratorActors` helper: resolve mentions once, and let each moderator
fall back to its own policy only when a message contains none. Whether a
moderator may ever *ignore* a mention (e.g. a strictly turn-based one) is a
decision to make explicitly rather than by accident of implementation.

### Resolving names

- Agent names live in `Agent.metadata`, users have a username; neither is
  guaranteed to be unique within a channel, and names may contain spaces or
  characters that do not survive in `@…`. The mention needs a *handle*: a
  normalised, case-insensitive form of the name, unique per channel, with a
  defined rule for collisions (reject at join time, or disambiguate).
- A mention must be resolved to an `ActorId` **when the message is sent** and
  stored with the message, not re-parsed from the text later. Otherwise a
  rename, or a participant leaving and another with the same name joining,
  retroactively changes who a past message addressed — which also matters for
  [06](06-context-in-message-for-auditability.md).
- An `@name` that matches no participant is plain text. Rejecting the message
  would break email addresses and ordinary uses of `@`; the UI can still show
  it as unresolved.
- Mentioning a user (not an agent) has no routing effect — users are not asked
  to respond — but is still worth resolving, so the UI can highlight or notify.

### Sleeping agents

`actor_candidates` skips agents in `ActorState::Sleeping`. An explicitly
mentioned agent should arguably be woken rather than skipped: the user asked
for it by name. That overlaps with `/wakeup` in
[28](28-user-commands.md) and should use the same mechanism.

### Mentions written by agents

If agents may mention each other, the moderator will route an agent's
`@other` just like a user's — which is powerful (delegation between agents) and
a loop waiting to happen (two agents addressing each other indefinitely). The
options are: honour only user mentions at first; honour agent mentions with a
bound on consecutive agent-to-agent hops; or honour them freely. The first is
the safe starting point. Either way the agents need to know the notation
exists and who they can address, which fits a `SystemPromptProvider`
contribution listing the channel's handles, and they need to see in their
context that a message was addressed to them.

### UI support

- **Rendering:** resolved mentions are highlighted in both the web UI and the
  TUI; unresolved `@…` stays plain text.
- **Completion:** typing `@` opens a list of the channel's participants,
  filtered as the user types, inserting the handle on selection. This needs the
  participant list with handles from the REST API; any change there must stay
  compatible with `caelumora-web-ui`.
- The same completion popup is the natural home for `/` commands from
  [28](28-user-commands.md), so the two should share the component.

## Done when

- A message with one or more `@handle` mentions results in a
  `ResponseRequested` for every mentioned agent and no other, regardless of the
  channel's moderator; without mentions, moderators behave as before.
- Every participant still receives the message.
- Handles are unique per channel, resolved to `ActorId`s at send time and
  stored with the message.
- Unmatched `@…` is left as plain text.
- Mentioned sleeping agents are woken, consistently with `/wakeup`.
- It is decided, and documented, whether mentions written by agents are
  routed, and if so how loops are bounded.
- The web UI renders mentions and offers participant completion on `@`; the
  TUI at least renders them.

## History

- **2026-09-21** — Issue created. A mention overrides the moderator's choice of
  responders rather than restricting delivery; resolved to `ActorId`s at send
  time; UI rendering and `@` completion included.
- **2026-09-21** — Implemented. Decisions taken along the way:
  - Handles are derived from names (`caelumora_model::mention::handle_of`),
    not stored. Uniqueness is *not* enforced: a handle shared by several
    participants is logged as a warning when the channel starts, and
    mentioning it addresses all of them. Rejecting at join time would have
    made channels from existing templates fail to start.
  - Resolution happens in `ChannelLoop::handle_actor_event`, before recording,
    history and routing; results are stored in `Message::mentions`. Text in
    backticks and `@` preceded by a handle character or `.` (emails) do not
    count.
  - Only user-written mentions are routed (`ModeratorActors::mentioned_responders`),
    for all three moderators; round-robin does not advance its rotation for a
    mention. Agent mentions are resolved for display only. Agents therefore
    get no system prompt about handles yet.
  - Waking: `ChannelLoop` wakes mentioned sleeping agents before routing.
    This exposed a race in `AgentActor::handle_sleep`, which returned a hard
    `Sleeping` and so overwrote a `wakeup()` that happened while it was
    blocked; it now keeps `Idle` if woken (`state_after_sleep`). The same race
    affected the existing wakeup on every REST/TUI message.
  - REST: `ChannelResponse` gained `members` (id, name, handle, kind);
    `participants` is unchanged for compatibility. Web UI highlights resolved
    mentions via a remark plugin and offers completion on `@` (arrow keys,
    Enter/Tab, Escape). TUI highlights resolved mentions; no completion there.
  - Verified end to end against the REST server with the `sassy-session`
    template: `@sassy` / `@jonathan` got exactly that agent's answer, two
    mentions got both. The web UI was type-checked, linted and built, but not
    exercised in a browser.
  - Not done: the shared completion popup with `/` commands from
    [28](28-user-commands.md) — 28 does not exist yet; the popup in
    `ChatView` is the place to extend.
- **2026-09-21** — REST response simplified: `ChannelResponse.participants`
  removed, `members` now lists *all* participants (approvers included) with
  `handle: Option<String>` — `null` where a participant cannot be mentioned.
  `ChannelLoop::members()` no longer sorts; the order is arbitrary (it comes
  from a `HashMap`) and sorting by name is left to the client, which the web
  UI does.
