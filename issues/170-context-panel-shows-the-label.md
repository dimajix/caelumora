# 170 — The context panel does not show why a message was classified as it was

**Status:** done
**Area:** User interaction
**Summary:** The context panel does not show how each part of a message's context was classified, so nobody can tell why the message was classified as it was.

## The problem

What an agent's turn writes is classified by the turn's label: the join of the classifications of
everything the turn had seen ([168](168-turn-label-as-high-water-mark.md)). Every prompt part
(`PromptRef::classification`) and every knowledge read (`KnowledgeRef::classification`) of a
message's context records the classification it had when it was read, and the transcript's messages
and tool results carry theirs. So why a message, a note or a memory entry was classified as it was
can be told.

The context panel does not tell it. It shows what the model was given, but not how each part was
classified, and not which parts raised the label to where it ended. `KnowledgeRefResponse` and the
prompt parts of `MessageContextResponse` do not carry the classification at all.

## Why it matters

- A user who finds a memory entry or a message classified higher than expected - often as high as
  the channel - has no way to see what caused it, and so no way to tell a correct classification
  from a missing one (a prompt part that does not say how it is classified counts as the channel).
- The owner of a personality who classifies its system prompt (`static_system_prompt_provider`'s
  `classification`) cannot see the effect.

## Considerations

- **Derived, not stored.** The label of a message is its classification; the parts that set it are
  those of its context whose classification is not below it. Nothing new needs to be stored.
- **What to show.** Per item of the context its classification, and the items at the label marked. A
  part recorded without a classification - a provider that does not say, or a reference recorded
  before 168 - shown as "counts as the channel".
- **REST.** `classification` added to `KnowledgeRefResponse` and to the prompt parts of
  `MessageContextResponse`: an addition, compatible.

## Done when

- The context panel shows the classification of every item of a message's context and marks those
  that set the message's label.

## History

- **2026-10-01** — Opened when [168](168-turn-label-as-high-water-mark.md) was done: the
  classification of every reference is recorded, the panel that would show it was left to this
  issue.
- **2026-10-01** — Done. `ContextItem::Prompt` carries the classification of its `PromptRef`, which
  the resolver used to drop. The REST server derives what set the label (`LabelCauses`, decided with
  the maintainer: in the server, not in the UI, which does not know the order of classifications): a
  part without a classification, counted as the channel; the parts with an audience, if the
  classified parts had none in common; otherwise the parts at the label's level. Leave-one-out
  ("without it the label would be lower") was rejected: two parts at the same level would mark
  neither. The label compared with is the message's classification
  (`AgentContextView::classification_of`), so a message reclassified since is compared with its new
  one. `SeenItemResponse` and `KnowledgeRefResponse` gained `classification` and `sets_label`; the
  context panel shows both and names the label in its head.
