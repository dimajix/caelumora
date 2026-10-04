# 224 — What an agent hears of others is shaped in two places, and on joining not at all

**Status:** in progress
**Area:** Context and knowledge

## The problem

What an agent takes into its transcript of another participant's messages is decided in two
places, by two rules:

- **Live**, the moderator shapes every message before it is delivered (`as_user_messages`,
  `policy_moderator.rs`): messages without content and tool messages are left out, every other one
  becomes a `user` message without tool calls or results. The sender never gets its own message
  back.
- **The external agent** filters again, on both of its ways in (`worth_giving`,
  `actor_impl/external_agent.rs`): said by another, not internal, `user` or `assistant`, with text.
  Its actor passes it to `perceive`, `catch_up` and `take_in` as their `takes` parameter; the agent
  run here passes `|_| true`.

On the live way the second rule changes next to nothing - the moderator has shaped the messages
already. But an actor that joins takes in what it missed straight from the channel's history
(`catch_up`, issues 190, 196), which holds messages as they were said: another agent's answers as
`assistant` messages with their tool calls, and its tool results. The external agent filters them
with `worth_giving`; the agent run here takes them as they are - its model is then given another
agent's message as one of its own, unmarked (`SenderFormat::given` marks only `user` messages), and
possibly tool calls whose results it never saw. This follows from the code; no test shows it yet.

## Why it matters

- An agent that joins a channel in which other agents have spoken may take their words for its own.
- One rule - what an agent hears of a message of someone else - lives in two places in different
  forms, and a third way in (222's `take_in`) had to be given the same parameter again.

## Considerations

- **One rule in `perceive`**, for both kinds of agent and every way in: what enters the transcript
  of a message of another is what the moderator's `as_user_messages` and `worth_giving` make of it
  together - said by another, with text, not internal, no tool message - as a `user` message without
  tool calls and results. The history keeps what was offered, as it is. `takes` goes from
  `perceive`, `catch_up` and `take_in`.
- **The moderator's shaping** reaches users too, and what frontends receive must not change; whether
  it stays, or the channel delivers messages as they were said and each kind of actor shapes them
  for itself, is to be decided with it.
- What an external agent gives on stays text alone; its filter may stay stricter if a reason
  remains once the shared rule exists - to be said if so.

## Done when

- What an agent of either kind takes into its transcript of others' messages is decided in one
  place, the same live and on joining.
- A test with an agent joining a channel in which another agent answered with a tool call shows the
  other's answer in its transcript as a marked `user` message without the call.

## Implementation plan

One rule, owned by the data it is about, and applied where an agent takes something in - not where
a message is delivered.

### What the code does today

- A turn publishes all its messages that are not internal (`agent_turn/ending.rs`), its tool calls
  and tool results included, and the channel keeps them in its history as they were said
  (`ChannelLoop::append_history`).
- Live, the moderator shapes what it delivers to everyone but the sender (`as_user_messages`):
  every message with content that is not a tool message becomes a `user` message without tool
  calls or results. Empty content (`Some("")`, common next to a tool call) is kept; a `system`
  message becomes a `user` message.
- On joining, `catch_up` takes what the agent missed from the channel's history unshaped. The
  external agent filters with `worth_giving`; the agent run here takes everything (`|_| true`). A
  probe confirmed it: another agent's `assistant` message with its tool call, and its `tool`
  message, end up in the transcript as they are. Resolving the context changes nothing of it, and
  `SenderFormat::given` marks only `user` messages.
- The agent's history therefore holds what it heard live shaped, and what it caught up with as
  said.
- Users do not need the shaping: the web UI does not read the messages of a
  `FullResponseGenerated` (the line is commented out, `ChatView.tsx`), no other client reads the
  event, and what users read of the channel's history through REST is as said anyway. What travels
  on the bus is published already (`Message::published`), so delivering it as said shows nobody
  anything new.

### Steps

1. **`Message::as_heard(&self) -> Option<Message>`** (`caelumora-model/src/message.rs`): what a
   participant hears of this message when someone else said it - `None` unless it is a `user` or
   `assistant` message, not internal, with text that is not empty; otherwise a `user` message with
   the same text and everything else of it, without tool calls and tool results. Whether the
   listener said it itself is not the message's to know, and stays with the caller. Tests in the
   model: an `assistant` message with a tool call is heard as its text; a tool message, an empty
   one, a system message and an internal one are not heard.
2. **`actor_impl::perceive` applies it** for both kinds of agent and every way in - live, `take_in`
   and `catch_up`: what is offered to the perceive processors is what others said, as heard
   (`m.sender_id != own`, `as_heard`). The parameter `takes` of `perceive`, `catch_up` and
   `take_in` is replaced by the agent's own `ActorId`. `worth_giving` goes; the external agent's
   tests that used it use `perceive` with its id. The history keeps everything as it was offered.
   A channel test: an agent run here that joins a channel in which another agent answered with a
   tool call has the other's text in its transcript as a `user` message without the call, and
   nothing of the tool message; its history has both as said.
3. **The moderator delivers what was said as it was said** (`PolicyModerator::forward_routes`):
   `as_user_messages` goes. Whether anyone is told and asked at all - nothing is, if no message has
   anything to hear, e.g. a response of tool calls only - it decides with `Message::as_heard`, as
   today with its own rule. Its test of the shaping moves to step 1; a test that a response of tool
   calls only is routed to nobody stays. A behaviour change of its own, in a commit of its own.
4. **Documents.** AGENT-TURN.md, *Perceive*: what an agent hears of others, by which rule, the same
   for both kinds and every way in. CHANNELS.md: the external agent's "what can be given on" names
   the shared rule; the moderator delivers as said.
5. **Record.** Status `done` here and in the index, a History entry.

Steps 1 and 2 are one commit (the rule in one place, the bug fixed), step 3 another.

### Not done

- Transcripts stored before, which may hold other agents' messages as they were said, stay as they
  are: they are branches of what the agent was given, and rewriting them would change what its
  model saw.
- Whether a response of tool calls only should still reach users as `FullResponseGenerated` - the
  web UI ends a running answer by it - is not changed here: it is routed to nobody, as today.

## History

- **2026-10-04** — Issue created at the maintainer's question, while resolving
  [222](222-queued-requests-answered-one-by-one.md), whether `worth_giving` should hold for the
  agent run here too. It should, and with the moderator's shaping it is one rule in two places; the
  gap is the way in on joining.
- **2026-10-04** — Confirmed with a probe: an agent run here that joins takes another agent's
  answer with its tool call, and its tool message, into its transcript as they are. Implementation
  plan added, agreed with the maintainer: the rule as `Message::as_heard`, applied in `perceive`
  for both kinds of agent; the moderator delivers as said.
- **2026-10-04** — Steps 1 and 2 done (`e64e1ca`): `Message::as_heard`, applied in `perceive` for
  both kinds of agent and every way in; `takes` and `worth_giving` are gone.
- **2026-10-04** — Step 3: the moderator forwards a response as it was said; `as_user_messages` is
  gone. Found on the way, and part of the step: a response with nothing to hear reached nobody, the
  users included, since 220 - after a cancel, the web UI kept the answer as under way
  ([225](225-cancel-leaves-answer-generating.md)). The users now always get it, the agents only if
  there is something to hear, and only then is anyone asked. Whether there is, the maintainer
  proposed to ask without building what is heard: `Message::is_heard`, which `as_heard` uses too. A
  test's agent that answered with a `system` message now answers as an `assistant`: nothing is heard
  of a `system` message, and nothing in the code says anything into a channel as one.
