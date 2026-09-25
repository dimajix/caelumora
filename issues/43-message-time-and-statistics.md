# 43 — The web UI shows neither when a message was written nor what it cost

**Status:** open
**Area:** User interaction

## The problem

Every message carries a `timestamp`, and messages produced by a model also
carry usage statistics - `prompt_tokens`, `completion_tokens`,
`prompt_duration`, `completion_duration`, `total_duration` (nanoseconds, as
reported by the LLM backend). The web UI shows none of it: a message has a
sender and a text, nothing about when it was written or how long and how
many tokens it took.

Wanted:
- **Date and time on every message, always**, next to the participant's
  name - before or after it.
- **The statistics behind an "(i)" symbol**, shown on hover.

## Why it matters

- Without time, a conversation has no shape: which answers came long after
  the question, what happened while one was away, whether a message is from
  today or last week.
- Token counts and durations are what one looks at when an agent is slow or
  expensive, and when comparing models or personalities. They are already
  recorded; not showing them wastes them.

## Considerations

- **Where:** in the header of a message, which `MessageView` renders for
  both the chat and the context panel ([41](41-show-message-context.md)) -
  one change covers both.
- **Date and time format:** the timestamp is UTC; show it in the browser's
  time zone and locale. "Always date and time" rules out relative times
  ("5 minutes ago") as the only form; a relative form could be the tooltip
  of the absolute one, or not exist at all. A compact form (time only for
  today, date and time otherwise) would contradict "always", so the full
  date and time it is unless decided otherwise.
- **Which messages have a time:** stored and streamed messages carry one.
  The UI-only notices - a failed turn, a command's output - have none of
  their own; the envelope's timestamp (failed turns, announced commands) or
  the time of receipt (private command output) can stand in. The message
  still being streamed has no final time yet.
- **The statistics:** only messages produced by a model have them; the "(i)"
  appears only where there is something to show. Durations in seconds with
  sensible precision, tokens as counts, and the throughput that follows
  from them (completion tokens per second) is the number people usually
  want. Fields that are missing are left out rather than shown as zero.
- **Hover is not enough on its own:** a tooltip that only appears on mouse
  hover is unreachable by keyboard and on touch screens. The symbol should
  be focusable and show the same on focus and on tap.
- **Which model produced it** would belong in the same tooltip, but the
  message does not record it today; that would be a server change of its
  own.

## Done when

- Every message in the web UI shows its date and time next to the sender's
  name, in the browser's time zone.
- Messages with usage statistics show an "(i)" symbol that reveals them on
  hover, focus and tap.

## History

- **2026-09-25** — Opened at the maintainer's request; only recorded for
  now.
