# 598 — A sub-agent nobody ends stays

**Status:** in progress
**Type:** feature
**Priority:** low
**Area:** Sessions and channels
**Summary:** A sub-agent of `caelumora-subagents` (issue 558) is ended only by the agent that started it (`end_subagent`); one it never ends stays - an agent, a participant of the session and an open thread - for as long as the session lasts, and across restarts.

## The problem

Issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md) left open (*An idle sub-agent*) whether a
sub-agent the main agent never ends is ended after a while. Today it is not: the agent stays in the
store, a participant of the session and its channel, its thread open, its delegate in the channel's
state; a model that forgets to call `end_subagent` leaves one behind with every task it hands off.

## Considerations

- **Ended after a while without a brief**, by the channel or the plugin - when, measured how, and
  whether a restart counts.
- **Ended with the main agent's turn, or its leave**, as a task of the work.
- **Ended with the session**, deleted as the session is.
- What an agent is told when its sub-agent was ended for it - so that `ask_subagent` answers
  `NoSubAgent` meaningfully.

## Done when

- It is decided when a sub-agent nobody ends is ended, and it is implemented and tested.

## What exists, as of 2026-11-24

- **A delegation** (`Delegation`, issue 557) is kept in the channel's state, written by the channel
  alone in `ChannelLoop::record`, which follows what the thread's delegate gives
  (`ChannelState::follow_given`, `Delegation::follow`): a brief given, what came of it given back,
  the next brief open. Once the last brief's answer is given back, `briefs` is empty, and nothing
  tells an answer not yet read from one read and left without a word.
- **The main agent is no participant of the thread.** It is the delegation's giver: the thread's
  circle is closed, founded with the sub-agent alone, and the main agent reaches it only by
  delegating again (`ask_subagent`).
