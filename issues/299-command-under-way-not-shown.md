# 299 — A command an agent carries out is shown only once it is done

**Status:** done
**Area:** User interaction
**Summary:** A command an agent carries out, such as `/compact`, is shown only once it is done: nothing tells that the agent began, and it is shown idle meanwhile.

## The problem

A command of an agent (issue [269](269-agent-commands.md)) - `/compact @jonathan`, `/skill @bob
…` - is carried out in a turn of the agent's own, and the issuer learns of it only at its end,
from `CommandExecuted` or `CommandFailed`. Meanwhile nothing shows that the agent took it up:

- **No notice at the start.** The chat shows nothing between sending the command and its answer.
  Whether the agent began, or the command waits behind a turn under way, cannot be told.
- **The agent looks idle.** A command turn announces no `StateChanged` and moves the agent to no
  state of its own (`AgentCommands::carry_out`): it stays `Idle` or `Sleeping`, and the members
  panel (issue [243](243-member-state-in-member-list.md)) shows it so.

For `/skill` this hardly matters; for `/compact`, which has the agent's model summarise its
context ([16](16-context-compression.md)), it takes as long as an answer. Found by the maintainer
trying `/compact @jonathan`.

This was left open on purpose: 269 decided that a command turn is no answer - it announces neither
`ResponseStarted` nor `StateChanged` nor `TurnFinished` - and noted in its history that
"`/compact` (16), which calls a model, will want a state to show and a way to cancel it; that is
16's to add". 16 did not add it.

## Why it matters

- A user who cannot see that the agent works sends the command again, or assumes it was lost.
- The members panel is where one looks to see who is busy (243); an agent busy with a command that
  shows `Idle` is shown wrongly, and the moderator's and the channel's view of it are the same
  state.

## Considerations

- **For every command of an agent**, not for `/compact` alone: the start and the state belong to
  carrying a command out (`AgentCommands`, both kinds of agent), not to one component.
- **A state of its own: `ActorState::CarryingOutCommand`.** `in_turn()`, but not
  `interruptible()`: a command turn is not registered as the turn under way
  (`RunControl::begin_turn`), so a cancel would not reach it, and the web UI must not offer a stop
  icon that does nothing. Shown as working ("Working: carrying out a command"). Reusing `Planning`
  was considered and not chosen: it is interruptible, and the stop icon would be offered.
- **A sleeping agent.** It carries commands out without waking (269). Moving it to
  `CarryingOutCommand` meanwhile would lose a `/wakeup` (`RunControl::wakeup` wakes only from
  `Sleeping`) and make it a candidate of the moderator, which leaves out only the sleeping
  (`PolicyModerator::actor_candidates`), so that it would be asked to respond and the request
  dropped once it is asleep again. Proposed: only an awake agent moves `Idle` →
  `CarryingOutCommand` → `Idle`; a sleeping one stays `Sleeping` while it carries one out. Clean
  would be sleep as a flag beside the state - a change of `RunControl`, the moderator and the
  channel, for an issue of its own if it is wanted. *To decide.*
