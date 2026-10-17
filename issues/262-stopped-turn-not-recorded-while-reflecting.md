# 262 — An agent removed while its turn ends leaves no end in the channel's history

**Status:** done
**Area:** Sessions and channels
**Summary:** An agent removed while its turn ends leaves the turn's messages in the channel's history without an end.

## The problem

An agent removed from a channel cannot tell the channel that its turn ended: the channel drops what
a removed actor still sends. So `ChannelLoop::remove_participant` asks the actor for its turn under
way before it stops it, and records that turn as given up, `Stopped`
([258](258-what-was-said-is-lost.md)). `RunControl::turn_under_way` names a turn only while it is
interruptible - planning, running tools, waiting for a person. An agent removed while its turn
reflects or is being cancelled - taken over, its `TurnFinished` not yet sent - leaves the turn in
the channel's history without an end; the agent's own chains record it, as given up.

## Why it matters

The channel's history then holds messages of a turn that never ends there; a reader sees an answer
without its end, and the web UI no notice. Rare - the window is one step - but the record is wrong.

## Considerations

- `Actor::turn_under_way` could name the turn in every state of a turn, not only the
  interruptible ones; the retry of a turn, which asks the same, would then have to check the state
  itself.
- Or the channel records `Stopped` for the last turn of the participant whose messages have no end
  yet, from its own history.
- Or the agent says so itself: it gives its turn up when stopped already, in every state, and only
  leaves the event out (`AgentActor::give_up`), since the channel would drop it. For that the
  channel has to take what a leaving actor sends as long as its task runs, and announce the removal
  once the task has ended - after the farewell, so that still nothing of the actor follows the
  announcement ([162](162-ui-keeps-a-removed-agents-turn.md)). An actor aborted after the grace
  says nothing, and its turn stays without an end - which is what happened.

## Decision

Only the actor that had a turn says whether and how it ended; the channel records no end in
anyone's name. A removed actor's events are taken as long as its task runs - the step it was in,
and its farewell - and the removal is announced when the task has ended, or was aborted after the
grace. A turn whose agent was aborted stays open in the channel's history: the agent never ended
it. The record the channel made itself since 258 is retracted.

## Plan

Written against the code as of `ee40d42`. One step with a change in behaviour - what the channel
takes from a leaving actor - and the documents on the subject; then the close.

### A. What the code shows

- **Removing.** `remove_participant` stores the removal, moves the actor out of `actors` into
  `leaving` (its `TaskTracker`, for [171](171-readded-while-still-stopping.md)), tells the
  moderator, asks the actor for its turn under way, stops it and sends `ActorChange::Stop(id,
  remover, under_way)`. `run` then drops the actor from `running` - which closes its inbox - spawns
  a task that aborts it after `STOP_GRACE`, announces `ParticipantRemoved` at once and records
  `TurnGivenUp { Stopped }` itself, in the participant's name, if a turn was named.
- **Dropping.** `handle_actor_event` drops every event whose sender is not in `running`; the
  `biased` select routes the events before any change of the actors.
- **The agent's stop.** The run loop sees a stop after the step it interrupted
  (`take_interrupt`): `give_up(Stopped)` records the end in the agent's own chains and returns
  before it withdraws the turn's requests and sends `TurnGivenUp` (`mod.rs`, "unless the agent was
  stopped"). A turn in `Reflecting` is finished by that step - `commit` sends `TurnFinished` - and
  there is nothing left to give up: its end was sent, and dropped.
- **The external agent's stop** says nothing either: `Ending::Stopped => Vec::new()`
  (`external_agent.rs`), though it cancelled an answer under way.
- **The gate** does not ask for membership: `ActionType::Publish` is the actor publishing in its
  own name and the classification flowing (`security.rs`, `check`); `admits_change` names the
  author from the history. A farewell passes as any event of the actor.
- **The moderator** routes `TurnGivenUp` to the users and `TurnFinished` through `ask` by kind,
  not by knowing the sender; `remove_actor` only takes the actor out of the candidates. A
  `TurnFinished` of a leaving agent would ask the others to answer, as any finished turn.
- **The approver** denies a leaving agent's pending approvals as it follows the participants
  (issue 14), and ignores a withdrawal of what it no longer holds.
- **The frontends** end an answer on `TurnGivenUp` (`answerEnded`) and, as a backstop, on
  `ParticipantRemoved` (the answers of the removed, its calls without a result marked abandoned);
  `givenUpText` has a text for `stopped` since 258. The TUI prints both.
