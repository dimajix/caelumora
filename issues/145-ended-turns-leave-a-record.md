# 145 — A turn that does not end regularly leaves no record of its tool calls

**Status:** done
**Area:** Tools and approvals

## The problem

What a turn did is kept in the store only when it is taken over
(`Turn::commit_turn`): its messages go into the agent's transcript, and
with the assistant messages and the tool messages, its tool calls and
their results. Every other end discards the turn's working state
(`Turn::abandon_turn`) and with it every tool call it made:

- a turn that **fails** (`AgentActor::fail_turn`) - after a tool ran,
  e.g. because the next model call fails;
- a turn that is **stopped**, because the agent leaves the channel
  (`AgentActor::run`, `stopping`);
- in future, a turn given up on a **branch switch**
  ([08](08-retry-and-edit-past-answers.md), plan point 4).

A tool call can have had its effect by then - a mail sent, a file
written. Afterwards, the store does not show that it happened, who
approved it, or what it returned; only the server log does.

A **cancelled** turn is taken over today (`handle_cancelling` →
`reflect` → `commit_turn`), so its tool calls stay. But taking it over is
what cancelling a turn on lowering its channel
([141](141-turn-during-reclassification.md)) must not do: the turn's
messages are classified as the channel was, and taken over they put
content above the channel into the agent's transcript. There, cancelling
needs an end that records what the turn did without taking it into the
conversation.

## Why it matters

- Tool effects are the one part of a turn that reaches outside the
  system ([20](20-transactional-effects.md)). Whether they are recorded
  must not depend on how the turn ended.
- An approval is a decision of a user about an effect; a record of it that
  disappears with a failed turn is no record ([140](140-audit-log.md)).
- Branching ([08](08-retry-and-edit-past-answers.md), plan point 8) needs
  to carry the tool calls with an effect of an abandoned branch into the
  new one as facts - from a record that the branch switch does not take
  with it.

## Considerations

- **Two ends, kept apart.** A turn is either *taken over* - its messages
  become part of the conversation - or *given up*. Either way it leaves a
  record of what it did. What is recorded and what the model is given
  afterwards are two questions, and only the first one is this issue's.
- **Where the record lives.**
  - *In the agent's state in the channel* (`AgentChannelState`), as a list
    of ended turns next to the transcript: stored with the rest, by the
    same `store_channel_state` that stores the turn today. Not a chain of
    the conversation, so a branch switch does not move it - the world does
    not branch. Grows with every turn; what it keeps of a turn that was
    taken over, whose calls are in the transcript already, is to be
    decided (all of them for one place to look, or only the given-up ones).
  - *In the transcript*, as messages marked as given up. One place, but
    the transcript is what the model is given, and the classification of
    a turn cut off by a lowering would be in it.
  - *In the audit log* ([140](140-audit-log.md)) or the recorder
    ([05](05-postgres-event-recorder.md)). Both exist only as a plan; the
    state store is what is there, and what the maintainer asked for.
- **What a record holds.** The call, whether and by whom it was approved
  or denied, its result, the classification of the turn, how the turn
  ended (cancelled, failed, stopped, branch given up), and the envelope it
  answered (for the `span_id` in the log). The turn's messages that were
  not taken over probably too - they say why the call was made.
- **Who reads it.** It holds what the turn read, at the turn's
  classification. Reading it is a question for the `SecurityManager`, like
  reading an agent's context (`ReadAgentContext`); it is not model input.
