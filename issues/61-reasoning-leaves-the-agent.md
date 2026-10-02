# 61 — An agent's reasoning is published with its messages

**Status:** done
**Area:** Context & knowledge

## The problem

A model's reasoning ("thinking") is stored in `Message::reasoning` and travels with the message
wherever the message goes: the streamed partials, `CompleteMessageGenerated`,
`FullResponseGenerated`, the channel history, REST and the event stream. The web UI does not display
it, but every user of the channel receives it.

Found while verifying [54](54-private-notes-visible-to-users.md): with the tool call `note_private`
hidden from the channel, the message that carried it still reached the history - holding only the
model's reasoning, which spelled out the note verbatim ("Call `note_private` with the text \"The
user's favorite color is teal\""). What `ToolVisibility::Hidden` withholds, the reasoning gives
away. In the web UI the message appears as an empty bubble.

## Why it matters

- Tool visibility ([54](54-private-notes-visible-to-users.md)) is undermined by any reasoning that
  mentions a call's arguments - which reasoning about a call usually does.
- More generally, reasoning is the agent's inner view: it may weigh private notes, recalled
  knowledge from other channels ([51](51-agent-private-notes.md), principle 12 in
  [19](19-memory-tiers.md)) or its instructions, none of which the answer itself would disclose.
- What a user receives but never sees in the UI is still disclosed; it is in the API and in the
  stored history.

## Considerations

- **Streaming makes a per-message rule too late.** Reasoning streams in partials *before* the model
  emits the tool call, so "withhold the reasoning of messages with non-public calls" cannot be
  applied to what was already streamed.
- Options:
  1. **Reasoning never leaves the agent.** Not published in partials or complete messages; kept in
     the agent's context, so its owner sees it in the context panel
     ([41](41-show-message-context.md)). Closes the leak completely, costs nothing in today's UI,
     and removes the empty bubble (a message with only reasoning has nothing left to publish).
     Showing reasoning in the chat later would need its own decision, e.g. to the owner only.
  2. **Withhold reasoning only from messages with non-public calls.** Clean history, but the
     streamed partials still leak it.
  3. **Buffer reasoning partials until the message is complete**, then decide as in option 2. Tight,
     but more complex, and it gives up streaming reasoning live without the UI gaining anything.
  4. **Make it configurable** (per personality or channel: publish reasoning or not), with "not" as
     the default - for setups where users are meant to follow the thinking, knowing the risk.
- Other agents: the moderator's `remove_tools` strips tool calls from messages routed to other
  agents but keeps their reasoning; whether other agents' reasoning reaches a model's input depends
  on the LLM adapter. Whatever is decided should hold for agents as readers too.
- Redaction already has one place, `Message::published` / `ActorEvent::published`, applied in
  `TurnContext::send_event`; any of the options fits there.

## Done when

- It is decided who may see an agent's reasoning, and published messages, streamed or complete,
  follow that decision.
- A hidden tool call's arguments cannot be read from the reasoning by anyone who may not see the
  call.

## History

- **2026-09-27** — Issue created at the maintainer's request, after the browser check of
  [54](54-private-notes-visible-to-users.md) showed the reasoning of a message with a hidden
  `note_private` call quoting the note in the channel history.
