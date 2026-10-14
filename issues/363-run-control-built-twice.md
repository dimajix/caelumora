# 363 — Where an agent's actor stands, and what `cancel` and `stop` asked of it, is built twice

**Status:** done
**Area:** Code structure

## The problem

[355](355-two-agent-actors-share-half-their-fields.md) gave both kinds of agent one `AgentBody`.
What it left with each actor is where the actor stands and what `cancel` and `stop` asked of it,
and that is written twice:

| | `AgentActor` | `ExternalAgentActor` (`actor/external_agent.rs:87`) |
|---|---|---|
| where it stands | `RunControl::state` (`actor/agent/run_control.rs:39`) | `processing` |
| telling the users | `RunControl::unannounced`, by the run loop before every step (`announce_state`) | `go_to`, at once, when the state moves |
| ending the work under way | the turn's token (`RunControl::begin_turn`, `token`) | `current`, replaced for every answer (`begin`) |
| `cancel` | only in a turn that can still be interrupted, left as an `Interrupt` the run loop takes after the step | cancels `current`, whatever it stands at |
| `stop` | `Interrupt::Stop` and the turn's token; the run loop gives the turn up and ends | cancels `ended`, the parent of `current`; `run` waits on it next to its inbox |

The two hold the same concern - "where the run loop stands, and what other tasks asked of it" - and
differ where nothing in the two kinds of agent asks for it. The cost is visible since 355: the loop
"carry out the command where it came, take in the rest of the batch" stays in each actor
(`AgentActor::handle_waiting` with `carry_out`, `ExternalAgentActor::hear`), five lines each,
because the one line that differs is how each shows `CarryingOutCommand` - the `AgentActor`
only when awake, announced with its next step, the external agent always and at once. The body
cannot own the loop without a callback or a trait for that line.

## Why it matters

Every change to how an agent's actor shows where it stands, or how it ends its work, is made
twice and in two shapes - the announcing of issue 243, the cancelling of issue 152, the stopping of
issues 75 and 262. A third kind of agent would bring a third.

## Considerations

- **One run control for both.** The `ExternalAgentActor` gets a `RunControl` too: its state
  instead of `processing`, its announcing (`unannounced`) instead of `go_to`, a turn's token for
  every answer (`begin_turn` with the answer's `Work::turn_id`) instead of `current`. What it lacks
  is a way for `run` to wait for a stop between requests - the external agent follows open tasks
  there, not only its inbox -: `RunControl` would get one (`stopped()`, say, a token that `stop`
  cancels and every turn's token is a child of), which also replaces `ended`. `RunControl` moves
  from `actor/agent/` to `actor/`.
- **Where it then lives.** Either each actor keeps its `RunControl`, and the command loop moves
  into `AgentBody` taking it as a parameter; or the run control is part of the body - where an
  agent stands in the channel is part of what it is there -, and the body owns the loop outright.
  An architecture decision, to be discussed before it is built.
- **What must stay as it is.** A `cancel` of the external agent between requests ends nothing
  today either, since the answer's token was replaced; it must still not reach a task followed
  in the background. A `stop` must still cancel its open tasks at the remote agent and store its
  state (`run`, after the loop). The external agent reaches no `Sleeping`, which only a turn's stop
  tool requests, so "only when awake" changes nothing for it; whether it announces `Idle` after a
  command at once or with its next step is visible to the users and is to be decided, not slipped
  in.
- **Not the subject:** `turn_under_way`, `retry_turn` and `interrupt_tool_call` of the external
  agent. With a turn's token for every answer they come within reach, but what they would mean
  for a remote agent is a feature, not this refactoring.

## Done when

- One type holds where an agent's actor stands and what `cancel` and `stop` asked of it, for both
  kinds of agent.
- The loop that carries out a command where it came exists once.
- No behaviour changes beyond what was decided above; the channel tests of agents and external
  agents pass unchanged.

## History

- **2026-10-14** — Left over from [355](355-two-agent-actors-share-half-their-fields.md), opened at
  the maintainer's request under epic [349](349-core-review-epic.md).
- **2026-10-14** — Done. `RunControl` moved to `actor/run_control.rs` and is part of the
  `AgentBody`: where an agent stands in the channel is part of what it is there. The body builds it
  from the stored state, after its actor made that fit to run from - `AgentBody::build` takes how
  the actor resumes: the `AgentActor` its `prepare_resume`, the external agent `Idle`, since it
  resumes no answer. `RunControl` got a `stopped` token, cancelled by `stop` and the parent of every
  turn's token, so that an answer begun after a stop finds its token cancelled, as it did with
  `ended`; `run` of the external agent waits for it (`stopped`, `is_stopped`). For the `AgentActor`
  that changes nothing: a turn begins within a step, after which its run loop takes the stop. The
  external agent's `processing`, `current`, `ended` and `begin` are gone; every answer is a turn of
  the run control (`begin_turn` with the work's turn id), which `cancel` and `stop` end. `cancel`
  now asks whether the state can be interrupted first - between answers it ended nothing before
  either, the token it reached being the last answer's.

  The command loop is `AgentBody::hear`, once; `AgentBody::announce_state` and `AgentBody::store`,
  which stores where the actor stands, serve both. As decided, the external agent tells `Idle` after
  a command as an agent run here does: with its run loop's next step, so that a command followed by
  a request shows `CarryingOutCommand`, then `Planning`, no longer an `Idle` in between. The begin
  and end of an answer it still tells at once (`go_to`). Not a subject, as considered:
  `turn_under_way`, `retry_turn` and `interrupt_tool_call` of the external agent. Stored data: an
  external agent's `AgentChannelState` records where it stood when it was stored, which it does
  not read back; nothing has to be deleted.
