# 56 — Agents answer in English regardless of their users' language

**Status:** done
**Area:** User interaction

## The problem

Agents often answer in English, even when their users write in another language. Their system
prompts, tool descriptions and the prompt parts the runtime adds (notes, plan instructions) are all
English, which pulls the model towards English - especially smaller models. A user has no way to say
which language they want the agents to use.

## Why it matters

- A user who prefers another language has to repeat that wish in the conversation, and the agent
  loses it again once the instruction scrolls out of focus or another agent answers.
- In a multi-agent channel the agents may answer in different languages, and agent-to-agent
  exchanges drift to English.

## What is not wanted

Answers are **not translated** - neither afterwards by the system, nor per user into possibly
different languages. A channel's conversation is one conversation, in one language, which every
participant reads as it is (and which history, notes, digests and `read_history` refer to).

## Considerations

- **Whose preference, since the language cannot be per user.** A user can choose a preferred
  language, but a channel may have several users with different preferences. Options for the
  language a channel uses:
  - A property of the channel (or session), chosen explicitly and changeable, with the preferred
    language of the user who creates it as the default. Clear and stable; recommended as the
    starting point.
  - Derived from the users' preferences each turn (the owner's, the majority's). Implicit, and it
    changes when someone joins.
  - No setting at all: instruct the agents to answer in the language the last user message was
    written in. Cheap, but exactly what fails today when the model's instructions pull towards
    English, and undefined for agent-to-agent turns.
  - A value "auto" on the channel can offer the last option where it is wanted.
- **Where the preference lives.** `User` has no preferences today; a `preferred_language` (a BCP 47
  tag such as `de`, `en-GB`) needs a place in the user model and the user backends
  ([25](25-file-user-backend.md) ff.), and a way for the user to set it in the web UI.
- **How it reaches the model.** A prompt part ([47](47-prompt-parts-with-provenance.md)) saying
  which language to use. The language changes rarely, so a part at the start keeps the prompt cache;
  a short reminder before the turn's input would be stronger for models that lose it, at the cost of
  a per-turn part. Built in for every agent, or a component a personality includes? Since it is a
  property of the channel, not of the agent, built in (or a default component) seems right - but it
  should be recorded in the context like every other part.
- **Everything the agent writes follows the channel.** Notes and private notes
  ([17](17-information-sharing-within-session.md), [51](51-agent-private-notes.md)) are read in
  other channels that may have another language; they are written in the channel's language and read
  as they are, like histories.
- **Not the same as UI localisation.** Translating the web UI and the TUI is a separate question,
  though it may use the same preference.
- Related: the English prompts in the code themselves - [57](57-prompt-catalogue.md).

## Done when

- A user can choose a preferred language, and a channel has a language (explicit or derived from
  that preference) that its agents answer in.
- Nothing is translated after the fact.

## History

- **2026-09-26** — Issue created at the maintainer's request: agents often answer in English; a user
  should be able to choose a preferred language that the agents' answers follow, without translating
  answers, least of all per user into different languages.
- **2026-10-05** — Done. A language is a BCP 47 tag (`Language`, in the model), checked by its form
  and kept in its usual case. The preference lives in the `UserProfile` (`preferred_language`), not
  in the `User` record: the profile belongs to the user and is written by them, while the record is
  the system's and the user providers' - a preference grants nothing. A channel has a language
  (`Channel::language`, `None` for none in particular), set when it is created - on request or from
  a template - to the one its creator prefers unless the request names another, and changed or taken
  back by whoever may change the channel (`ChannelLoop::update_language`, `UpdateChannel`, stored
  like a new name). "auto" - the language of the last user message - was left out: it fails where
  the issue begins. The agents are told by the component `channel_language_prompt_provider`, a part
  of the personality like the roster rather than built in: a personality without it may ignore the
  language. It reads the language through the views at every turn and names it in English and in
  itself, with its tag (`German - Deutsch, BCP 47 tag "de-CH"`), from a small table
  (`Language::names`) - a model follows a name more surely than a tag; with `changes_only` for
  external agents. REST: `language` on channels, `PUT …/channels/{cid}/language`, and
  `preferred_language` in `/profile`, whose `PUT` now replaces both fields. Web UI: the preference in
  the profile dialog, the language preset in the new-channel dialog, shown and changed in the chat.
  Not done: writing the part in the language itself rather than in English, which would belong in
  the prompt catalogue ([57](57-prompt-catalogue.md)) and is worth it only if a comparison on the
  models in use shows it; and creating a channel explicitly held in no language by someone who
  prefers one - it is taken back after creating.
