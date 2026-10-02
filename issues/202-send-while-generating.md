# 202 — A user cannot send a message while an agent is answering

**Status:** done
**Area:** User interaction

## The problem

While an agent generates an answer, the web UI does not let the user send a message. The send
button in `ChatView` is disabled while `isGenerating` (`disabled={isSending || isGenerating || …}`),
and with the form's only submit button disabled, the browser does not submit on Enter either. The
user can only wait or stop the agent; a correction ("no, I meant the other file") or an addition
has to wait until the answer is complete.

`handleSendMessage` itself checks only `isSending`, not `isGenerating`: the block is the button's
alone.

## Why it matters

- Waiting for a long answer, or for a chain between agents, to end before one can write is the
  opposite of a conversation in a channel with several participants.
- The block suggests a guarantee that does not exist: the order in which the agents see messages
  is not the order in which the user sees them in the channel anyway, so holding the user back
  does not keep anything consistent.

## Considerations

### What the runtime does with such a message

Nothing has to change there, as far as can be seen:

- The REST API and the `UserActor` take a message at any time; the channel routes it as any other.
- An agent in a turn leaves the events it receives queued until the turn ends
  ([AGENT-TURN.md](../docs/AGENT-TURN.md), `AgentActor::handle_waiting`). It then perceives the
  message and, if the moderator asked it to respond, starts a turn for it. The turn running when
  the message arrived does not see it.
- After the user stopped a turn, the channel is quiet until the next message of a user; a new
  message ends that, as intended.

This has to be confirmed by a test of a message sent during a turn, under `round_robin_policy` in
particular, where the agent asked for the new message may be another than the one still answering.

### What the web UI has to change

- Drop `isGenerating` from the send button's condition; `isSending` (a request in flight) stays.
- The stop button stays next to it while an agent generates.
- `isGenerating`, `streamingMessage` and `streamingSender` describe one answer per channel. With
  a second message, several agents may answer at the same time; whether the view shows that
  correctly is to be checked, and may be an issue of its own.

### What the agent does not learn

An agent in the middle of a turn does not learn of the new message before the turn ends. Letting
it take a message in during its turn is a different, larger question and not part of this issue.

## Done when

- A user can send a message while an agent generates, with the button and with Enter.
- It is checked, ideally by a test, that the message reaches the agents and is answered after the
  running turn.
- WEB-UI.md says so.

## History

- **2026-10-03** — Issue created. The block is the send button's condition in `ChatView`; the
  runtime appears to handle a message during a turn already.
- **2026-10-03** — Done. The send button in `ChatView` is no longer disabled while an agent
  generates; `isSending` still holds it while a message is on its way. The runtime needed no
  change: `what_a_user_says_during_a_turn_is_answered_after_it` (`channel.rs`) sends a message
  while the agent waits for its model and shows that the running turn ends without it and a turn
  of its own answers it. WEB-UI.md (*Input*) says so. Not checked: how the view shows several
  agents answering at once - `isGenerating` and the streamed message are still one per channel.
