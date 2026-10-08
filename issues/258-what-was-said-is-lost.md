# 258 — What a turn said is lost from the history when it is internal or the turn is given up

**Status:** open
**Area:** Sessions and channels

## The problem

A message an agent's turn publishes reaches the channel's history only if the turn ends
normally and the message is not `internal`. Two kinds of message are shown live and then lost:

- **The steps of a plan.** While an agent executes a plan (`create_plan`), every message of a step
  is `internal` (`Turn::in_plan_step`): the step's answer, its tool calls and their results. Such
  messages are published as `IntermediatePartialGenerated` and `IntermediateMessageGenerated`, kept
  in the agent's transcript, and left out of everything else: `Turn::commit` passes only the
  messages that are not internal on to `FullResponseGenerated` (`agent/turn/ending.rs`), the
  channel fills its history from that event alone (`ChannelLoop::append_history`), and the agent's
  own history leaves them out as well (`AgentChannelState::record_turn`).
- **The messages of a turn given up** - failed, stopped, asked for again (issue
  [235](235-retry-of-an-answer-under-way.md)), or the channel lowered below it (issue
  [141](141-turn-during-reclassification.md)). `Turn::give_up` keeps them on a branch of the
  agent's transcript that is no longer active (issue [145](145-ended-turns-leave-a-record.md)),
  tells the agent only which tools ran (`told_of_executed`), and the channel's history never gets
  them. The users saw them as they were said; after a reload they are gone, and `TurnFailed`, which
  said why, with them.

The web UI makes the first case worse: it has no branch for the two intermediate events
(`ChatView.tsx`, `handleMessage`), so from the moment a plan is approved until the answer after its
last step, nothing of the agent appears - no streamed text, no tool call, no result, and the
workspace's file tree is not refreshed. The TUI shows them as `[working]`.

So a tool call an agent makes without a plan is shown and kept in the history; the same call as a
step of a plan, or in a turn that later fails, is not. The flag `internal` was introduced with the
refactoring of the planning phase (`13756dd`) without a recorded reason; the comment at
`Message::internal` calls the history "the user-facing/audit trail", which is what these messages
are missing from.

## Why it matters

- What an agent did - which commands it ran, which files it changed - must be traceable. Its tool
  calls have effects whether or not its turn ends well, and whether or not it worked through a
  plan.
- When what is kept depends on how a turn ended, nobody can tell beforehand what will be there
  afterwards.
- The agent itself is told of a given-up turn's calls only by name and arguments in a runtime
  message, not by the calls and results it made; and to the user, a long plan looks as if the agent
  had stopped answering.

## Considerations

- **Keep only the steps of a plan, through `FullResponseGenerated`.** Decided first: the event
  carries every message of the turn, internal ones included, and the channel keeps storing on it
  alone - so that a turn given up still reaches nothing. Superseded by the principle below: it
  would keep the plan's steps, but not the calls of a turn that failed, and it makes what is kept
  depend on how the turn ended.
- **Keep given-up turns only for some reasons** - failed and stopped, not asked for again, not
  lowered. Set aside: every exception is a rule to learn, and the effects of tool calls do not
  depend on the reason.
- **Make the agent's transcript readable over REST** and show it per turn under the answer. Set
  aside: the transcript is the agent's own view, including what it heard, so it needs a permission
  question of its own; and the history would still differ between one kind of turn and another.
- **Nothing said is unsaid (the decision).** Every complete message a turn publishes stays - in the
  channel's history and in the agent's transcript and history, on the active path - however the
  turn ends. A message still being streamed when the turn ends was never complete and is not kept.
  A turn given up leaves a record of that, with the reason. In detail:
  - **The channel stores each message as it arrives:** every `CompleteMessageGenerated` and
    `IntermediateMessageGenerated` it admits goes into its history. `FullResponseGenerated` is no
    longer where a turn commits, only where it ends. What has been said survives a crash in the
    middle of a turn, too (issue [20](20-transactional-effects.md)).
  - **The agent keeps them as they are produced,** not at `commit`. What was not said stays bound
    to the turn's end: changes to notes and memory are applied only by `commit`, as now.
  - **A turn given up** leaves its messages on the active path, the calls that did not run answered
    as not executed (issue [148](148-cancelled-call-without-result.md)), and an entry saying it
    was given up and why: in the channel's history an entry of its own, like `HistoryEntry::Fitted`
    and `Reclassified` (issue [197](197-one-chain-one-model-of-change.md)), in the transcript
    `TranscriptEntry::GivenUp`. `give_up` no longer branches, and `told_of_executed` is no longer
    needed: the agent sees its calls and their results. `TurnFailed` stays as the live event.
  - **Asking an answer under way again** (issue 235) ends it and answers after it, no longer
    instead of it: the attempt stays, marked as given up, and the new turn sees it. Asking a
    finished answer again (issue [234](234-modify-revoke-and-retry-messages.md)) is unchanged - it
    is refused already when tools of the answer ran.
  - **A lowered channel** (issue 141) fits what was said before the lowering, as it fits every
    message it holds (`HistoryEntry::Fitted`); what the turn would say afterwards is refused before
    each step, as now. Since the channel stores each message as it arrives, it admits one only if
    it fits the channel as it is then.
  - **`internal`** keeps its meaning for a plan's steps: not heard by the other participants
    (`Message::is_heard`), and not the answer. It no longer decides what is kept.

  - **The other agents hear the messages of a turn given up** that are not internal: by the
    principle they were said, and they hear them as `as_heard` gives them. The steps of a plan and
    every tool call stay unheard, as now.
  - **A turn given up makes the moderator ask nobody to respond,** not even for a mention in what
    it said: it is no contribution to be answered. Its messages are heard with the next turn that
    is asked for anyway.

## Done when

- Every complete message of a turn - its answer, a plan's steps, their tool calls and results - is
  in the channel's history and in the agent's transcript and history on the active path, as soon
  as it is said, also when the turn is given up; changes to notes and memory are still applied only
  when the turn ends well.
- A turn given up leaves an entry with its reason in the channel's history and the transcript;
  `give_up` no longer branches.
- The other agents hear the messages of a turn given up that are not internal; a turn given up
  makes the moderator ask nobody to respond.
- Asking an answer under way again keeps the attempt; asking a finished one again, revoking,
  reclassifying, lowering a channel and paging the snapshot work with the messages kept.
- The web UI streams `IntermediatePartialGenerated`, shows `IntermediateMessageGenerated` live,
  refreshes the workspace after a step's tool results, shows the steps of a plan collapsible, a
  turn given up marked with its reason, and no longer removes an attempt that was asked for again.
- AGENT-TURN.md, CHANNELS.md, CONFIDENTIALITY.md and MEMORY.md describe it; the issues whose
  decisions this changes (141, 145, 235) say so under their history.
- Nothing stored needs deleting: turns from before keep what they had.

## History

- **2026-10-08** — Opened after the maintainer noticed that no messages arrive in the web UI
  while a plan is executed. Decided at first that `FullResponseGenerated` carries the whole turn,
  internal messages included, and the history keeps storing on it alone. Then the maintainer
  widened it to a principle - nothing said is unsaid, also when a turn is given up - so that what
  is kept no longer depends on how a turn ended; the channel stores each message as it arrives.
  Two questions were left open and then decided with the maintainer as recommended: the other
  agents hear what a turn given up said, if it is not internal; and a turn given up makes the
  moderator ask nobody to respond.
