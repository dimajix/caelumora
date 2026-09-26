# 56 — Agents answer in English regardless of their users' language

**Status:** open
**Area:** User interaction

## The problem

Agents often answer in English, even when their users write in another
language. Their system prompts, tool descriptions and the prompt parts the
runtime adds (notes, plan instructions) are all English, which pulls the
model towards English - especially smaller models. A user has no way to say
which language they want the agents to use.

## Why it matters

- A user who prefers another language has to repeat that wish in the
  conversation, and the agent loses it again once the instruction scrolls
  out of focus or another agent answers.
- In a multi-agent channel the agents may answer in different languages, and
  agent-to-agent exchanges drift to English.

## What is not wanted

Answers are **not translated** - neither afterwards by the system, nor per
user into possibly different languages. A channel's conversation is one
conversation, in one language, which every participant reads as it is
(and which history, notes, digests and `read_history` refer to).

## Considerations

- **Whose preference, since the language cannot be per user.** A user can
  choose a preferred language, but a channel may have several users with
  different preferences. Options for the language a channel uses:
  - A property of the channel (or session), chosen explicitly and changeable,
    with the preferred language of the user who creates it as the default.
    Clear and stable; recommended as the starting point.
  - Derived from the users' preferences each turn (the owner's, the
    majority's). Implicit, and it changes when someone joins.
  - No setting at all: instruct the agents to answer in the language the
    last user message was written in. Cheap, but exactly what fails today
    when the model's instructions pull towards English, and undefined for
    agent-to-agent turns.
  - A value "auto" on the channel can offer the last option where it is
    wanted.
- **Where the preference lives.** `User` has no preferences today; a
  `preferred_language` (a BCP 47 tag such as `de`, `en-GB`) needs a place in
  the user model and the user backends ([25](25-file-user-backend.md) ff.),
  and a way for the user to set it in the web UI.
- **How it reaches the model.** A prompt part ([47](47-prompt-parts-with-provenance.md))
  saying which language to use. The language changes rarely, so a part at
  the start keeps the prompt cache; a short reminder before the turn's
  input would be stronger for models that lose it, at the cost of a
  per-turn part. Built in for every agent, or a component a personality
  includes? Since it is a property of the channel, not of the agent, built
  in (or a default component) seems right - but it should be recorded in the
  context like every other part.
- **Everything the agent writes follows the channel.** Notes and private
  notes ([17](17-information-sharing-within-session.md), [51](51-agent-private-notes.md))
  are read in other channels that may have another language; they are
  written in the channel's language and read as they are, like histories.
- **Not the same as UI localisation.** Translating the web UI and the TUI
  is a separate question, though it may use the same preference.
- Related: the English prompts in the code themselves -
  [57](57-prompt-catalogue.md).

## Done when

- A user can choose a preferred language, and a channel has a language
  (explicit or derived from that preference) that its agents answer in.
- Nothing is translated after the fact.

## History

- **2026-09-26** — Issue created at the maintainer's request: agents often
  answer in English; a user should be able to choose a preferred language
  that the agents' answers follow, without translating answers, least of all
  per user into different languages.
