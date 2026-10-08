# 260 — A turn keeps its messages apart from the transcript until it ends

**Status:** in progress
**Area:** Code structure

## The problem

A turn's messages live in `TurnState.entries` until the turn ends; only then do `record_turn` and
`give_up_turn` push them into the agent's transcript and history. Each entry is given the digest it
will have in the transcript ahead of time, and a `TranscriptView` continues the stored transcript
with the turn's entries for whatever reads it during the turn. Since
[258](258-what-was-said-is-lost.md), every message a turn says stays, however the turn ends - so the
reason to keep them apart, that a turn given up should leave the active path untouched, is gone.

## Why it matters

Two places hold what an agent said, and every reader of the agent's context has to put them
together (`TranscriptView`, `Turn::full_context`, the warning in `record_messages` that the
transcript changed during a turn). The working state stored after every step carries the turn's
messages a second time.

## Considerations

- **Push each message into the transcript and history as it is said** (`Turn::say`), and keep in
  `TurnState` only what is staged - notes, memory - and the turn's bookkeeping. The prompt parts a
  turn places among its messages (issue 49) are referred to by the digest of the entry they follow,
  which works on the transcript as well.
- Which messages are a turn's is told by their `turn_id` (issue 241), so nothing has to count the
  turn's entries.
- A turn resumed after a restart then finds its messages in the transcript, not in its state.
- Without behaviour change: a restructuring of its own.

## Plan

Written against the code as of `6b7cdad`. A restructuring without a change in behaviour, in one
step, as [258](258-what-was-said-is-lost.md) step 3 was: the model, the runtime that follows it,
and the documents on the subject.

### A. What the code shows

- **Who reads `TurnState.entries`.** In production three places: `TranscriptView`
  (`caelumora-model/src/context.rs`), which continues the stored transcript with the entries for
  `ContextResolver::resolve` - so `Turn::full_context` - and for `usage`; `Turn::commit`, which
  takes the turn's answer from them (`final_messages`); and `record_messages`, through
  `record_turn` and `give_up_turn`. Everything else is a test.
- **How an entry is made.** `TurnState::push` sets the message's `turn_id`, keeps a copy of
  `Work::context` as what the message was produced from, computes the digest the message will
  have in the transcript - `digest::message_chain` over `context.transcript` - and moves
  `context.transcript` on to it. `Chain::push_with` computes a digest the same way, over the
  chain's head: pushing live gives the same digests, as long as the head is what the context
  names. `a_turn_under_way_is_resolved_as_its_messages_are_recorded` asserts that already.
- **The head is what the context names.** Nothing is perceived during a turn: the actor takes in
  what others said between turns (`take_in`), `WaitingForUser` buffers every other event,
  `catch_up` runs before the actor is spawned, a retry's revocations happen before `begin_turn`.
  Two places allow for the opposite: `AgentChannelState::perceive` has a branch for a turn under
  way (`see_message`), and one runtime test perceives during a turn. `record_messages` warns if
  the transcript changed meanwhile.
- **Prompt parts placed within a turn** (`place_turn_prompt`, issue 49) take `after` from
  `Work::context.transcript`, never from the transcript itself; `advance_plan` places both of its
  instructions so.
- **What a resumed turn reads.** `prepare_resume` reads the processing state, the tool calls and
  the turn's classification; `full_context` needs the entries only to resolve
  `context.transcript`, which names the last entry.
- **What is stored.** `AgentChannelStateChange::Turn` carries the turn whole at every step, its
  messages with it; the transcript and the history tell only what was added.
- **What a reader sees mid-turn.** `AgentContextView` reads the stored state: `history`,
  `has_context`, `in_transcript` and `context_of` know nothing of the turn's messages until it
  ends, and the web UI's history panel shows them only then, though the channel's history has
  held them as they were said since 258.

### B. Decisions

