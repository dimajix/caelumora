# 224 — What an agent hears of others is shaped in two places, and on joining not at all

**Status:** open
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

## History

- **2026-10-04** — Issue created at the maintainer's question, while resolving
  [222](222-queued-requests-answered-one-by-one.md), whether `worth_giving` should hold for the
  agent run here too. It should, and with the moderator's shaping it is one rule in two places; the
  gap is the way in on joining.
