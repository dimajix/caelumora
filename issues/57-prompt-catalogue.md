# 57 — Prompts are English constants spread through the code

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Context and knowledge
**Summary:** The prompts the runtime gives models are English constants spread through the code, in one variant for every language and model.

## The problem

The texts the runtime gives models are string constants in the code, all in English: tool
descriptions (`note`, `note_private`, `read_history`, ...), the headers of prompt parts ("[Notes in
this channel ...]", "[Your private notes ...]"), `PLANNING_SYSTEM_PROMPT` and the plan step
instructions, the texts a model receives for refused or failed tool calls. They exist in one
variant, for every language ([56](56-response-language.md)) and every model.

The question raised: should these prompts exist in several languages, and perhaps in variants for
particular models? If so, where would such a collection be managed, and how would the best variant
for a model be chosen? **First to be discussed is whether this is sensible at all.**

## Why it matters

- English instructions pull answers towards English, especially in small models - one cause of
  [56](56-response-language.md).
- Models differ: small local models need shorter, more explicit prompts; some merge system messages
  ([49](49-plan-step-system-messages.md)); tool calling behaves differently. One text for all is a
  compromise for each.
- Texts in the code can only be changed by a release; an operator cannot tune them.

## Considerations

- **Against it.** Larger models follow English instructions at least as well as translated ones, and
  "answer in German" in an English prompt usually suffices; tool descriptions in English are the
  norm and what models are trained on. Every text in n languages times m model variants multiplies
  maintenance, and without evaluations nobody knows whether a variant is better or just different.
  Automatic matching of prompts to models is a prompt engineering platform of its own.
- **For it.** Small models demonstrably mix up or ignore instructions given in another language than
  the conversation; model-specific quirks exist and are today worked around in code. An operator
  being able to adjust texts without a release is useful independently of languages and models.
- **Where a collection would live**, options:
  - Constants in the code, as today.
  - Built-in texts become parameters of the components that use them, with the current English text
    as the default - the pattern `static_system_prompt_provider` already follows with `prompt`. The
    personality, which also chooses the model, overrides them where needed. No new infrastructure;
    variants per language or model are then simply different personalities or component
    configurations.
  - A prompt catalogue: resource files or entities in the `EntityStore`, keyed by text id, language
    and model selector, with a fallback chain (`de-CH` -> `de` -> `en`; model -> family -> default),
    overridable per deployment.
- **How to match a variant to a model**, if at all: explicit selectors in the catalogue (model name
  patterns, families, sizes) with the most specific match winning, like locale fallback; or no
  matching - the personality binds its texts and its model together. An automatic "best variant"
  needs evaluation data that does not exist.
- **Order.** Measure first whether a language prompt part ([56](56-response-language.md)) already
  solves the language problem with English instructions. The step that is useful in any case is
  making built-in texts overridable component parameters; a catalogue with languages and model
  selectors only if repetition across personalities proves painful.

## Done when

- It is decided whether prompts in the code are to exist in several languages and/or model variants,
  and why - or recorded as rejected, with the reason.
- If so: where they are managed and how a variant is chosen.

## History

- **2026-09-26** — Issue created at the maintainer's request, together with
  [56](56-response-language.md): should the prompts spread through the code be kept in several
  languages, perhaps model-specific, where would such a collection be managed, and how is the best
  variant for a model chosen - to be discussed first whether this is sensible at all.
- **2026-10-05** — From [56](56-response-language.md): the part telling an agent the language of its
  channel (`channel_language_prompt_provider`) is English, naming the language in English and in
  itself. Whether a part written in the channel's language holds a model to it more surely is open;
  if a comparison on the models in use shows it does, that variant belongs in this catalogue, not in
  constants beside the English one.
