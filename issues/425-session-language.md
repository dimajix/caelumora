# 425 — A session has no language its channels could follow

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** Only a channel can be held in a language, so the language of a session is set again on each of its channels, a channel added later is held in none, and nothing that belongs to the session as a whole - the formatting of times by its time zone among it - has a language to go by.

## The problem

A channel may be held in a language (`Channel::language`, [56](56-response-language.md), see
[CHANNELS.md](../docs/CHANNELS.md#its-language)); a session cannot. The users of a session usually
speak one language in all its channels, but it has to be chosen on each of them: the web UI sets
the creator's preferred language after creating a session on each of its channels that the
template holds in none ([331](331-language-default-in-the-frontend.md)), and a channel created
later in the session is preset again in the new-channel dialog. Changing the language of a session
means changing every channel of it, one by one.

And what belongs to the session as a whole has no language: the times of
[424](424-time-in-the-prompt.md) are given in the session's time zone and formatted in a language,
which today can only be the channel's.

## What is wanted

A session may be held in a language, as a channel may. A channel that is held in no language of its
own is held in its session's.

## Considerations

- **Followed or copied.** A channel without a language of its own can follow its session's -
  whatever the session's language is at the moment, so that a change of the session's language
  reaches every such channel with the next turn of its agents - or copy it once when it is created.
  Copying is simpler, but then a change of the session's language reaches none of the channels
  that exist, though that is what it is changed for. **Decided: followed.**
- **What `None` on a channel means.** Today `Channel::language == None` means "held in no language
  in particular", and since 331 such a channel can be created by someone who prefers a language.
  Once a channel follows its session, `None` means "as the session": a channel explicitly held in
  no language within a session held in one is no longer possible. A third state - its own language,
  as the session, explicitly none - would keep it, at the cost of a value that is not an `Option`
  any more, in the model, the REST API and the web UI. Who wants no language leaves the session's
  empty, too. **Decided: no third state**; this takes back part of what 331 made possible.
- **One owner of the language that holds.** Which language a channel is held in - its own, else its
  session's - is a rule of the model and is answered in one place in `caelumora-model`, from the
  channel and its session. Everything that needs the language asks it: the
  `channel_language_prompt_provider` (which reads it through the agent's views at every turn), the
  formatting of times of 424, what REST shows of a channel. No caller decides it itself.
- **Changing it.** `Session::language`, `None` for none, changed or taken back by whoever may change
  the session, stored like a new name with its update event, as `ChannelLoop::update_language` does
  for a channel. The channels that follow it learn of it the way they learn of other changes of
  their session; an agent is told with its next turn, as of a change of its channel's language.
- **Templates.** A session template may name its session's language, which then holds for whoever
  creates the session, before the language they prefer - as `ChannelTemplate::language` does for a
  channel. A channel template's language stays its own.
- **The creator's preferred language.** The frontend presets the new session's language from it,
  not each channel's; channels are created in no language of their own and follow the session.
  Still the frontend's decision, not the core's (331).
- **REST and the web UI.** `language` on sessions and `PUT …/sessions/{sid}/language`, like the
  channel's. A channel shows both its own language and the one that holds, so that the web UI can
  show a language as followed from the session or as the channel's own, and the new-channel dialog
  offers "as the session" by default. Additions only; the API stays compatible.
- **Stored data.** A new optional field on the session; nothing has to be deleted.

## Done when

- A session can be held in a language - set when it is created, from its template or the request,
  and changed or taken back by whoever may change the session, over REST and in the web UI.
- A channel without a language of its own is held in its session's, and a change of the session's
  language reaches its agents with their next turn.
- Which language holds is answered in one place in the model, and the prompt part, REST and 424 ask
  it.
- CHANNELS.md and AGENT-TURN.md say so.

## History

- **2026-10-18** — Opened at the maintainer's request, from the discussion of
  [424](424-time-in-the-prompt.md): a session is to have a language as well, used where a channel
  has chosen none. Decided with it: a channel follows its session's language rather than copying it,
  and there is no third state for a channel explicitly held in none.
- **2026-10-18** — Done as decided. `Session::language` (`SessionUpdateEvent::LanguageChanged`,
  `SessionLoop::update_language`, checked as `UpdateSession`); the rule which language holds is
  `Channel::effective_language(&Session)` in the model, asked by `ChannelView::effective_language`,
  the snapshot and the `channel_language_prompt_provider`. A session is held in the language the
  request names, else in its template's (`SessionTemplate::language`, the precedence in
  `SessionTemplate::build`); `SessionManager::create_from_template` takes the request's. REST:
  `language` on sessions and in `POST /sessions`, `PUT /sessions/{id}/language`,
  `effective_language` on channels, `language` on templates. The web UI presets a new session's
  language from its template or the profile and no longer sets it on each channel; a new channel
  is held in none of its own unless one is chosen; a session's language is changed from its menu;
  the chat shows the language that holds, marked where it is the session's. The TUI presets the
  session's language as the web UI does. Stored data: a new optional field, nothing to delete -
  but channels the web UI or the TUI gave the preferred language before keep it as their own and
  do not follow their session's until it is taken back.
