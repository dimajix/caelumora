# 464 — Compaction is wired into the agent

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Context and knowledge
**Summary:** The agent's actor knows compaction by name - its policy and strategy as fields, when it is due, how it is carried out and what is said when there was nothing to compact -, though what an agent does of its own accord before a model call is meant to be generic, so that every further such activity would be wired into the actor the same way.

## The problem

Issue [284](284-automatic-context-compaction.md) made compaction automatic and announced it
generically, as an *activity*: `ActorState::Busy` is "doing something of its own accord within its
turn, besides answering", told by `ActorEvent::ActivityStarted` and `ActivityFinished`. A
compaction was to be the first such activity. The actor, though, knows only this one, and knows it
by name:

- `AgentActor` holds the `CompactionPolicy` and the `CompactionStrategy` as fields of their own, and
  lists both in `COMPONENT_FACETS` (`caelumora-agents/src/actor/agent.rs`).
- `handle_planning` asks `compaction_due` - `Turn::may_compact` and the policy's `is_due` - before
  every model call, and goes to `Busy` if it is due.
- `handle_busy` is generic in what it does around the activity - `ActivityStarted`, a cancel ending
  it as it ends a call, the state stored before `ActivityFinished` (issue
  [405](405-gauge-stale-after-compact-command.md)) - but calls the strategy itself, names the
  activity "compacting its context", and words what came of it, down to "found nothing to compact".
- `Turn::may_compact`, `Turn::try_compaction` and `TurnState::compaction_tried_at` keep a
  compaction to at most one try before each model call - bookkeeping every activity needs, kept
  for compaction alone.

The `compaction` component, meanwhile, only provides `/compact`; when the agent compacts on its own
is a second component (`threshold_compaction_policy`), a single facet with a default, which every
agent has whether its personality lists it or not.

## Why it matters

- The actor is to run a turn, not to know what its components do. Every further activity - an old
  tool result dropped without a model call, a note written before the context fills - would add a
  field, a check in `handle_planning` and a branch in `handle_busy`.
- What belongs to compaction - when it is due, how it is done, how it is told - is spread over the
  actor, `Turn` and two components, instead of living in the `compaction` module.

## Considerations

- **A facet for activities, as there is one for commands.** The closest pattern is
  `AgentCommandProvider`: a facet of which an agent has any number, given the `Turn` to work on,
  answering in words for the channel. An activity is asked before every model call whether it is
  due; the actor carries out the first one due that was not tried since the last call, in `Busy`,
  and goes back to `Planning`, where the next one due is asked for. The message stage processors
  (`PerceiveInProcessor`, `EmitOutProcessor`) are the other generic hook of an agent, but they
  transform messages in a pipeline, which an activity does not.
- **The component reports what came of it, not a state.** An activity runs before a model call, so
  the actor always goes on to `Planning` after it; a component that set the `ActorState` would move
  the actor's state machine into plugins. It returns its outcome in words - what `ActivityFinished`
  carries - or an error, which ends the turn as a failed compaction does today. What it changes in
  the context it changes through the `Turn`, as `/compact` does.
- **One try per activity and call.** `compaction_tried_at` becomes the activities tried since the
  last model call, by name, cleared as a call is counted. Several activities then run one after
  the other, each at most once before a call.
- **The default.** Only single facets have a default kind (`ComponentRegistry::single`,
  `ComponentContainer::with_defaults`); a facet of which an agent has any number has none. Either an
  agent compacts on its own only if its personality lists `compaction` - as `/compact` is given
  today -, or the registry gets defaults for such facets, a mechanism of its own with no second use
  so far.
- **One component or two.** The `compaction` component can provide the activity beside the
  command, with `threshold` as its parameter, or a kind of its own can. A kind of its own is a
  switch nobody needs today.
- **A crate of its own** for compaction becomes possible once the actor names no compaction type;
  where `KnowledgeRestorer`, which `caelumora-skills` implements, would live then is open. Not part
  of this issue.

## Decisions taken

Agreed with the maintainer on 2026-10-24:

- A facet `ActivityProvider` in `caelumora-agents`, of which an agent has any number:

  ```rust
  /// What a component does of the agent's own accord before a model call (issue 284) -
  /// compacting its context. An agent has any number.
  #[async_trait]
  pub trait ActivityProvider: Send + Sync + Debug {
      /// What it does, as its channel is told it begins - "compacting its context".
      fn activity(&self) -> &str;
      /// Whether it is due before the next model call of `situation`.
      fn is_due(&self, situation: &Situation<'_>) -> bool;
      /// Carries it out on `turn`: what came of it, in words for the channel.
      async fn carry_out(&self, turn: &mut Turn<'_>) -> Result<String, ActivityError>;
  }
  ```

  The error type follows `ProcessingError` (issue
  [336](336-errors-of-a-plugin-shown-as-a-failure.md)).
- The actor keeps the activities as a list and carries them out generically in `handle_busy`; it
  names no compaction type. The `ActorState` stays the actor's: after an activity it goes on to
  `Planning`.
- The `compaction` component provides `/compact` and the activity, with `threshold` as its
  parameter. `CompactionPolicy` and the `threshold_compaction_policy` kind go; `CompactionStrategy`
  stays a single facet with its default, asked by both.
- **An agent compacts on its own only if its personality lists `compaction`** (option (a) above).
  Personalities and examples that are to compact are given it; an agent without it no longer
  compacts automatically.
- `TurnState::compaction_tried_at` is replaced by the activities tried since the last call; a turn
  stored under way with the old field is not converted.
- **In two steps.** Step 1 is the above, in `caelumora-agents`. Step 2 moves the `compaction`
  module into a plugin crate of its own, `caelumora-compaction` (agreed on 2026-10-24, after step 1
  had left nothing in the agents naming it but their plugin): the component, the strategies,
  `CompactionStrategy`, `Compaction`, `CompactionError` and `KnowledgeRestorer`, which
  `caelumora-skills` then implements from there; its texts in a namespace of their own. The
  mechanism - `Turn::compact`, `AgentChannelState::compact` - stays where it is.

## Done when

- `AgentActor` names neither `CompactionPolicy` nor `CompactionStrategy`, and `handle_busy` carries
  out any `ActivityProvider`.
- The `compaction` component provides `/compact` and the automatic compaction; the policy facet and
  its kind are gone.
- The shipped personalities and examples that compact list `compaction`.
- `docs/AGENT-TURN.md` (`Busy`, *Compaction*), the facets in `docs/COMPONENTS.md` and
  `docs/PLUGINS.md`, and the module docs of `crate::compaction` describe activities and where
  compaction lives.
- Tests cover an activity due and carried out, one tried only once before a call, several due
  one after the other, and an agent without `compaction` that does not compact.

## History

- **2026-10-24** — Opened, from a discussion with the maintainer; the design above agreed.
- **2026-10-24** — Implemented on `issue-464-activities`: `ActivityProvider` and `ActivityError` in
  `crate::activity`, the latter judged by `error::user_message`, so that a failed compaction is
  now told to the users; the actor carries out any activity in `Busy`; `TurnState` keeps
  `activities_tried` by component id. The `compaction` component, now in
  `compaction/component.rs`, provides the command and the activity; `CompactionPolicy` and
  `threshold_compaction_policy` are gone, and the agent no longer asks for the
  `CompactionStrategy` itself - `compaction` needs it. The shipped personalities list
  `compaction` through the bundle `agent-basics` already.
- **2026-10-24** — Step 1 reviewed: a try is recorded once the activity is done, so that one cut
  short by a stop is carried out again as the turn resumes in `Busy`. Step 2, the crate of its
  own, agreed.
