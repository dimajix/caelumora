# 44 — A moderator that asks only mentioned agents to respond

**Status:** done
**Area:** User interaction

## The problem

Every moderator asks some agent to respond to every message. `broadcast`
asks all of them, and `round_robin` and `random` ask one
(`caelumora-runtime/src/moderator_impl/`). An `@`-mention overrides that
choice ([29](29-mentions.md)), but a message without a mention still gets an
answer from whoever the moderator picks.

There is no way to have a channel where agents stay quiet unless they are
addressed: a conversation among users, or with several specialised agents,
where an agent speaks only when someone writes `@name`.

The feature is a new moderator, e.g. `mentions_only`. It asks exactly the
mentioned agents to respond and nobody else. A message without a mention gets
no answer.

## Why it matters

- In a channel with several agents, the existing moderators produce answers
  nobody asked for. `broadcast` in particular makes every agent answer every
  message. Being addressed explicitly is how people expect to reach an agent in
  a group chat.
- It makes channels with several users and a few agents practical. Users can
  talk with each other without waking an agent on every message.

## Considerations

### Where it lives

- Moderators are chosen by name in `ChannelTemplate.moderator` and built in
  `RuntimeManager::new_moderator` (`caelumora-runtime/src/runtime.rs`). The new
  one goes there next to the others.
- Most of the logic already exists in `ModeratorActors`:
  - `forward_routes`: every participant still *receives* every message, as
    with the other moderators, so histories stay consistent.
  - `mentioned_responders`: who is asked to respond.
  - The routing of approvals, `TurnFailed` and `CommandExecuted`.
- The moderator is `mentioned_responders(...).unwrap_or_default()`. The work is
  in the decisions below, not in the code.

### A message without a mention

A message without a mention is received by everyone, and nobody answers - no
hint, no event. That is what the moderator promises (decided 2026-09-25).

### Mentions written by agents

Only mentions written by a user are routed, as with the other moderators:
`mentioned_responders` returns `None` for any other sender. Under this
moderator agents therefore never answer each other. Routing agent mentions
needs a way to bound chains of agents addressing each other, which is
[45](45-agent-mentions.md), for all moderators.

### Sleeping agents

The channel already wakes mentioned sleeping agents before routing
([29](29-mentions.md)), so nothing changes here.

### Showing the moderator in the UIs

Neither UI shows the channel's moderator. With this moderator a user must know
that agents answer only when addressed, so it belongs to this issue:

- `ChannelResponse` carries the channel's moderator, which `Channel.moderator`
  already stores. It is an added field, so it stays compatible with `caelumora-web-ui`.
- The web UI shows it in the channel header, and for `mentions_only` a
  placeholder in the input field says that agents answer only when mentioned.
- The TUI shows it at least when the channel is opened.

### An example session template

A session template that uses the moderator belongs to this issue, so it can be
tried without writing one. Modelled on `sassy-session`: the user and the two
agents Jonathan and Sassy in one channel `main`, with
`"moderator": "mentions_only"`, named e.g. `mentions-session`.

Session templates live in the entity store
(`local/entity_store/templates/sessions/`), which is git-ignored. The example
therefore also needs a versioned home, e.g. next to `examples/config.yaml`,
and a note in the README on where to copy it; alternatively, the store gets
it when it is empty. Which of the two is decided when implementing.

## Done when

- A channel with `moderator: mentions_only` (name to be decided) asks exactly
  the agents mentioned by a user to respond, and nobody for a message without
  a mention.
- Every participant still receives every message. Approvals, failed turns and
  command output are routed as by the other moderators.
- The REST API returns the channel's moderator; the web UI shows it and, for
  this moderator, says in the input field that agents answer only when
  mentioned. The TUI shows it when the channel is opened.
- There is an example session template with this moderator, versioned in the
  repository, and it is documented how to use it.
- The moderator is covered by the shared moderator tests in
  `moderator_impl/mod.rs` and documented in `ARCHITECTURE.md` and
  `examples/config.yaml`.

## History

- **2026-09-25** — Issue created. It builds on the mention resolution from
  [29](29-mentions.md); the recommendation is to route only user mentions at
  first and to handle agent-to-agent delegation separately for all moderators.
- **2026-09-25** — Decided: a message without a mention gets no answer and no
  hint (works as designed); only user mentions are routed, agent mentions move
  to [45](45-agent-mentions.md); showing the moderator in the UIs is part of
  this issue.
- **2026-09-25** — Added: an example session template with this moderator
  is part of the issue.
- **2026-09-25** — Implemented.
  - The three moderators were copies of each other: the same `route()`, the
    same forwarding, the same precedence of mentions, differing only in whom
    they ask when nobody is mentioned. They are now one `PolicyModerator`
    with a `ResponderPolicy` each (`BroadcastPolicy`, `RandomPolicy`,
    `RoundRobinPolicy`, `MentionsOnlyPolicy` in
    `caelumora-runtime/src/moderator_impl/`), so the precedence of mentions
    lives in one place, which [45](45-agent-mentions.md) will change. One
    unexplained difference went away with it: `broadcast` asked mentioned
    agents only if the message was forwarded at all, the others always; now
    all do the former, which makes no difference for a message with text.
    `round_robin` no longer adjusts its index when an agent leaves; it takes
    it modulo the candidates on every turn anyway.
  - The moderator is named `mentions_only`.
  - `ChannelResponse.moderator` (from `Channel.moderator` via
    `ChannelLoop::moderator()`, and from the stored channel for sessions that
    are not running). The web UI maps the four known kinds to a label with an
    explanation as tooltip (`src/moderator.ts`), shown above the member
    list, and changes the input placeholder for `mentions_only`. The TUI
    prints the moderator when the chat starts.
  - The example template `mentions-session` (the user, Jonathan and Sassy in
    `main`) is seeded by the REST server in `caelumora-rest-server/src/data.rs`
    like `simple-session` and `sassy-session`. The question of a versioned
    home for it did not arise: example templates were never read from the
    git-ignored store but created at startup. `examples/config.yaml` has no
    templates, so it is not changed; the moderators are documented in
    `ARCHITECTURE.md`.
  - Verified in the browser: a message without a mention got no answer,
    `@sassy` got exactly Sassy's; a `round_robin` session still answers and
    shows "Round robin" with the normal placeholder.