1. **The rule lives in the model, as the others do.** `AgentChannelState::said(message)` records
   what the turn under way said: sets the message's `turn_id`, pushes it into the transcript,
   into the history with the context it was produced from, and moves `Work::context.transcript`
   on to the transcript's head. `Turn::say` is that and the publishing (`Turn::push_message`
   keeps its name for the recording half, which the tests call). `TurnState::push`, `TurnEntry`
   and `entries` go.
2. **`TurnState` holds no message.** The turn's answer at `commit` is read from the transcript,
   `Transcript::of_turn(turn_id)` - the messages of the active path that carry the id - not kept
   as a list beside it; `Turn::messages()` reads the same. Until
   [261](261-turn-finished-repeats-the-answer.md) takes the payload off `TurnFinished`.
3. **`record_turn` becomes `finish_turn`.** It records the messages no more; it ends the turn
   (`TurnFinished` on both chains) and applies the notes. `give_up_turn` only ends it. Same
   concept, same name as the entries and the events.
4. **`TranscriptView` goes.** `ContextResolver` resolves against the transcript itself, `usage`
   sums over `transcript.all_entries()` - the turn under way is in it now.
5. **The head is what the context names, by construction.** `perceive` moves
   `Work::context.transcript` on when a turn is under way, so that the invariant the warning in
   `record_messages` watched holds without a warning; the branch stays unreachable in production.
   Otherwise an entry pushed after a perceived message would carry a context that leaves the
   perceived message out.
6. **No migration of a turn stored before the change.** `TurnState` has no `deny_unknown_fields`:
   a stored turn's `entries` are dropped on load, its context names a digest the transcript lacks,
   its first step fails and it is given up - its messages lost to the agent, though the channel's
   history holds them (258). AGENTS.md: no readers of an older form. The summary says so.
7. **`begin_turn` with a turn left behind** cannot happen - a turn ends before the state is
   stored idle - and is a bug if it does: `debug_assert!` that none is under way, and in release
   a `warn!`. The messages of such a turn stay on the active path without an end, which no reader
   minds.

### C. The model (`caelumora-model`)

- `turn.rs`: `TurnState` without `entries`, `push` and `messages`; `TurnEntry` gone. The doc of
  `TurnState` and of `Work::context` says: the context names the transcript's head.
- `agent.rs`: `said`, `finish_turn(turn_id, actor, notes, private_notes)`,
  `give_up_turn(turn_id, actor, reason)`, `record_messages` and its warning gone; `perceive` moves
  the context on; `usage` over the transcript; `begin_turn` asserts no turn under way.
- `transcript.rs`: `of_turn(turn_id) -> Vec<Message>`; the doc of `modify` says why not during a
  turn (the context names the head, and the actor takes changes in between turns).
- `context.rs`: `TranscriptView` gone; `ContextResolver::of` holds the transcript; module doc.
- Tests: `a_turn_under_way_is_stored_with_the_state_and_none_between_turns` (the turn stored
  without its messages),
  `a_turn_recorded_continues_the_transcript_and_the_history_and_ends_finished` (said as it goes,
  context recorded at once), `a_turn_given_up_keeps_what_it_said_and_ends_given_up`,
  `all_that_is_heard_is_kept_and_what_is_perceived_is_seen_by_a_turn_under_way` (the context
  moves on), `a_turn_under_way_is_resolved_as_its_messages_are_recorded` (resolved from the
  transcript, same digests), `usage_sums_the_agent_s_own_calls_on_every_branch_and_in_its_turn`.

### D. The runtime (`caelumora-runtime`)

- `agent/turn/mod.rs`: `push_message` calls `said`; `messages()` reads `of_turn`; module doc.
- `ending.rs`: `commit` takes `final_messages` from the transcript, calls `finish_turn` with the
  notes; `give_up` calls `give_up_turn` with the reason; docs.
- `context.rs`: `full_context` unchanged in what it does.
- `actor/agent/mod.rs`: nothing but what resolves through the turn; `store_channel_state` after
  every step now writes the messages once, as transcript and history changes.