- **What is dispatched, not only what returned.** A record written before
  a call runs ([20](20-transactional-effects.md): "a durable record of
  this tool call was dispatched") would cover the crash in the middle of a
  tool as well, which `prepare_resume` answers today as "not executed"
  without knowing. The same record can serve both; whether this issue
  takes that on or leaves it to 20 is to be decided.
- **What the model is given afterwards.** Whether a given-up turn's
  effects are told to the agent in its next turn - as 08 plans for a
  branch - is a question of what the model sees, for 08 and 141; the
  record is what they build on.

## Done when

- Every turn that executed a tool call leaves a record of it in the store,
  however it ended.
- The record says who approved the call, what it returned, and how the
  turn ended, and it can be read by whoever may read the agent's context.
- A turn can be given up without being taken over and still be recorded,
  so that 141 and 08 can end turns that way.
- The agent's next turn knows the calls a given-up turn executed.

## History

- **2026-09-30** — Opened in the discussion of
  [141](141-turn-during-reclassification.md): cancelling a turn does not
  undo what its tools did, and the maintainer wants every turn - cancelled
  ones too - recorded in the state store, so that its tool calls stay
  auditable. Overlaps with [08](08-retry-and-edit-past-answers.md), which
  gives up the turns of an abandoned branch, and
  [20](20-transactional-effects.md).
- **2026-09-30** — Direction from the maintainer: a turn that is given up
  becomes a **dead branch** of the agent's transcript
  ([08](08-retry-and-edit-past-answers.md)), keeping its messages - not
  only the tool calls - also on `fail_turn`. No hard end may lose messages
  without leaving a branch: failing, stopping, cancelling on a lowering
  ([141](141-turn-during-reclassification.md)) and, later, a branch
  switch. `/cancel` stays as it is: it drops only the partial message the
  model is generating, which is not kept, and takes the turn over.
  What follows from it:
  - The chains have to be trees first (step 1 of 08's plan).
  - The branch hangs in the agent's transcript only; the channel's history
    has taken nothing over from a given-up turn.
  - How the branch ended (cancelled on a lowering, failed with its message,
    stopped) and the `ToolCallState`s of calls without a tool message
    (given up while waiting for approval) are kept with the branch; no
    message says them. `ToolCallState` does not record who approved - a
    gap of its own ([140](140-audit-log.md)).
  - Changes the turn staged to notes and memory are still not applied.
  - The model does not see a dead branch, so it does not know what a
    given-up turn's tools did. Whether executed calls are carried into
    the live branch as facts already here, or with 08 and
    [20](20-transactional-effects.md), is open.
- **2026-09-30** — Decided: the executed calls of a dead branch are
  carried into the live branch as facts already here, not only with 08 -
  otherwise a turn that failed after sending a mail sends it again. Until
  tools declare whether they have an effect
  ([20](20-transactional-effects.md)), every executed call is carried
  over.
- **2026-09-30** — Decided in the discussion before implementing: no type
  of its own for a given-up turn. Almost everything it would have held is
  in the tree or the messages already - where the branch forked, the
  classification, the calls and their results; what is new is only that
  and how the branch ended, and that is a mark on its last node.
- **2026-09-30** — Done. `Turn::abandon_turn` is `give_up_turn`: the
  turn's messages go into the transcript and the agent's history as
  `commit_turn` would put them, each with its `ContextRef`, and both heads
  go back to where the turn began. The transcript's last message of the
  turn carries `given_up` (`GivenUp`: the reason, `Failed { message }` with
  what the users were told or `Stopped`), the
  time, and how many messages were the turn's), outside the digest.
  Nothing the turn staged is applied. Calls that ran (their result has a
  `started_at`) are told on the active branch in an internal system
  message of the runtime in the transcript: with arguments and results,
  classified as the turn, while the turn may still flow into the channel
  (`ActionType::Publish`), else only by name, classified as the channel.
  `Actor::given_up_turns` (under `ReadAgentContext`) and
  `GET …/actors/{aid}/given-up-turns` list the turns; the context of their
  messages is resolved on their branch by the existing endpoint. The web
  UI is [146](146-show-given-up-turns.md). Deviations from "done when":
  who approved a call is not recorded - `ToolCallState` never held it, a
  gap left to [140](140-audit-log.md) as noted above - and a turn that
  produced no message leaves nothing, since there is nothing to keep.
- **2026-09-30** — `Stopped` briefly carried the `SecurityContext` of the
  act that stopped the agent, and was taken back: `stop`, `cancel` and
  `wakeup` are to be alike, and none of them knows its caller. Who removed
  the agent or deleted the channel belongs to that act, in the audit log
  ([140](140-audit-log.md)); until then `remove_participant` and
  `delete_channel` log it. That the three can be called without a check
  is [147](147-actor-control-unchecked.md).
