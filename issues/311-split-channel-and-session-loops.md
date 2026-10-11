# 311 — `channel.rs` and `session.rs` hold everything about channels and sessions in two files

**Status:** done
**Area:** Code structure

## The problem

`caelumora-runtime/src/channel.rs` has about 5900 lines, `session.rs` about 2750. Each is one file
holding several subjects that have little to do with each other except the type they hang on.

`channel.rs` has about 1640 lines of production code:

- `ChannelLoop`, its construction and accessors, `ChannelMember`, `ChannelError`;
- `Member` - who may take part, their clearance, the scope they act in - and making their actor;
- changing the channel: `update_name`, `update_language`, `update_description`,
  `update_uses_workspace`, `update_approval_rules`, `reclassify`;
- its participants: `add_participant`, `remove_participant`, `left`, the mentions, finding members
  by name;
- controlling the actors: `cancel`, `wakeup`, `cancel_actor`, `interrupt_tool_call`,
  `wakeup_actor`, `stop_actors`;
- the run loop: actor changes, spawning actors, admitting, resolving mentions, routing, appending
  to the history, recording;
- `VisibleHistory` and the rules of `HistoryAccess`.

Its tests have about 4260 lines, and most of them are not about the channel: about 1450 lines test
an agent's turn (plans, approvals, retries, cancelling, removal during a turn), about 1740 an
external agent, the rest commands, nudges and questions - each through a running channel, with
fixtures (`Stuck`, `agent_channel`, `external_channel`, `Gated`, `Sleeper`) only this module has.

`session.rs` has about 1140 lines of production code: `SessionLoop` with its run loop and the
creating, deleting and restoring of channels in a running session (about 520), and
`SessionManager` (about 525), which starts, stops, deletes, restores and purges sessions - and
agents. Its tests have about 1590 lines, a good part of them about channels: classification,
reclassifying, clearance, participants added and removed.

## Why it matters

A change to one subject - how a channel is renamed, how an event is routed - is made in a file that
holds all of them, and its review has to find it among them. The tests are where the fixtures are,
not where their subject is: whoever changes how an external agent answers looks for its tests under
`actor/external_agent.rs` and finds them in `channel.rs`.

## Considerations

**One module or two.** A channel belongs to a session, so `session/channel/` would mirror
ownership. But `ChannelLoop` is used by itself - by the actors, the views, the REST server - and the
views already have `view/channel.rs` and `view/session.rs` side by side. Two modules `channel/` and
`session/` keep `crate::channel::…` and `crate::session::…` as they are, so nothing outside
changes. Recommended: two modules.

**The files of `channel/`** - one subject each, as listed above:

- `mod.rs`: `ChannelLoop`, its construction and accessors, `ChannelMember`, `ChannelError`;
- `member.rs`: `Member` and `require_admitted`;
- `settings.rs`: the `update_…` methods and `reclassify`;
- `participants.rs`: adding, removing and leaving, mentions, members by name;
- `control.rs`: cancelling, waking, interrupting and stopping actors;
- `run.rs`: the run loop and routing;
- `history.rs`: `VisibleHistory` and what part of the history a participant sees.

**The files of `session/`:** `mod.rs` with `SessionLoop` and its run loop, `channels.rs` with the
channels of a running session, `manager.rs` with `SessionManager`, and `testing.rs` as now.

**Where the tests go.** The guidelines put a unit test at the bottom of the file it tests. The
channel's own tests go with the part they test. For the tests of the actors there are two ways:

- `channel/tests/{agent_turn, external_agent, commands, questions}.rs`, with the fixtures in the
  module above them. Cheap, but the tests stay under the channel.
- Next to the actors they test (`actor/agent`, `actor/external_agent`, `command`, ...), with the
  fixtures in a `#[cfg(test)] pub(crate) mod testing` of `channel`, as `session::testing` already
  is. The fixtures become `pub(crate)`.

Recommended: next to the actors, since a test belongs with its subject. The same holds for the
tests in `session.rs` that are about a channel.

**What it costs.** A child module sees the private fields of `ChannelLoop`, so fields stay as they
are. A method called from another file needs `pub(super)` - the reason
[159](159-split-agent-actor-file.md) did not split the agent actor. Splitting by subject keeps such
calls few (mostly `run.rs` calling into `Member` and the participants), but where a part would need
`pub(super)` on most of its methods, it stays in `mod.rs`.

**A step of its own.** The move changes no behaviour and is reviewed as such: `channel` and
`session` each in a commit of their own, nothing renamed or rewritten on the way. What is noticed
while moving goes into issues. One such question already: `SessionManager` also deletes, restores
and purges agents (`delete_agent`, `restore_agent`, `purge_agent`). Deleting an agent has to remove
it from the running sessions, which may be the reason; whether these belong there is to be looked
at, not decided here.

## Done when

- `channel` and `session` are directories, and each of their files can be described in one sentence.
- The tests of an actor's behaviour are with that actor, or this issue records why not.
- No behaviour changed; the tests and `cargo clippy --workspace --all-targets` pass.

## History

- **2026-10-11** — Opened.
- **2026-10-11** — Done, as moves only: every line of the two files is in one of the new ones;
  added are re-exports, so that `crate::channel::…` and `crate::session::…` stay as they were, and
  `pub(super)` where one part calls another.
  - `channel/` holds `mod.rs`, `member.rs`, `settings.rs`, `participants.rs`, `control.rs`,
    `run.rs` and `history.rs`, as proposed. Calls between the parts are few: `pub(super)` went on
    `Member` and four of its methods, on `rebuild_mentions`, `cancel_actors`, `wake_for_delivery`
    and `history_start`. `RunningActor` is `run`'s alone and moved there.
  - The tests of the actors went to `channel/tests/`, the cheaper way, as the maintainer decided:
    `agent.rs` (named after the actor, since it also holds what an agent takes in when it joins),
    `external_agent.rs`, `commands.rs` and `questions.rs`, with the fixtures and the tests of
    `mod.rs` in `channel/tests/mod.rs`. A child of `channel`, the fixtures see what the tests used
    of `ChannelLoop`'s private fields; the few the tests in the part files use are `pub(super)`. The
    tests of cancelling one actor and of a cancelled channel call `route_event` and moved to
    `run.rs`.
  - `session/` holds `mod.rs`, `settings.rs` (added: the session's settings, as for the channel),
    `channels.rs`, `manager.rs` and `testing.rs`. The free functions several parts use
    (`require_classification`, `preferred_language`, `purge_channel_document`,
    `require_attachable`) stay in `mod.rs`.
  - The tests in `session.rs` about a channel went to `channel/settings.rs`, `member.rs`,
    `participants.rs` and `tests/external_agent.rs`; the fixtures they share with the session's
    tests are in `session::testing`, whose `record_users` reads `SessionManager::runtime_manager`,
    now `pub(super)`. Four tests of reading and snapshots stay in `session/mod.rs`, though they are
    about the views: `view/mod.rs` has fixtures of the same names (`make_session`, `record_users`)
    and other signatures, and two of them build a `ChannelRuntime` from `SessionLoop`'s private
    fields - moving them would mean rewriting them.
  - Opened from what was noticed: [314](314-session-manager-handles-agents.md), whether the
    `SessionManager` should delete, restore and purge agents, and
    [315](315-view-and-session-test-fixtures.md), the fixtures that keep the four tests of the views
    with the sessions.