- **Tests that pin the old rule:**
  `a_removed_agent_abandons_its_turn_and_leaves_an_idle_state_behind` ("a stopped agent announces
  no answer"), `nothing_of_a_removed_participant_enters_the_channel` (an empty `running`),
  `an_agent_removed_while_a_tool_runs_gives_up_its_turn_and_keeps_the_call` (the end recorded by
  the channel), `an_external_agent_removed_while_it_answers_cancels_its_task_and_says_nothing`.

### B. Decisions

1. **The farewell is the actor's.** `AgentActor::give_up` treats `Stopped` as every other reason:
   it withdraws the turn's open approval requests and questions and sends `TurnGivenUp`. A turn
   that no longer flows is still recorded and told as `Lowered` (`turn_flow`), whatever ended it.
   The external agent sends `TurnGivenUp { Stopped }` for an answer it cancelled when stopped
   (`Ending::Stopped`). One rule for both: how a turn ended is told by whoever had it.
2. **The channel takes what a leaving actor sends until its task has ended.** `run` keeps the
   actors told to stop in a map of its own beside `running` - `leaving`, local to `run`: the
   actor's id, who removed it and the id of its task (`AbortHandle::id`). `handle_actor_event`
   routes an event whose sender is in either; nothing is delivered to a leaving actor, since its
   `RunningActor` - its inbox - is dropped as today. The waiting task that aborts after
   `STOP_GRACE` stays as it is.
3. **The removal is announced when the task has ended.** `run` takes the join results with their
   task id (`JoinSet::join_next_with_id`; an aborted task ends with a `JoinError` that carries it
   too) and, for a task in `leaving`, announces `ParticipantRemoved` in the remover's name and
   forgets the actor. Events before join results (`biased`): everything the actor sent before it
   ended is routed first, and nothing of it follows the announcement - the rule of 162 holds, the
   actor's part now ending with its farewell. Also before `ActorChange::Start` of an actor under an
   id still in `leaving`: `add_participant` waited on the tracker, not on `run`, so the removal is
   announced there if the join result was not seen yet - the announcement precedes the new actor
   either way, without relying on which of the two notifications `run` sees first.
4. **The channel asks for no turn.** `ActorChange::Stop` carries the actor and its remover only;
   `remove_participant` no longer calls `turn_under_way`, and the channel's own `TurnGivenUp` goes.
   `Actor::turn_under_way` stays for what it was made for: retries and the member's snapshot.
5. **A stopped channel announces nothing** (`stop_actors`, deletion): it is gone. The farewells
   the actors send then are routed or not, as the teardown goes; nothing depends on them.
6. **An actor aborted after the grace** says nothing. Its turn stays in the channel's history
   without an end, as the frontends' backstop on `ParticipantRemoved` shows it; no record is made
   up for it.

### C. The runtime (`caelumora-runtime`)

- `channel.rs`: `ActorChange::Stop(ActorId, SecurityContext)`; `run` with `leaving` and
  `join_next_with_id`; `apply_change` - `Stop` moves the actor out of `running` into `leaving` and
  spawns the waiting task, `Start` announces a removal still pending under the id first; the join
  arm announces; `handle_actor_event(event, running, leaving)`; `remove_participant` without the
  question. The docs of `remove_participant`, `ActorChange`, `handle_actor_event`, `run`'s select
  comment, and `left`.
- `actor/agent/mod.rs`: `give_up` without the early return for `Stopped`; its doc and the run
  loop's comment at `Interrupt::Stop`.
- `actor/external_agent.rs`: `Ending::Stopped` told as `TurnGivenUp { Stopped }`.
- `actor/mod.rs`: the doc of `Actor::stop` - the actor tells the channel of the turn it gives up.
- Tests (`channel.rs`):
  - `a_removed_agent_gives_its_turn_up_says_so_and_leaves_an_idle_state_behind` (from
    `…abandons_its_turn…`): `TurnGivenUp { Stopped }` from the agent, no `TurnFinished`.
  - `a_removal_is_announced_in_the_channel_in_the_name_of_who_removed`: after the agent's
    farewell, and nothing of the agent after it.
  - `nothing_of_a_removed_participant_enters_the_channel`: one whose task has ended. New beside
    it: `what_a_leaving_actor_sends_enters_the_channel_until_its_task_ends`, at
    `handle_actor_event` with the actor in `leaving`.
  - `an_agent_removed_while_a_tool_runs_gives_up_its_turn_and_keeps_the_call`: the end in the
    channel's history is the agent's event (its sender), not the channel's.
  - `an_agent_removed_while_waiting_for_approval_gives_up_its_turn`: the approval request is
    withdrawn, as for any turn given up.
  - `an_external_agent_removed_while_it_answers_cancels_its_task_and_gives_its_answer_up` (from
    `…says_nothing`).
  - `a_participant_is_added_again_only_once_its_old_actor_has_ended`: the removal is announced
    before anything of the new actor.
  - The window itself - a removal while the turn reflects - if a step in `Reflecting` can be held
    by a component of the test; else the mechanism is pinned at `handle_actor_event` and through
    the long tool, and the window is covered by construction: whatever the step, the end is sent
    and taken.
- The security suite: unchanged. Removing asks what it asked
  (`removing_an_agent_needs_owning_the_channel`); the farewell is the agent's own event, judged at
  the gate as every event it sends.

### D. The frontends

Nothing to change: the chat shows a turn given up as stopped with its notice and ends the answer,
as for any reason; `ParticipantRemoved` still ends what an aborted actor left. The TUI prints both
as it does.

### E. Documents

- CHANNELS.md: *Acting in a channel* ("Nothing said is unsaid": the end of a removed agent's turn
  is its own), *What changes in the running channel* (a removed actor stopped at once, its
  farewell taken), *How others learn of it* (announced once the task has ended; nothing follows;
  the aborted actor), *Stopping an actor* if it says a stopped agent announces nothing.
- AGENT-TURN.md: *Storing and resuming* ("The agent announces nothing, since it has left"),
  `Turn::give_up` ("only a stopped agent leaves them to the approvers").
- WEB-UI.md: the sentence at `ParticipantRemoved` - a turn given up for a stop is shown as any
  other, the announcement ends what an aborted agent left.
- Issues: [162](162-ui-keeps-a-removed-agents-turn.md) (the announcement now follows the actor's
  end), [171](171-readded-while-still-stopping.md) (announced before the new actor starts, by the
  `Start` arm too), [258](258-what-was-said-is-lost.md) (the channel's own `Stopped` record
  retracted).

### F. Steps

1. **The change** (C, D, E): one commit. Tests pass; `cargo clippy` is silent. No stored data
   changes.
2. **Close**: this file's history, status `done`.

### G. What to watch

- **A deletion** stops every actor (`stop_actors`) before the channel's task is aborted: the
  farewells go into a channel being torn down and are recorded or not; the channel is gone.
- **A farewell after a lowering** is told as `Lowered`, as `give_up` records it - the users learn
  why the answer is gone, not that the agent left, which `ParticipantRemoved` says.
- **The re-added participant** ([171](171-readded-while-still-stopping.md)): `left` waits on the
  tracker, which ends with the task; `run` announces the removal on the join result or on the
  `Start`, whichever comes first, and the old actor's events were routed before either.
- **Aborted tasks** end with a `JoinError`; its task id must still reach the announcement, so the
  join arm handles both outcomes.
- **The waiting task** that aborts after `STOP_GRACE` and `LEAVE_GRACE` stay as they are.

## Done when

- Every turn whose messages are in the channel's history has its end there as its agent told it,
  also when the agent was removed while the turn ended. Only a turn whose agent was aborted after
  the grace stays without one, since nobody ended it.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), whose plan noted it
  under "What to watch".
