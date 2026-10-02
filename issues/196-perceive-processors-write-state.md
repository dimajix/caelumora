# 196 — What an actor perceives is recorded by configurable components

**Status:** in progress
**Area:** Code structure

## The problem

An agent run here keeps what it hears in its history and its transcript through two components of
its personality, `state_history_appender` and `transcript_appender` - `PerceiveInProcessor`s that
write into the agent's state through the `&mut Turn` every `MessageProcessor` is given:

- **Whether the history is complete depends on the configuration.** A personality without
  `state_history_appender` keeps nothing it heard, though the history is meant to hold everything
  that reached the actor and everything it said (issue 06).
- **One mechanism does two things:** filtering or changing what comes in - which is what a
  processor stage is for - and recording it, which must not be optional.
- **The external agent has no such stage** (issue [179](179-external-agents-via-a2a.md)). It records
  what it hears itself, and nothing can filter what reaches it. Giving it the `Turn` the processors
  ask for would mean giving it an `AgentChannelState` - against the decision of issue 179 - or
  making `Turn` generic over the state, for a tool it uses a fraction of.
- `apply_stage_processors` takes the input out (`mem::take`) before a processor runs, so a failing
  fail-open processor is skipped with an empty input: everything after it gets nothing.

## Decision (2026-10-02, with the maintainer)

- A `MessageProcessor` filters or changes messages and nothing else. It is given the actor's
  `SecurityContext` - who perceives, where - instead of the `Turn`.
- Both actors build their `PerceiveInProcessor`s from their personality and pass what they hear
  through them. **Filtering comes before the history:** what a processor drops has not reached the
  actor.
- Recording is the actor's own: what passes the stage always goes into the history - for an agent
  run here into its transcript too; an external agent's transcript is still written when it gives
  the remote agent something.
- `state_history_appender` and `transcript_appender` are removed at once. Stored personalities that
  name them fail to build and are to be fixed or deleted; old data may be deleted.

## Done when

- `MessageProcessor` takes no `Turn`; neither appender exists.
- An agent run here and an external agent both record what passes their `PerceiveInProcessor`s,
  whatever their personality configures.
- A failing fail-open processor is skipped with its input intact.

## History

- **2026-10-02** — Opened and decided in the discussion that followed issue
  [193](193-external-state-rewritten-whole.md): the external agent's history is complete as the
  agent's is, and the maintainer wanted the early filtering of `PerceiveInProcessor`s for both.
- **2026-10-02** — Step 1, for an agent run here: `MessageProcessor::process_messages` is given the
  actor's `SecurityContext` instead of the `Turn`; `AgentActor::perceive` passes what it hears
  through its `PerceiveInProcessor`s and appends what passes to its history and its transcript.
  `state_history_appender` and `transcript_appender` are gone, from the registry, the example seed
  and the documents. `apply_stage_processors` gives the next processor what a failing fail-open one
  was given. A test agent without the appenders now keeps what it hears; one test that read an
  empty transcript as "nothing of the turn was recorded" checks for the agent's messages instead.
