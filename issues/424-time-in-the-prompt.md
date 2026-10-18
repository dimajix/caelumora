# 424 — An agent knows neither when something was said nor what time it is

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** Context and knowledge
**Summary:** A model is given the messages of its channel without when they were said and is never told the current date and time, so it cannot tell a message of a minute ago from one of last week, answers questions about today from its training cutoff, and a session has no time zone that times could be given in.

## The problem

Every `Message` carries its `timestamp` (UTC), but the model never sees it: the `ContextResolver`
marks a message of another participant with its sender (`ModelProfile::sender_format`, `@alice:
text`, see [AGENT-TURN.md](../docs/AGENT-TURN.md#who-said-what)) and nothing else. Only the tool
`read_history` returns an `at`, in RFC 3339 and UTC. Nor is the model told what time it is now -
no prompt part names the date.

So an agent

- cannot tell that a conversation was taken up again after days, that a question was asked an hour
  ago and is still unanswered, or that two messages came in a second apart;
- answers "what day is it", "until Friday", "tomorrow at nine" from its training data or not at
  all, and writes dates into files and notes that are wrong;
- has no way to give a time in the users' local time: a session has no time zone, and a UTC time is
  not what the users of a session in Berlin mean by "at nine".

## What is wanted

- Every message the model is given carries the time it was said, in its text.
- At the end, as the last message before the model answers, the model is told the current date and
  time - presumably by a `PromptProvider`.
- A session has a time zone; times are given in it, and formatted in the language of the channel
  ([56](56-response-language.md)).

## Considerations

- **Where the time is put into a message.** The sender is marked in one place, the
  `ContextResolver`, from the profile, so that the stored transcript keeps the message as received;
  the time belongs next to it, by the same rule: `@alice (Sun 18 Oct, 14:32): text`, `<message
  from="@alice" at="…">`. Whether it is given, and in which form, is then part of the
  `ModelProfile` like `sender_format` - which suits which model goes into
  [LLM-MODELS.md](../docs/LLM-MODELS.md).
- **The agent's own messages.** The sender is never marked on the agent's own messages, because a
  model whose own messages carry a marker imitates it and starts its answers with one. The same is
  to be expected of a time. Options: no time on its own messages (it knows the time from the
  messages around them, but not how long it worked); a time only on its own messages of earlier
  turns; or the time on all of them and the control tokens / the output cleaned of a leading
  marker. To be tried on the models in use before deciding.
- **The prompt cache.** The transcript is a stable prefix, and its marking is computed from the
  stored message, so a timestamp in it costs nothing per turn - as long as it is formatted the same
  every time. A change of the session's time zone or the channel's language rewrites every marker
  once and invalidates the cache once; that is acceptable. A *relative* form ("5 minutes ago")
  changes with every turn and would invalidate the cache every time; it is ruled out for the
  transcript.
- **Which form.** A long localised form (`Sonntag, 18. Oktober 2026, 14:32`) on every message
  costs tokens on every message of a long conversation; a short one (`18.10. 14:32`, the date only
  when it changes from the message before) costs few. Possible: short in the transcript, long and
  complete - weekday, date, time, time zone - only in the current-time part. The short form in the
  channel's language too, as wanted, but without the weekday's name where the date is numeric.
- **The current time at the end.** `PromptPosition` has `Start` (the system prompt) and
  `BeforeTurnInput` (before the newest message). The current time changes with every call, so it
  can be neither in the system prompt nor anywhere before the transcript's end without invalidating
  the cache. Wanted is the very end, after the turn's input - a new position (`End`, after the last
  entry of the transcript, transient like `BeforeTurnInput`). To be weighed: a user message after
  the input may draw the model's answer to the time instead of the question; a part at
  `BeforeTurnInput` is right before the input and would serve as well for the cache, since it moves
  with the input. In a turn with several model calls (tool calls, a plan) the time is read anew for
  every call.
- **What "now" is.** The clock of the server, read when the context is resolved - not the time the
  turn began. Through a clock the tests can set.
- **The time zone.** An IANA name (`Europe/Berlin`), a value type of the model checked on
  assignment like `Language` (strings at the API, the model validates), stored on the `Session`,
  `None` for UTC. Changed by whoever may change the session, as a new name is, with its update
  event; a session template may name one. Its default for a new session: the creator's - a
  `preferred_time_zone` in the `UserProfile` beside `preferred_language`, which the web UI can
  preset from the browser (`Intl.DateTimeFormat().resolvedOptions().timeZone`) - set by the
  frontend, not the core ([331](331-language-default-in-the-frontend.md)).
- **Time zone on the session, language on the channel.** The language is a property of the channel
  ([56](56-response-language.md)), the time zone one of the session: the users of a session are in
  one place, the channels of a session may be held in different languages. A channel held in no
  language in particular formats in a neutral form (ISO 8601, English names).
- **Formatting by language.** `chrono` knows time zones only with `chrono-tz` and localised names
  only with the feature `unstable-locales` (`format_localized`, by POSIX locale, not by BCP 47 tag);
  `icu4x` formats properly by BCP 47 tag, with its data, at the cost of a large dependency;
  `jiff` brings time zones but no localisation. To be decided with the form above - the short
  numeric form barely needs localisation, the long one does. The localisation belongs to the model
  (`Language`), next to `Language::names`.
- **Who formats.** The `ContextResolver` is given the session's time zone and the channel's language
  with the profile when the turn begins, as fixed for the turn as the profile; it does not fetch
  them itself. The current-time part reads them through the views at every call, as the language
  part does.
- **External agents.** An external agent keeps its conversation itself and is given what was said
  by the same rule as the marker; the time follows the same rule. Whether it is told the current
  time is a question of the protocol - presumably not.
- **`read_history`.** Gives `at` in RFC 3339 and UTC; it should give the time in the session's time
  zone too, so that the model does not have to convert between two forms of the same time.
- **Prompts in English.** The title and explanation of the current-time part are English like every
  other part; the time itself is in the channel's language. Writing parts in the channel's language
  belongs to [57](57-prompt-catalogue.md).

## Done when

- A session has a time zone, set when it is created and changeable by whoever may change it, over
  REST and in the web UI.
- The model is given every message of others with the time it was said, in the session's time zone
  and the channel's language, in a form its profile chooses; how the agent's own messages are
  treated is decided on the models in use and recorded in LLM-MODELS.md.
- As the last message of every model call the model is told the current date and time, by a
  component a personality includes.
- The prompt cache keeps the transcript: nothing in it changes from turn to turn.

## History

- **2026-10-18** — Opened at the maintainer's request: messages are to reach the models with their
  time in the text, the current date and time is to be given as the last message, presumably by a
  prompt provider, and a session is to have a time zone in which times are formatted, in the
  language of its channel.
- **2026-10-18** — Measured on gemma4:26b (`work/message_times/RESULTS.md`). The time on the
  others' messages is used: without it the model takes a review announced three days before for
  tomorrow, with it it names the date. The agent's own messages must carry none - with a time on
  them it begins its answers with one of its own, made up, in about half of them; with times on
  the others' only, never. The model takes the time of the last message for now, which is wrong
  for a turn that comes later - a question about opening hours asked on Friday evening and
  answered on Sunday: only the current time makes it right. Where the current time is put - the
  system prompt, before the turn's input, at the very end - made no difference to gemma4, nor did
  it distract. In a German channel the English, German and numeric forms worked alike, and none
  pulled the answers into English.
  Decided with the maintainer: the work goes in two steps, the times on the others' messages
  first, then the current time. The times are written in one fixed form for every language,
  `Sun 18 Oct 2026, 14:32` - a weekday by name, which the model uses, and no localisation, so that
  what the model is given does not depend on the channel's language and only the session's time
  zone has to be recorded with a context. The agent's own messages carry no time. A session's
  language, where a channel has none, is [425](425-session-language.md)'s.
- **2026-10-18** — Decided for the first step: whether a model is given the times is a switch of
  its `ModelProfile` (`message_times`), as how it is given senders is; there is no preferred time
  zone in the user's profile - the web UI presets a new session's from the browser. A session
  without a time zone is held in UTC, never in the server's, which is the deployment's and would
  make what a model is given depend on the machine. A context records the time zone it was given
  by, as it records its profile. Neither new field has a default where it is read: stored data
  without them is not read - every channel's history, whose agents' answers carry their context,
  and every agent's state is to be deleted.
- **2026-10-18** — First step done: the times on the others' messages. `TimeZone` (a value of the
  model, an IANA name checked against the database `chrono-tz` compiles in, written as
  `TimeZone::format` writes a time); `Session::time_zone` (`None`: UTC,
  `Session::effective_time_zone`), changed by whoever may change the session
  (`SessionLoop::update_time_zone`, `PUT /sessions/{id}/time-zone`), named by a template or the
  request that creates the session. `ModelProfile::message_times` - `true` unless the component
  (`ollama_client`, `a2a_agent`) is configured otherwise; `decision_policy` gives no times -
  makes the marker say when a message was written, never on the agent's own; the roster's
  explanation says so. The time zone is read through `ActorRuntime::session_time_zone` as a turn
  begins and recorded in `ContextRef::time_zone`; `read_history` writes times alike. The web UI
  presets a new session's time zone from the browser and changes it from the session's menu; the
  TUI leaves it at UTC. A channel has no time zone of its own (decided with the maintainer: the zone
  belongs to the people, not the conversation; an issue of its own if a case comes up). Stored data
  to delete: every channel's history and every agent's state - neither `message_times` nor the
  context's time zone has a default where they are read.
- **2026-10-18** — Second step done: the component `current_time_prompt_provider` tells the agent
  the date and time, written out (`TimeZone::format_long`), in the time zone the turn records, and
  is wired into the seed's `channel-context` - not into the external personality, whose remote
  agent keeps its own clock. It goes before the turn's input (`BeforeTurnInput`, its default
  `position`), not as the very last message the issue asked for: where it stands made no
  difference to the model measured, and before the input it needs no new position. Unlike the
  issue's "every model call", it is read as the turn's context is built, once per turn - parts are
  built once for a turn - so that a long plan keeps the time it began at; worth an issue of its own
  if a plan's later steps need the time.