- **2026-10-08** — Decided with the maintainer, against both considerations: only the actor that
  had a turn says how it ended. The channel takes what a leaving actor sends until its task has
  ended and announces the removal then, so that the agent's farewell - `TurnGivenUp { Stopped }`,
  or the `TurnFinished` of a turn that was reflecting - enters the channel as its own. The record
  the channel made itself since 258 is retracted; a turn whose agent was aborted stays open.
  Planned against the code as of `ee40d42` (parts A to G).
- **2026-10-08** — Step 1 done, as planned: `AgentActor::give_up` withdraws the requests and sends
  `TurnGivenUp` for a stop as for any reason, the external agent tells `Ending::Stopped` as
  `TurnGivenUp { Stopped }`; `run` keeps the actors told to stop in `Leaving` - who removed each,
  and its task's id - takes their events until their task has ended, and announces the removal
  from the `JoinSet`'s result (`join_next_with_id`, an aborted task's `JoinError` carrying the id
  too) or before an actor starts under the same id; `ActorChange::Stop` carries no turn, and
  `remove_participant` asks for none. One deviation: the window itself - a removal while the turn
  reflects - has no test of its own, since no component of the tests holds that step; the
  mechanism is pinned at `handle_actor_event` and through the long tool, the `Start` arm at
  `apply_change`. Nothing to change in the frontends.
- **2026-10-08** — Restructured at the maintainer's request, without a change in behaviour: an
  actor told to stop stays in `run`'s `running` until its task has ended - its inbox gone
  (`RunningActor::sender` is `None`, nothing is delivered to it) and who removed it kept
  (`RunningActor::leaving`) - instead of being moved to a map of its own. `handle_actor_event`
  asks `running` alone, as before 262; the task that ended is found by its `AbortHandle`'s id.
  Weighed and not taken: keeping the actor in `ChannelLoop::actors` as well, until its task has
  ended - `actors` says who takes part, as the stored channel does, and its readers (`members`,
  the mentions, `wakeup`, `actor_by_id`) would each have had to leave a leaving actor out.
  `ChannelLoop::leaving` stays, for `add_participant` to wait on (171). A task that ends on its
  own is now dropped from `running` as well, where before its entry stayed.
- **2026-10-08** — Step 2, closed: every turn whose messages are in the channel's history has its
  end there as its agent told it, also when the agent was removed while the turn ended; only a
  turn whose agent was aborted after the grace stays without one.
