# 424 — An agent knows neither when something was said nor what time it is

**Status:** open
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
