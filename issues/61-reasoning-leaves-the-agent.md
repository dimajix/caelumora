# 61 — An agent's reasoning is published with its messages

**Status:** open
**Area:** Context & knowledge

## The problem

A model's reasoning ("thinking") is stored in `Message::reasoning` and
travels with the message wherever the message goes: the streamed partials,
`CompleteMessageGenerated`, `FullResponseGenerated`, the channel history,
REST and the event stream. The web UI does not display it, but every user of
the channel receives it.

Found while verifying [54](54-private-notes-visible-to-users.md): with the
tool call `note_private` hidden from the channel, the message that carried
it still reached the history - holding only the model's reasoning, which
spelled out the note verbatim ("Call `note_private` with the text \"The
user's favorite color is teal\""). What `ToolVisibility::Hidden` withholds,
the reasoning gives away. In the web UI the message appears as an empty
bubble.

## Why it matters

- Tool visibility ([54](54-private-notes-visible-to-users.md)) is undermined
  by any reasoning that mentions a call's arguments - which reasoning about
  a call usually does.
- More generally, reasoning is the agent's inner view: it may weigh private
  notes, recalled knowledge from other channels
  ([51](51-agent-private-notes.md), principle 12 in [19](19-memory-tiers.md))
  or its instructions, none of which the answer itself would disclose.
- What a user receives but never sees in the UI is still disclosed; it is
  in the API and in the stored history.

## Considerations

- **Streaming makes a per-message rule too late.** Reasoning streams in
  partials *before* the model emits the tool call, so "withhold the
  reasoning of messages with non-public calls" cannot be applied to what
  was already streamed.
- Options:
  1. **Reasoning never leaves the agent.** Not published in partials or
     complete messages; kept in the agent's context, so its owner sees it
     in the context panel ([41](41-show-message-context.md)). Closes the
     leak completely, costs nothing in today's UI, and removes the empty
     bubble (a message with only reasoning has nothing left to publish).
     Showing reasoning in the chat later would need its own decision,
     e.g. to the owner only.
  2. **Withhold reasoning only from messages with non-public calls.** Clean
     history, but the streamed partials still leak it.
  3. **Buffer reasoning partials until the message is complete**, then
     decide as in option 2. Tight, but more complex, and it gives up
     streaming reasoning live without the UI gaining anything.
  4. **Make it configurable** (per personality or channel: publish
     reasoning or not), with "not" as the default - for setups where users
     are meant to follow the thinking, knowing the risk.
- Other agents: the moderator's `remove_tools` strips tool calls from
  messages routed to other agents but keeps their reasoning; whether other
  agents' reasoning reaches a model's input depends on the LLM adapter.
  Whatever is decided should hold for agents as readers too.
- Redaction already has one place, `Message::published` /
  `ActorEvent::published`, applied in `TurnContext::send_event`; any of the
  options fits there.

## Done when

- It is decided who may see an agent's reasoning, and published messages,
  streamed or complete, follow that decision.
- A hidden tool call's arguments cannot be read from the reasoning by
  anyone who may not see the call.

## History

- **2026-09-27** — Issue created at the maintainer's request, after the
  browser check of [54](54-private-notes-visible-to-users.md) showed the
  reasoning of a message with a hidden `note_private` call quoting the note
  in the channel history.
