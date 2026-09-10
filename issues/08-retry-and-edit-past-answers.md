# 08 — "Retry" and changing past answers

**Status:** open
**Area:** Sessions & channels

## The problem

A conversation is append-only. Once an agent has answered, there is no way to
ask for a different answer to the same question, and no way to correct a
message already sent and have the conversation continue from the corrected
version.

The workaround — say "no, do it differently" in a new message — leaves the bad
turn in the history, where it keeps influencing every subsequent turn.

## Why it matters

This is the single most-used affordance in comparable chat systems, and its
absence is felt immediately. It is also the cheapest way to recover from a
wrong tool call or a derailed plan without discarding the whole session.

## Considerations

- The hard part is not the UI, it is the state. `MessageHistory` is shared
  input to the agent's cognitive cycle; truncating it means deciding what
  happens to everything derived from the truncated turns — context, tool
  results, and any effect the tools had on the outside world. A retried turn
  cannot un-send an email. See [20](20-transactional-effects.md).
- Two designs, materially different:
  - **Truncate and re-run** — history is rewound to the edited message. Simple
    mental model, destroys the record of what happened.
  - **Branch** — the old turns stay, a new branch continues from the edit
    point. Preserves history (and suits
    [05](05-postgres-event-recorder.md)), but every consumer of history now
    has to know which branch it is on, including the moderator and every
    context provider.
- In a multi-agent channel the question compounds: retrying one agent's turn
  invalidates what the *other* participants said in response to it.
- Whichever is chosen, the `Recorder` stream should keep the discarded turns
  even if history does not.

## Done when

- One of the two designs is chosen, with the reasoning recorded here.
- Retry works for a single-agent channel; the multi-agent case is at least
  explicitly deferred with a stated reason.

## History

- **2026-09-10** — Issue created from `TODO.md`.