- Tests: `ending.rs`, `turn/context.rs`, `turn/plan.rs`, `tool_calls.rs` (`turn.messages()` from
  the transcript); `channel.rs` - `a_turn_is_stored_with_the_step_that_comes_next`,
  `an_agent_removed_during_a_turn_gives_it_up`, `lowering_the_channel_gives_up_a_running_turn` -
  poll the stored transcript for the tool result instead of `state.turn…messages()`;
  `a_turn_has_seen_its_transcript_its_prompt_parts_and_its_tool_results` perceives during a turn
  and asserts the context moved on. New: a turn resumed in `Planning` after a restart resolves
  its context from the transcript; the history panel's view (`AgentContextView`) shows a turn's
  messages with their context before the turn ends.

### E. Documents

- AGENT-TURN.md: *Beginning and ending a turn* (`commit`, `give_up`), *Storing and resuming*
  (the turn under way holds no message), *The turn's context and the resolver* (*The turn's
  messages*, *Resolving*, *Recording*; the stale "how many of the transcript's last entries are
  the turn's own"), *Tokens used*, the file list at the top.
- STORAGE.md: what the turn under way holds; the small parts recorded whole.
- MEMORY.md: the rows *Context* and *Turn state*.

### F. Steps

1. **The restructuring** (C, D, E): one commit, no change in behaviour but what A's last point
   says - the agent's history, and so the panel and the contexts, show a turn's messages as they
   are said. Tests pass; `cargo clippy` is silent. Stored data: agent channel states holding a
   turn under way at the upgrade - stop the server between turns, or delete them.
2. **Close**: this file's history, status `done`; 261 notes that `of_turn` is what its first
   option would gather.

### G. What to watch

- **Digests.** The same function over the same predecessor: every `ContextRef` recorded before
  resolves as before. `digests_are_kept_as_recorded` and the resolver's test pin it.
- **A step is one state write**, as before: the message in the transcript, in the history and the
  turn's context moved on go into the store together; a crash before the write loses the step
  whole, as `Storing and resuming` describes.
- **`said` before `send`**: as `say` does today, a message recorded whose publish fails stays
  recorded and the turn fails; nothing changes there.
- **More is seen mid-turn**: `has_context` and `in_transcript` are true for a message as soon as
  it is said, `recorded_context_of` finds it, the history panel reloads on every message. The
  `retried` of a message walks the transcript between turns only.
- **A turn stored before the change** fails its first step on resume (decision 6).
- **`TurnFinished` still carries the answer**, now read from the transcript (261).

### H. Not in this plan

Taking the payload off `TurnFinished` (261); deriving `has_context` from the transcript; the
external agent's work, which keeps no turn under way.

## Done when

- A turn's messages are in the agent's transcript and history from the moment they are said, and
  `TurnState` holds no message.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), from part J of its plan.
- **2026-10-08** — Planned, against the code as of `6b7cdad` (parts A to H): one step, the rule as
  `AgentChannelState::said`, `TurnState` without a message, `record_turn` renamed `finish_turn`,
  `TranscriptView` gone, `perceive` moving the turn's context on, no migration of a turn stored
  before. The mention of `ContextRef::turn_entries` above was wrong: it went with issue 241; which
  messages are a turn's is told by their `turn_id`.
- **2026-10-08** — Step 1 done, as planned: `AgentChannelState::said` records what a turn says
  into the transcript and the history at once and moves the turn's context on to the head;
  `TurnState` holds no message, `TurnEntry` and `TurnState::push` are gone; `finish_turn` and
  `give_up_turn` only end the turn; `TranscriptView` is gone, the `ContextResolver` and `usage`
  read the transcript; `Transcript::of_turn` gives what a turn said, `commit` its answer from it;
  `perceive` moves a turn's context on. One deviation: `begin_turn` with a turn under way only
  warns - the `debug_assert!` planned would have failed the many tests that begin a turn on a
  state as their setup, without finding anything. New tests: a turn stored under way resolves its
  next context from the transcript; the agent's view shows a turn's message with its context
  before the turn ends.