- **The giver takes the answer in between turns.** `GivenToRead` is never taken during a turn
  (`HearsDuringTurn::takes`), only by `take_in` between turns, after a turn set aside in its thread
  ended; the agent records it as `MessageGiven`, carrying the answer's `MessageId`, which the
  channel records in its history. The giver's next turn in the thread the delegation was opened at
  is so the one that read the answer - its context is built from the transcript as it begins -,
  whatever request it answers (`answered` in the agent's intake joins requests).
- **What an agent records is told before its next step** (`tell_recorded`, at the top of the
  agent's loop): `MessageGiven` reaches the channel before the `TurnFinished` of the turn that read
  it.
- **Delegating does not wait** until `run` has recorded the brief
  (`ChannelLoop::delegate` sends `ActorChange::Delegate` and returns), and `run` takes events before
  changes (`biased`): a brief delegated in a turn may be recorded after that turn's end.
- **Ending a sub-agent** is the plugin's procedure `Ending` (`caelumora-subagents`): let go from the
  session, its thread closed, the agent deleted. The plugin hears no events: it acts only in a turn
  of the main agent, through its writer.
- **No channel starts a procedure.** Procedures are started by commands, by frontends, by the
  sub-agents' writer, and by the `AgentFollower` (`procedure::follower`), which follows the bus
  and carries out a `Follow` for every agent deleted or restored - every running session again
  after it lost events.

## Decision

Decided by the maintainer on 2026-11-24, after weighing the options below.

**D1 - The giver's word is due once it read the answer.** A delegation whose answer was given back
with no brief open waits for its giver's word on it. The word is due in the turn that read the
answer: the giver's first turn that ends, in the thread the delegation was opened at, after the
channel recorded the giver reading it (`MessageGiven`). A brief delegated before that turn ends is
the word *go on*; none is the word *let go*. A turn given up decides nothing - the word stays due
for the next turn that ends there -, and so does a thread quiet after a cancel, in which the giver
is not asked to respond. The giver leaving the channel lets the delegation go as well.

**D2 - The channel decides, not the agent.** What the giver read and when its turn ended, the
channel records already; it follows both in the delegation as it follows what the delegate gives.
The responsibility for the protocol stays with the channel: no model has to remember anything, and
no agent has to keep an obligation.

**D3 - A delegation let go ends a thread made for its founders.** A thread opened for founders made
for it alone (a sub-agent's) is ended with them as its delegation is let go: the founders let go
from the session, the thread closed, the founders deleted. Ending is a procedure of the kernel; the
channel announces the delegation let go, and the follower of the kernel carries the procedure out,
as it follows agents deleted. A delegation let go in another thread ends nothing; there is none
today.

Rejected:

- **Ended after a while without a brief** - a clock with a setting nobody chose, and the question
  whether a restart counts, for what the giver's word says exactly.
- **A new start ends the giver's idle sub-agents** - simple and within the plugin, but it bounds
  the leak rather than closing it: those of the last start stay; and a start that ends others is a
  side effect the model has to be told.
- **An obligation kept by the agent**, answered with a default as its turn ends - the
  responsibility for correct behaviour would move into the agent.
- **A hook of the event queue telling when an event is taken** - for one user, and `MessageGiven`
  tells it already.
- **Starting the procedure from the channel itself** (`tokio::spawn` from `record`) - no precedent,
  and it turns around who holds whom: procedures hold channels.

## Plan

The interfaces as proposed on 2026-11-24, to be agreed before the bodies are written. Each step a
commit of its own, reviewed before the next.

### Step 1 - Ending a thread with its founders, in the kernel

A restructuring without a change of behaviour: what the plugin's `Ending` does moves into the
kernel, and the thread knows that its founders were made for it.

```rust
// caelumora-kernel/src/channel/spec.rs
pub struct ThreadSpec {
    pub origin: ThreadOrigin,
    pub circle: Circle,
    pub founders: Vec<ActorId>,
    /// Whether its founders were made for it alone, and end with it; not unless asked.
    pub owns_founders: bool,
}

// caelumora-domain/src/channel/thread.rs
pub struct Opening {
    pub origin: ThreadOrigin,
    pub by: ActorId,
    pub circle: Circle,
    /// Whether its founders were made for it alone, and end with it (`ThreadSpec`).
    pub owns_founders: bool,
}

// caelumora-kernel/src/procedure/thread.rs
/// Ending a thread with its founders (issue 598): let go from the session, the thread closed, the
/// founders deleted if it owns them - in this order, since closing waits for nobody to take part.
pub struct End { .. }

impl End {
    /// Ending `thread` of the channel `channel_id` in the session `session_id`, in `context`.
    pub fn new(
        context: &SecurityContext,
        session_id: SessionId,
        channel_id: ChannelId,
        thread: ThreadId,
    ) -> Self;
}
```

The plugin opens a sub-agent's thread with `owns_founders`, and `end_subagent` carries out
`thread::End` after checking that the work's agent is the giver; `Ending` and `wind_up` go. Tests:
`End` lets go, closes and deletes, an owned founder only; the plugin's ending, as tested today.

Changed on 2026-11-24, as the bodies were begun (see *History*): a flag alone does not tell a
closing whom to end - the founders are kept nowhere, and those taking part are not they once one
was let go by a closing that stopped half-way. The opening keeps the agents it owns instead
(`Opening::owned: Vec<AgentId>`, from `ThreadSpec::owns_founders`, the agents among the founders),
and no procedure of its own is added: `thread::Close` closes a thread that owns agents with them -
each let go from the session, the thread closed, each deleted -, read from the channel's state, so
that asking again finishes it also once the thread is closed. A thread that owns nobody closes as
before.

### Step 2 - The giver's word, followed by the channel

```rust
// caelumora-domain/src/channel/thread.rs
pub struct Delegation {
    pub giver: ActorId,
    pub recipient: ActorId,
    pub initiator: ActorId,
    pub briefs: VecDeque<Brief>,
    /// What came of the last brief, given back while no brief is open: the giver's word on it
    /// is due.
    pub awaiting: Option<Awaiting>,
    /// Whether it was let go: its giver's word was none, or the giver left. It takes no brief.
    pub let_go: bool,
}

/// The answer given back on which the giver's word is due, and where the giver read it.
pub struct Awaiting {
    pub answer: MessageId,
    pub read_at: Option<Position>,
}

impl Delegation {
    /// Follows `reader` reading `message` at `at`: the answer awaited, read by its giver there.
    /// Whether it moved on.
    pub fn follow_read(&mut self, reader: ActorId, message: MessageId, at: Position) -> bool;

    /// Follows a turn of `ender` ending at `at` in the thread it was opened at: let go if it is
    /// the giver's, ending after it read the answer awaited, with no brief given since. Whether
    /// it was let go.
    pub fn follow_turn_end(&mut self, ender: ActorId, at: Position) -> bool;
}

// caelumora-domain/src/channel/state.rs
impl ChannelState {
    /// Follows a giver reading what came of its brief, as `entry` records it at `at`; the thread
    /// whose delegation it moved on.
    pub fn follow_read(&mut self, entry: &HistoryEntry, at: Position) -> Option<ThreadId>;

    /// Follows a giver's turn ending, as `entry` records it at `at`; the thread whose delegation
    /// it let go.
    pub fn follow_turn_end(&mut self, entry: &HistoryEntry, at: Position) -> Option<ThreadId>;

    /// Lets go the delegations `giver` gives, as it left the channel; the threads let go.
    pub fn let_go_of(&mut self, giver: ActorId) -> Vec<ThreadId>;
}
```

`Delegation::follow` sets `awaiting` as it takes the last brief out; `ThreadState::delegate` clears
it as a brief comes, and takes none into a delegation let go. `ChannelLoop::record` follows the
reading and the turn's end beside `follow_given`, storing the state with the entry;
`announce_removal` lets go what a giver leaving gave. A delegation let go is announced on the bus
(`channel::DELEGATION_LET_GO`, carrying the thread, by `ChannelController`), and again as the
channel starts, for every delegation the state holds let go.

`ChannelLoop::delegate` waits until `run` has recorded the brief (`carried_out`, as `close_thread`
waits), so that a brief delegated in the turn that read the answer is recorded before that turn's
end; one into a delegation let go is refused (`ChannelError::DelegationLetGo`), which the plugin
answers as `NoSubAgent`. What the delegate gives back says, in a thread that owns its founders, that
the recipient is let go unless it is given more before the turn ends (the kernel's prompts,
`kernel.delegation_*`); the description of `start_subagent` says it too.

Tests, in the domain: a delegation awaits the giver's word once its last answer is given back;
reading it, then a turn ending, lets it go; a brief before the turn's end, a turn of another, a
turn in another thread, a turn that ended before the reading, do not. In the channel: a brief
delegated in the reading turn is recorded before its end; a turn given up leaves the word due; a
giver leaving lets go; what is let go survives a restart and is announced again.

### Step 3 - The follower ends what was let go

The `AgentFollower` follows delegations let go as well, carrying out `thread::Close` for each, and,
after it lost events, for every delegation let go in the running sessions
(`ChannelLoop::delegations_let_go`). Its one sentence changes - it follows what ends or comes back
from outside a procedure - and its name with it (proposed: `Follower`), so that every program that
runs sessions still starts one. Test, with a sub-agent of `caelumora-subagents`: its answer read and
the turn ended without `ask_subagent`, it is let go from the session, its thread closed, the agent
deleted; asked again before, it stays.

### Open for the interface review

- **In whose name `thread::Close` runs from the follower**: the delegate's context
  (`SecurityContext::of_delegate`, answered for by the channel's owner), as the delegate acts - to
  be checked against [PERMISSION-CHECKS.md](../docs/PERMISSION-CHECKS.md).
- **The follower's name**, if it is renamed.

### Documents and stored data

CHANNELS.md (*Delegating into a thread*: the giver's word, a delegation let go, a thread owning its
founders), TOOLS.md (*Sub-agents*), EVENTS.md (`channel::DELEGATION_LET_GO`), ARCHITECTURE.md where
it names the follower. The channels' stored states have to be deleted: `Opening` and `Delegation`
gain fields without a default.

## History

- **2026-11-18** — Opened from issue 558's open points (*An idle sub-agent*).
- **2026-11-24** — Taken up. Decided by the maintainer (*Decision*): the giver's word is due once
  it read the answer, followed by the channel from `MessageGiven` and the turn's end; a delegation
  let go ends a thread made for its founders, by a procedure of the kernel the follower carries
  out. Weighed and rejected: a timeout, a new start ending idle sub-agents, an obligation kept by
  the agent, a hook of the event queue. Plan written in three steps, its interfaces to be agreed.
- **2026-11-24** — Step 1, changed by the maintainer as it was begun: the opening keeps the agents
  it owns (`Opening::owned`), and `thread::Close` closes a thread with them, in place of a
  procedure `thread::End`. Built: `ThreadSpec::owns_founders`, `Opening::owned` (`Opening` no
  longer `Copy`), `ChannelLoop::owned_by`, `thread::Close` letting the owned agents go, closing
  and deleting them - whether it may close the thread asked ahead, with the owned agents aside
  (`ChannelLoop::require_closing`, taken out of `close_thread`), so that nobody is let go for a
  closing refused -; the sub-agents' start opens its thread owning the sub-agent, `end_subagent`
  closes it; the plugin's `Ending` gone, and with it the last users outside the kernel of
  `close_thread`, `let_go_from_session` and `lock_letting_go`, now `pub(crate)`.
- **2026-11-24** — Decided by the maintainer on step 1's review: a thread owns only agents made for
  it alone - `thread::open` refuses to found a thread owning an agent that takes part in the
  session already (`SessionError::AlreadyParticipant`), so that no closing deletes an agent that
  was there before the thread.
- **2026-11-24** — Step 2 built: `Delegation::awaiting` and `let_go`, `follow_read`,
  `follow_turn_end` and `follow_leave`; `ThreadState::delegate` taking no brief into a delegation
  let go; `ChannelLoop::record` following the giver's reading and its turn's end or leave;
  `channel::DELEGATION_LET_GO`, announced once stored and again as the channel starts;
  `ChannelLoop::delegate` waiting until `run` recorded the brief, refusing one into a delegation
  let go (`ChannelError::DelegationLetGo`, which the plugin answers as `NoSubAgent`). Changed
  from the plan as it was built: the channel's state follows the giver by
  `ChannelState::follow_read(reader, entry, at)` - the entry of what an agent was given names no
  reader - and `follow_giver(entry, thread, at)`, one rule for a turn's end and a leave, in place
  of `follow_turn_end` and `let_go_of`; only a delegation into a thread that owns its recipient is
  let go, so that no other delegation stops taking briefs with nothing to end it; the giver is told
  the rule by the descriptions of `start_subagent` and `ask_subagent` alone, not by what the
  delegate gives back, which is the recipient's words. The scripted model of the agents' tests
  answers what it is called with as well (`Scripted::answering`, `register_answering`), so that a
  test's lead can ask the sub-agent of the thread its start returned. On review: the giver leaving
  the thread the delegation was opened at - alone, or with the channel, which records its leave
  there too - lets it go, since nothing comes back to it any more; a brief `run` does not take
  is refused to the giver (`run` answers only one taken, `delegate` asks why not) instead of
  dropped behind an `Ok`. A turn given up is held by a test of the domain alone.
- **2026-11-24** — Decided by the maintainer on step 2's review, before its commit: the giver's
  word is one state of the delegation, not two fields - `Delegation::word`, a `Word` of `NotDue`,
  `Due { answer, read }` and `LetGo` -, and where the giver read the answer is not kept: the
  channel follows the entries in the order of its history, so that a turn's end it follows after
  the reading always comes after it (`read: bool`; `follow_read` and `follow_turn_end` without a
  position, `ChannelState::follow_read` and `follow_giver` likewise).
