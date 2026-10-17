# 203 — The chat view shows only one answer under way per channel

**Status:** done
**Area:** User interaction
**Summary:** The chat view keeps one answer under way per channel, so the streamed answers of agents answering at once run into each other or vanish.

## The problem

`ChatView` keeps what is under way in a channel as one answer: `isGenerating`, `streamingMessage`,
`streamingSender`, `generatingActor` and `currentStep` exist once per channel. But several agents
can answer at the same time - under `broadcast_policy` every agent answers a message, and since
[202](202-send-while-generating.md) a user can send a message while an agent still answers, so that
another agent starts before the first has ended.

What then happens, as far as the code shows (not yet seen in the browser):

- `PartialMessageGenerated` of every agent is appended to the same `streamingMessage`, so the
  partial texts of two agents run into each other, under the name of whoever sent the last part.
- The first `FullResponseGenerated` or `TurnFailed` clears the streamed message of all and sets
  `isGenerating` to false: the other agent's answer so far disappears, and with it the stop
  button, until its next part sets them again.
- `ParticipantRemoved` ends the answer only of `generatingActor`, which is whoever sent last.
- `currentStep` shows the plan step of whichever agent reported last.

## Why it matters

- Two answers mixed into one text are unreadable, and an answer that vanishes looks like a failure.
- The stop button disappearing while an agent still runs suggests there is nothing left to stop.

## Considerations

- **Keep what is under way per agent:** a map from the sender (`actorKey`) to its streamed text,
  name and plan step, filled by `ResponseStarted` and `PartialMessageGenerated` and emptied by that
  sender's `FullResponseGenerated`, `TurnFailed` or `ParticipantRemoved`. The view shows one
  bubble per entry.
- **The stop button** is shown while the map is not empty. It cancels the channel, so every agent's
  turn, as it does today; stopping one agent only is a different question.
- **The "stopped" bubble:** today a streamed message stays, marked stopped, once generation ended
  without an answer; with several entries, that has to stay per agent.
- **The snapshot** clears what is under way, as today; it would clear the whole map.

## Done when

- Several agents answering at once are shown as separate answers under way, each under its sender.
- One agent's end does not end what another shows, and the stop button stays while any agent
  generates.
- WEB-UI.md says so.

## History

- **2026-10-03** — Issue created, after [202](202-send-while-generating.md) let users send while
  an agent answers, which makes simultaneous answers more frequent. The problem already existed
  under `broadcast_policy`.
- **2026-10-04** — Decided with the maintainer: complete messages are shown above every answer
  under way, so an answer that ends first moves up past one that began before it. The channel's
  history holds messages in the order they were completed, and the event stream delivers them in
  that order; showing them so keeps the view as a reload shows it. Answers under way keep the order
  they began in, and an agent that goes on after a complete message - a tool call - keeps its place
  below.
- **2026-10-04** — Done. `ChatView` keeps the answers under way as a list of `Answer` (sender,
  name, streamed reasoning and content, plan step), changed by `withAnswer`: `ResponseStarted`
  begins an answer at the end, `PartialMessageGenerated` adds to its sender's,
  `CompleteMessageGenerated` empties it, and `FullResponseGenerated`, `TurnFailed` and
  `ParticipantRemoved` end it; the snapshot ends all. One bubble per answer, keyed by its sender,
  and one plan-step bar per answer that has a step, naming the agent. `isGenerating` - the stop
  button - is whether any answer is under way. The "stopped" bubble is gone: since every turn ends
  with `FullResponseGenerated` or `TurnFailed`, both of which clear what was under way, it could no
  longer be reached. Not verified in the browser with two agents answering at once.
- **2026-10-08** — The plan-step bar above the chat is gone: each step of a plan is a block of its
  own in the chat, headed by the step, live and after a reload alike
  ([263](263-plan-steps-not-in-chat.md)).
