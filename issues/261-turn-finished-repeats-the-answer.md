# 261 — `TurnFinished` carries the answer the channel already has

**Status:** rejected
**Area:** Sessions and channels
**Summary:** `TurnFinished` carries the answer the channel already has from the messages said, only for the moderator to decide whom to ask.

## The problem

Since [258](258-what-was-said-is-lost.md), every message an agent says reaches the channel as a
`CompleteMessageGenerated`, which the channel keeps and the agents hear. `TurnFinished` still
carries the turn's answer - its messages that are not internal - a second time, only for the
moderator, which decides from them whom to ask to respond: mentions, whether anything is heard, the
last message for a chain cut off.

## Why it matters

Every answer goes over the transport twice, and a client receives it twice. Nothing goes wrong -
the channel stores only what is said, the web UI ignores the payload - but the event says more than
it means, and a reader has to know which copy counts.

## Considerations

- **The moderator gathers a turn's messages as they are said**, by `turn_id`, and decides at
  `TurnFinished { turn_id }` from what it gathered: the event names only the turn. The moderator
  becomes stateful per turn under way, and has to forget a turn given up.
- **Or the channel hands the moderator the turn's messages** from its history at the turn's end:
  the history is the record already; the moderator stays stateless, and is given what the channel
  holds rather than what the sender claims.
- A message said outside of any turn is decided on as it is said, as now.

## Done when

- `TurnFinished` names the turn and nothing else, and whom to ask is decided as before.

## History

- **2026-10-08** — Opened when closing [258](258-what-was-said-is-lost.md), which renamed the event
  and kept its payload for the moderator.
- **2026-10-08** — Since [260](260-turn-messages-into-the-transcript.md), what a turn said is in
  the agent's transcript as it is said, `Transcript::of_turn` gives it, and `commit` reads the
  answer from there. The channel's history holds the same messages by their `turn_id` - which is
  what the second option, the channel handing the moderator the turn's messages at its end, would
  read.
- **2026-10-08** — Rejected by the maintainer for now: no pressing problem. The payload costs only
  the size of one event per turn, and the moderator decides correctly with it. Should it matter -
  a large answer, or the moderator wanting the turn's messages from elsewhere anyway - the second
  option above is what this issue found: the channel hands the moderator the messages of the turn
  from its history, by their `turn_id`.