- **Two holders of the state.** The `AgentActor` keeps its state in `RunControl`, the
  `ExternalAgentActor` in a `ProcessingStateHolder` with `go_to`; both run the same short loop over
  `take_in` already (269's history). The move to and from `CarryingOutCommand` comes into both, or
  `carry_out` is given what moves and announces the state; a shared intake of both actors would
  fold it - worth naming as a refactoring.
- **The notice at the start: `ActorEvent::CommandStarted { command, args, turn_id, to }`**, sent
  by the agent once the command is admitted and before its provider runs - so
  `CommandExecutor::execute` for agents is split into `admitted` and the provider's call; otherwise
  a refused command would show as begun, then failed. Not recorded in the history: it is
  transient. The web UI shows the command's notice (`commandNotice`, keyed by the turn) at once as
  under way and replaces it with the answer; `CommandFailed` gets `turn_id: Option<TurnId>` so
  that a failure replaces it too. The TUI prints a line.
- **To whom the start is told.** Proposed: to the issuer alone. The answer of a private command
  reaches only the issuer, so a start told to everyone would stay "under way" for the others until
  they reload; they see the agent work from its state. The alternative - the start to everyone,
  and to the others a `CommandExecuted` without output at the end - closes the gap 269's history
  noted as well, and is larger. *To decide.*
- **Not here: cancelling a command under way** - e.g. a `/compact` while its summary is being
  written. The second half of 269's note; an issue of its own if it is wanted.

## Done when

- The issuer of a command of an agent sees in the chat that the agent began carrying it out, and
  the notice turns into the answer or the failure.
- While an agent carries a command out, the members panel shows it working, not idle, and offers
  no stop icon.
- Both kinds of agent behave alike.

## History

- **2026-10-10** — Issue created on the maintainer's report from trying `/compact @jonathan`: the
  channel learned of it only at its end, and Jonathan was shown idle meanwhile. Proposal above;
  how a sleeping agent is shown and to whom the start is told are still to decide.
- **2026-10-10** — Decided with the maintainer:
  - A sleeping agent stays `Sleeping` while it carries a command out; only an awake one moves to
    `CarryingOutCommand`.
  - The start is told to the issuer alone. The answer keeps its visibility on `CommandOutput`, as
    the command chooses it - for `/compact` everybody's: the issuer sees it begin, everybody sees
    it done. Declaring the visibility on `Command` instead, so that start and end follow one rule,
    was considered and not chosen.
  - The dot of an agent carrying out a command pulses orange, in colour and size as a working one
    does in green.
  - Beside it, in the web UI: the pulsing dots of an answer under way - "generating…" (blue) and
    "Thinking" (grey) - pulse as the members panel's dot of a working agent does, in green.
- **2026-10-10** — Implemented as decided; not yet seen in the browser.
  `ActorState::CarryingOutCommand` is neither `in_turn` nor `interruptible`: it is entered and left
  while the actor waits, so a `/nudge` is not refused as "already responding" and nothing resumes it
  as a turn (`prepare_resume` starts such a state idle). The `AgentActor` moves an awake agent there
  around `AgentCommands::carry_out` and back to `Idle`, the `ExternalAgentActor` with its `go_to`. A
  shared intake of both actors, as 269's history suggested, was looked at and is not worth it: what
  is the same - taking a batch in, ending it at a command - is `take_in` already; what is around it
  differs for a reason - the agent run here judges requests to answer again, sleeps and is woken,
  and lets its run loop announce its state, the external one tells its remote agent of changes
  (`tell`), rejects every request to answer again and announces at once. Shared, the five lines of
  the loop would take the actor's carrying out as an async closure over the state.
  `ActorEvent::CommandStarted { command, args, to, turn_id }`, routed to `to` alone;
  `CommandFailed` got `turn_id: Option<TurnId>`, set if the start was told. To tell the start
  between admission and carrying out without asking the policy twice, `CommandExecutor::admitted`
  for agents returns an `AdmittedCommand`, the only way to carry a command out; its `execute`
  replaced the executor's. Web UI: the command's notice appears under way with an orange pulsing
  dot and is replaced by the answer or the failure; the member's dot pulses orange
  (`animate-command`); the dots of "generating…", "Thinking" and the step of a plan under way pulse
  green (`animate-working`). The TUI prints the start. Tests: an agent run here tells the issuer
  it began, is shown `CarryingOutCommand` and `Idle` after; a failed command ends what began; a
  sleeping one tells the start and no state; an external agent alike; the moderator routes the
  start to the issuer alone. AGENT-TURN.md, TOOLS-AND-COMMANDS.md, CHANNELS.md, REST-SERVER.md,
  WEB-UI.md. Nothing stored changes.
