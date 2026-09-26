# 55 — Notes are written only when the model thinks of calling a tool

**Status:** open
**Area:** Context & knowledge

## The problem

Notes ([17](17-information-sharing-within-session.md)) and private notes
([51](51-agent-private-notes.md)) come into being only through explicit tool
calls (`note`, `note_private`, `revoke_note`, `revoke_private_note`). Whether
something worth keeping is kept depends on the model deciding, in the middle
of answering, to call one of them. In practice models do that rarely and
unevenly: they focus on the answer, forget the tools, or note trivia while
missing what matters. What an agent learnt in a turn is then lost to its
other channels and to the rest of the session, although it was said.

## Why it matters

- The value of session memory ([19](19-memory-tiers.md)) depends on it being
  filled reliably, not on the model's attention in a given turn.
- Keeping notes is a different task from answering. Mixing both into one
  call burdens the answer (more tools, more instructions in every prompt)
  and gives neither the model's full attention.
- Explicit calls never tidy up: outdated or duplicate notes stay until the
  model happens to revoke them, so the notes grow and their prompt part with
  them ([16](16-context-compression.md)).

## Considerations

- **Where: the end of the turn.** The agent already has a phase for it:
  `ActorState::Reflecting`, whose `AgentActor::reflect` today only takes the
  turn over (`TurnContext::record_turn`) and emits `FullResponseGenerated`
  and `NotesChanged`, without a model call. A reflection call would run
  there, *before* `record_turn`, so that its notes are committed with the
  turn like those written by tools - and bound to the same branch point
  ([08](08-retry-and-edit-past-answers.md), principle 1 in
  [19](19-memory-tiers.md)).
- **What the call gets and returns.** Input: the turn's messages (or the
  conversation since the last reflection), the agent's current notes and
  private notes with their ids, and an instruction to keep, add, revoke or
  merge. Output, options:
  - The existing note tools offered as the only tools. Reuses the tool
    executor, permissions and validation; the result is an ordinary tool
    loop without a visible answer.
  - A structured output (a JSON list of operations). One call, no loop, but
    a second path that must validate what the tools already validate, and
    not every model supports it reliably.
- **As a component, not hard-wired.** In line with the existing extension
  points, reflection would be a facet of a component (e.g. a
  `TurnReflector` provider) configured per personality, so it can be
  switched on, parameterised and replaced. Parameters that suggest
  themselves: which chains it may write (public, private, both), its own
  prompt, a separate (smaller, cheaper) model, and how often it runs (every
  turn, every n turns, only after turns of some length).
- **Cost and latency.** One more model call per turn. The answer should not
  wait for it: `FullResponseGenerated` can go out first, reflection runs
  afterwards, and the agent returns to `Idle` only when it is done, so that
  the next turn sees its notes and cannot race it. Messages arriving
  meanwhile queue as they do during a turn.
- **Public notes are an effect.** Writing into the public chain publishes to
  every channel that may read this one (principle 4 in [19](19-memory-tiers.md)).
  An automatic step doing that is riskier than a deliberate tool call; a
  cautious default is private notes only, public ones opt-in.
- **Tools stay.** Explicit `note` calls remain for "remember this" in the
  conversation; reflection complements them, and may revoke or consolidate
  what the tools wrote.
- **Provenance and audit.** Notes written by reflection record that origin
  (besides the turn they belong to), and the reflection call's context is
  recorded like any other model call so the context panel
  ([41](41-show-message-context.md)) can show it. Nothing of it appears in
  the chat - consistent with hidden tool calls from
  [54](54-private-notes-visible-to-users.md).
- **Failure and cancellation.** The answer is already delivered, so a failed
  reflection must not turn the turn into `TurnFailed`: it is logged
  (`warn!`) and the turn ends without new notes. A cancelled turn skips
  reflection.
- **Plans.** Reflection runs once at the end of the whole turn, not after
  every plan step.

## Done when

- It is decided whether notes are also written by a reflection step at the
  end of a turn, and how it is configured.
- If so: an agent configured for it keeps notes without calling note tools
  itself, the notes are committed with the turn, and the answer does not
  wait for the reflection.

## History

- **2026-09-26** — Issue created at the maintainer's request: notes should
  not depend on explicit tool calls alone but be written by a recurring
  model call in the `Reflecting` phase at the end of every turn.
