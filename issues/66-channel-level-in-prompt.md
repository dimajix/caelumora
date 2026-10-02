# 66 — Private content is not marked as such for the model

**Status:** done
**Area:** Context & knowledge

## The problem

Reasoning and tool calls are published by their confidentiality labels
([61](61-reasoning-leaves-the-agent.md)), but the answer text is not labelled: it cannot be redacted
without discarding the answer, so it is always published. Whatever the agent quotes in its answer
reaches the channel.

Most of what an agent knows in a channel may be said there anyway: other channels are read only if
their contents reach no further than this channel's (rule 2 of
[61](61-reasoning-leaves-the-agent.md)), and private notes from other channels are recalled by the
same rule (principle 12 in [19](19-memory-tiers.md)). What remains is content labelled for the agent
alone:

- its **private notes** ([51](51-agent-private-notes.md)), its inner view - of this channel too;
- the **results of tools whose content the channel does not admit**
  (`ToolConfidentiality::CALL_ONLY` or `HIDDEN`, or a content label above the channel's level): the
  channel does not see them, but the agent does, and quoting one makes the redaction worthless.

The model is not told that these are for it alone; a result looks like any other.

## Why it matters

- Redacting a hidden tool's result from the channel is pointless if the agent then quotes it in its
  answer.
- The model can only keep something to itself if it knows that it should.

## Considerations

- **Mark private content where it arises**, not by telling the model the channel's level: the level
  adds nothing, since everything else the agent knows in the channel is admitted there by
  construction.
  - The private notes' prompt part and the tool description of `note_private` say not to quote or
    reveal them in answers.
  - A tool result whose content label the turn may not publish (`Publication`) is wrapped for the
    model with a note that only the agent sees it and it must not be revealed - generically, by
    label, in `crate::tool` where the model's view of every outcome is decided
    ([15](15-disallowed-tool-errors-to-llm.md)), not per tool.
- It is guidance, not enforcement: the model can disregard it. Enforcing it for the answer text
  would need filtering or labelling the text itself (e.g. a check by a second model), a separate
  decision.
- Telling the agent about its channel (name, level, moderator, participants) as a prompt part may
  still be useful for other reasons; that would be a component of its own (e.g. `channel_info`, a
  `PromptProvider` at `Start`), not needed for this issue.
- The wording belongs with the other prompts in the code ([57](57-prompt-catalogue.md)).

## Done when

- The model is told, for private notes and for tool results the channel does not see, that they are
  for it alone and must not be revealed in answers.

## History

- **2026-09-27** — Split off from [61](61-reasoning-leaves-the-agent.md) at the maintainer's
  request: part of its decision ("the answer text gets no label for now; the channel's level is
  given to the agent as a prompt part instead"), not implemented with it.
- **2026-09-27** — Reframed at the maintainer's request before implementing. The first idea -
  telling the agent its channel's level as a prompt part - rested on a wrong example: a private note
  about a favourite colour, noted in one `session` channel and repeated in another, discloses
  nothing, because the second channel may read the first one's history anyway (rule 2, principle
  12). By construction, everything an agent knows in a channel may be said there except content
  labelled for the agent alone - private notes and results of tools the channel does not see. So
  those are marked for the model where they arise instead; the channel's level adds nothing. A
  `channel_info` component for other purposes stays possible.
- **2026-09-27** — Implemented. `tool::successful_result` wraps a successful result whose content
  label the turn may not publish (`TurnContext::publication`) as `{"result": ..., "note": "Only you
  see this result - the channel does not. Do not quote or reveal it in your answers."}`; results the
  channel sees are passed on unchanged. The private notes' prompt part and the description of
  `note_private` say not to quote or reveal them in answers. Verified in the browser: the context
  panel shows the `note_private` result reaching the model with that note, and the prompt part with
  its new header.