- **2026-09-27** — Decided after discussion with the maintainer. The first proposals were to decide
  at the source against the channel's current users (A) or per viewer at every egress (B). The
  maintainer's objection: a channel is a construct of a certain trustworthiness, and what of an
  actor's information is visible in it must depend on the channel alone, not on its participants -
  consistent while membership changes. Labels on messages stay as the actor's statement of what it
  wants to publish; the `SecurityManager` evaluates them from the **label**, the **owner / actor**
  of the message and the **channel**.
  - **Confidentiality levels as reaches**, ordered `public < session < channel < private`: as the
    label of a message part, how far it may travel (anywhere / channels of this session / only this
    channel / never leaves the actor - its owner sees it in the context panel only); as the level of
    a channel (one of the first three), how far the channel's contents may travel. Compartments
    later: [64](64-confidentiality-compartments.md).
  - **Rule 1 - publishing:** a part labelled `L` may appear in channel `C` if `L <= C.level`.
    Redaction stays in one place (`TurnContext::send_event`), now against the channel's level
    instead of the fixed `ToolVisibility` behaviour. Since the decision depends only on the channel,
    it holds for later participants as well; per-viewer redaction (B) is not needed.
  - **Rule 2 - reading across channels** (replaces the per-user check of `ChannelLoop::may_read`):
    channel A may read the notes and history of B if `B.level <= A.level` (the agent passes on what
    it reads, principle 11 in [19](19-memory-tiers.md)) and A is within B's reach (`public`: always;
    `session`: same session; `channel`: only A itself). Private notes from another channel are
    recalled by the same rule (principle 12).
  - **Labels:** `ToolVisibility` becomes the label of a call's arguments and result (`CallOnly`: the
    call itself `public`, arguments and result `private`; `Hidden`: the whole call `private`).
    Reasoning is labelled with the maximum of what the turn saw - private notes in the prompt give
    `private`, read channels their level, a non-public call `private` - and a default of the
    personality. The answer text gets no label for now; the channel's level is given to the agent as
    a prompt part instead.
  - **Streaming:** reasoning partials are streamed only if the label known before the call may be
    published and the agent has no non-public tools; otherwise the reasoning follows with the
    complete message under rule 1.
  - **Setting a channel's level:** only its owner, when creating it (dialog and REST); default
    `session`, the closest to today's behaviour. The owner may change it later, lowering included
    for now.
  - Joining a channel is not checked against its level yet, since participants come from the
    template only ([46](46-channel-participants.md)).
  - Two commits: (1) channel level and reading rule, (2) labels and the publishing rule.
- **2026-09-27** — Step 1 implemented: channel levels and the reading rule.
  `caelumora_model::confidentiality` holds `Confidentiality` (ordered `Public < Session < Channel <
  Private`) and `ChannelClassification` (session, channel, level) with `readable_from`, rule 2.
  `Channel` and `ChannelTemplate` have a `confidentiality` (default `session`; `private` is refused
  as `TemplateError::InvalidConfidentiality` / `SessionError::InvalidConfidentiality`);
  `ChannelSpec`, the REST request and response and the web UI's channel dialog carry it, and the
  members panel shows it. `ActionType::ReadChannel` now carries both channels' classifications
  instead of the source's attributes; `ChannelLoop::may_read` asks once, in the name of the reading
  channel's owner, instead of once per user, and the ownership policy decides by `readable_from`.
  Verified in the browser: from a `session` channel an agent lists only the `session` channels and
  cannot read a new `channel`-level one. Not yet: changing a channel's level after creation (the
  owner may, per the decision), and step 2 - labels and the publishing rule.
- **2026-09-27** — Step 2 implemented: labels and the publishing rule. `ToolVisibility` is replaced
  by `ToolConfidentiality { call, content }` (`PUBLIC`, `CALL_ONLY`, `HIDDEN`) on `Tool`, `ToolCall`
  and `ToolCallResult`; `Message` has `reasoning_confidentiality`, `Personality` a
  `reasoning_confidentiality` default (`public`). The agent labels every message
  (`AgentActor::label`): calls by their tool's declaration; the reasoning by the personality's
  default, `private` once the turn saw private notes (`TurnContext::saw_private_knowledge`), and the
  content labels of the message's calls - while streaming, of every tool it could still call.
  Knowledge from other channels adds nothing, since rule 2 only lets in what the channel admits
  anyway. Publishing is `ActionType::Publish { label, actor, channel }`, allowed by the ownership
  policy if `ChannelClassification::admits(label)`; the agent asks once per turn and level in its
  owner's name and keeps the answer as the turn's `Publication`, which `TurnContext::send_event`
  redacts by (`Message::published`). `private` is never published and never asked for. The web UI
  shows a call's labels instead of its visibility. Verified in the browser: asked to note something
  privately, the agent's message with the hidden call no longer reaches the history - neither the
  call nor its reasoning - and its answer arrives without reasoning, labelled `private`; no empty
  bubble. Still open from the decision: the channel's level as a prompt part for the agent (the
  answer text is not labelled), and changing a channel's level after creation.
- **2026-09-27** — The two open points are split off at the maintainer's request: the channel's
  level as a prompt part as [66](66-channel-level-in-prompt.md), changing a channel's level as
  [67](67-change-channel-level.md). Reasoning and tool calls are published by their labels, so this
  issue is done.
