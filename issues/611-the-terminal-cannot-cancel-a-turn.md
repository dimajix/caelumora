# 611 — The terminal cannot cancel an agent's turn

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The terminal has no way to end the turn an agent is in: the kernel can cancel a channel, a thread or one participant, and the web UI does so over REST, but no command or key of `caelumora-tui` reaches it, so a turn that hangs or goes astray can only be ended by taking the agent out of the channel.

## The problem

The kernel can end turns under way: `ChannelLoop::cancel` ends those of the whole channel or of
one thread, `ChannelLoop::cancel_actor` the turn of one participant, both under `CancelChannel`
(`caelumora-kernel/src/channel/control.rs`). REST maps them to
`…/channels/{cid}/cancel`, `…/threads/{tid}/cancel` and `…/participants/{aid}/cancel`, and the
web UI stops a turn with them.

`caelumora-tui` offers none of it. Among the terminal's commands and the channel's
(`docs/COMMANDS.md`) none cancels, and no key of the full screen does. What comes closest does
something else:

- `/agent retry` ends the step at once, as a cancel does, but then asks for the answer again - an
  agent stuck on a call makes the same call again.
- `/exit` stops the session; an agent stores its turn at the step it stands at and resumes it as
  the session runs again.
- `/channel leave <agent>` ends the turn, since an agent that leaves gives its turn up, but takes
  the agent out of the channel; it has to be added again.

Seen in the terminal: an agent waiting for the approval of a call in a channel without an approver
(issue [610](610-a-channel-made-by-command-has-no-approver.md)) could only be freed by
`/channel leave`. A cancelled turn withdraws its approval requests (`docs/APPROVALS.md`), which
is what the user wanted.

## Considerations

1. **A channel command `/agent cancel [agent]`, beside `/agent wakeup`.** Built as `/agent wakeup`
   is (`caelumora-kernel/src/command/agent/wakeup.rs`): without an agent it ends the turns of the
   thread it is issued in, or of the whole channel issued in the line (`ChannelLoop::cancel`);
   with one, that agent's turn (`ChannelLoop::cancel_actor`). The channel checks `CancelChannel`
   as for REST, and everybody in the channel is told, since it changes who answers. Every frontend
   that runs commands has it, not the terminal alone, and the terminal needs nothing of its own.
2. **A terminal command, as `/agent retry` is.** `/agent retry` is the terminal's because it
   needs what the terminal follows - the answer under way, the turn given up. Cancelling needs
   nothing of that: the channel and the thread a command is issued in are enough. A terminal
   command would be a second way to what option 1 gives every frontend.
3. **A key of the full screen** (`Esc`, `Ctrl+C`). Quick to reach, but the line-by-line screen for
   a screen reader needs a command anyway, and `Esc` already puts a question or an approval aside.
   It can come on top of option 1 if a command turns out too slow to type.

Recommended: option 1.

## Done when

- A user in the terminal ends the turn of an agent in the channel or thread they talk in, without
  taking the agent out of it; an approval it waited for is withdrawn.
- The command is tested as `/agent wakeup` is, its permission included.
- `docs/COMMANDS.md` lists it.

## History

- **2026-11-26** — Opened after an agent waiting for an approval nobody could give in the terminal
  could only be stopped by taking it out of the channel.
- **2026-11-27** — Done by option 1, as the maintainer decided, with the short form `/cancel` as
  `/agent wakeup` has `/wakeup`. `/agent cancel` is a command of the group `agent`, built as
  `/agent wakeup` is: without an agent it ends the turns of the thread it is issued in, or of the
  whole channel issued in the line (`ChannelLoop::cancel`), and keeps them quiet; with one, that
  agent's turn (`ChannelLoop::cancel_actor`), which leaves it in the channel. Both ask for
  `CancelChannel`, and the answer goes to the whole channel - "Ended any turn of …", since neither
  method tells whether a turn was under way. A cancelled turn withdraws its approval requests as
  before. Tests hold the turn of the agent named ended and the agent still in the channel, the
  thread or the channel cancelled and quiet as it is issued, and the refusal by `CancelChannel`;
  the scripted channel of the tests (`channel_of_alice_and`) runs the built-in commands for it.
  `docs/COMMANDS.md`, `docs/CHANNELS.md`, `docs/COMPONENTS.md` and `docs/PERMISSION-CHECKS.md`
  name it. No key of the full screen was added.
- **2026-11-27** — As the maintainer decided after the review, the answer tells what was ended:
  `Actor::cancel` returns whether the actor had a turn to end - as `retry_turn` and
  `interrupt_tool_call` return whether they hit -, `ChannelLoop::cancel` the names of those that
  had, and `ChannelLoop::cancel_actor` whether the actor had, as `wakeup` and `wakeup_actor` do.
  `/agent cancel` answers "Ended the turn of …" or "… had no turn." Beside it, the tests of
  `/participant nudge` run it through `UserActor::execute_command` instead of a helper of their
  own, and the security suite holds the permissions `/agent cancel` and `/agent wakeup` ask, for
  the channel and for one agent. `RunControl::cancel` decides under the turn's lock, as `retry`
  does, so that a turn resumed meanwhile is not answered as none; its tests and those of an
  agent's cancel hold what it returns, for a turn running and one set aside.
