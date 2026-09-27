# 66 — The agent does not know how confidential the channel it writes in is

**Status:** open
**Area:** Context & knowledge

## The problem

Reasoning and tool calls are published by their confidentiality labels
([61](61-reasoning-leaves-the-agent.md)), but the answer text is not
labelled: it cannot be redacted without discarding the answer, so it is
always published. An agent that knows its private notes
([51](51-agent-private-notes.md)) can write them into its answer in plain
text - e.g. asked in a `session` channel "what do you know about Kaya's
preferences?", it answers with the favourite colour it noted privately
elsewhere. No label catches that. The agent is not even told what level the
channel it is writing in has.

## Why it matters

- Private knowledge leaks through the one part of a message the
  confidentiality rules cannot touch.
- The model has no basis for judging what it may say where: the tool
  description says private notes are private, but not how that relates to
  the channel at hand.

## Considerations

- **A prompt part** ([47](47-prompt-parts-with-provenance.md)) telling the
  agent the level of its channel and what that means, e.g. "This channel is
  at level *session*: what you write here can be read by the other channels
  of this session. Do not repeat your private notes or anything from more
  confidential channels here." The level changes rarely, so a part at the
  start keeps the prompt cache.
- It is guidance, not enforcement: the model can disregard it. It reduces
  accidental disclosure; enforcement for the answer text would need
  filtering or labelling the text itself (e.g. a check by a second model),
  a separate decision.
- Built in for every agent or a component a personality includes? The
  level is a property of the channel and the rule applies to every agent
  that has private knowledge, so a default component (like the notes
  components) or a built-in part seems right; either way it is recorded in
  the context like every other part.
- The wording belongs with the other prompts in the code
  ([57](57-prompt-catalogue.md)).

## Done when

- An agent is told the confidentiality level of the channel it writes in
  and what it may therefore not repeat there.

## History

- **2026-09-27** — Split off from [61](61-reasoning-leaves-the-agent.md) at
  the maintainer's request: part of its decision ("the answer text gets no
  label for now; the channel's level is given to the agent as a prompt part
  instead"), not implemented with it.
