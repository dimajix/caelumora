# 268 — Choices stand side by side, and an answered question is shown as raw JSON

**Status:** done
**Area:** User interaction
**Summary:** An agent's question offers its choices side by side, their meanings in a list apart, and once answered is shown as raw JSON beside a record without the choices offered.

## The problem

An agent asks with `ask_user` (issue [165](165-ask-the-user-tool.md)). The web UI shows it in two
places, both of which read poorly:

- **While it waits**, `QuestionCard` above the approvals offers each question's choices as buttons
  side by side (`flex flex-wrap`), each only its label, a "(recommended)" behind it. What a choice
  means - its `description` - is a list of its own below the buttons and a tooltip. A choice with a
  label of a few words wraps into the next line or squeezes its neighbours; which description
  belongs to which button has to be looked up.
- **Once answered**, the call is shown in the message like any other call: `ask_user` declares no
  presentation, so `ToolCallView` shows its arguments - the questions - as a line of JSON and its
  result - `{answered_by, responses}` - as another. Below them the frame shows the record of the
  questions (`QuestionsRecord`): each question's text and its answer in small type, without the
  choices that were offered. What was asked is said three times, and readably only once, and
  barely. While the call waits, the same message shows it as "Executing" with the raw arguments,
  next to the card.

## Why it matters

A question is where an agent hands a decision to a person. The person has to see the choices and
what each means at a glance, and later anyone reading the channel should see what was decided and
among what - not reconstruct it from JSON.

## Considerations

**Choices one below the other.** Each choice a row of the full width: a marker of how it is chosen -
a circle where one is chosen, a box where several may be (`multi_select`) -, its label, its
description below the label in the same row instead of in a list of its own, and "recommended" as a
small badge. The whole row is the control (`role="radio"` / `role="checkbox"`, `aria-checked`),
reachable by keyboard; a chosen row is marked by its border and background, not only by its marker.
"Other" stays below the choices, as the last row of the question. A question without choices is
unchanged.

**The answered call.** Two ways to give it a presentation:

- **A. A presentation `questions` for `ask_user`** (`ToolPresentation::Questions`), whose handler
  shows the questions and their answers itself, from the arguments and the result. The frame would
  have to leave out its `QuestionsRecord` for this presentation, or show the answers twice.
- **B. Make the record itself the presentation of answered questions, and give `ask_user` a
  presentation that shows nothing raw.** The record is the frame's, for every call that asked -
  `ToolProvider::questions` is generic; `ask_user` is the only tool that asks today, but a tool
  that asks for confirmation or a secret would show its questions the same way. Improving the
  record improves them all. The `questions` presentation of `ask_user` then shows no arguments and
  no result: before the answer, its questions read-only and whom they wait for ("Waiting for
  @alice") - the card is where they are answered -; once answered, nothing beyond the record,
  which says it all.

Recommendation: B. What a person answered belongs to the record of the call, whichever tool
asked; the presentation decides only what of the tool's own arguments and result is shown, and for
`ask_user` that is nothing the record does not say better.

**What the record shows**, from `answers` - whose `questions` carry the choices that were offered:

- each question with its header and its text, line breaks kept;
- for a question with choices: every choice that was offered, as the card showed them, one below
  the other - the chosen ones marked (a filled circle or a checked box, the row highlighted), the
  others muted -, "recommended" kept, and "other" with its text where one was written;
- for a free-text question: the answer as a quote, its line breaks kept;
- a question skipped as "skipped", the whole declined as "Declined by …", otherwise "Answered by
  …";
- sensitive questions leave no record at all (`Answers::record`, see
  [APPROVALS.md](../docs/APPROVALS.md#sensitive-questions)): nothing of them is
  shown, as now.

The card while waiting and the record once answered show a question alike - the same rows, once
with controls, once with the answer - so they share how a question and its choices are drawn
(one component, with and without controls), rather than drawing them twice.

**Server:** `ToolPresentation::Questions` and its use by `ask_user`; nothing else - the record is
in the result already (`ToolResult::answers`). An older client shows the call as before.

## Done when

The card offers choices one below the other as full rows with their descriptions; an answered
`ask_user` call shows its questions with every choice offered and the answer marked among them,
without raw JSON; the record of any call that asked looks the same; `WEB-UI.md` and
`TOOLS-AND-COMMANDS.md` say so.

## History

- **2026-10-09** — Opened at the maintainer's request: choices one below the other, and an
  answered question presented as such.
- **2026-10-09** — Decided with the maintainer: B. The record of a call's answers becomes the
  presentation of answered questions, for every tool that asks; `ask_user` is presented as
  `questions`, which shows nothing raw.
- **2026-10-09** — Done as decided:
  - `QuestionView` draws a question - `QuestionHeading`, and `ChoiceRows` one below the other,
    with controls in `QuestionCard` and without in the record - and holds `AnswersRecord`, which
    replaces the frame's `QuestionsRecord` in `ToolCallView`.
  - `ToolPresentation::Questions`, set by `ask_user`; its handler shows the questions and whom they
    wait for until answered, then nothing beyond the record, which the frame sets apart only where
    the presentation shows something.
