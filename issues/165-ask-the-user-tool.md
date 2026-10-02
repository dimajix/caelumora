# 165 — An agent cannot ask the user a question and wait for the answer

**Status:** open
**Area:** User interaction

## The problem

An agent that needs a decision or a piece of information from the user can only end its turn with a
question in plain text and hope the next message answers it. There is no tool for asking, so:

- the question has no form: a yes/no question, a choice among options and a free-text question all
  look like prose, and the user answers in prose that the model has to interpret again;
- the turn ends, and with it whatever the agent was doing - a plan in progress, tool calls still to
  come - instead of waiting for the answer and going on;
- the web UI cannot show the question as what it is (buttons, a list to choose from, an input
  field), because it does not know that a question was asked.

Wanted, from the maintainer: an "ask the user" tool with three kinds of question - yes/no, a choice
among several options (one or several), and free text - and its proper integration into the web UI's
`ChatView`: the question is shown as such, and the answer is given in the form the question asks
for.

## Why it matters

- Agents that work on a task with the user - planning, approving a draft, choosing between
  alternatives - need a dependable way to ask without losing the turn.
- A structured answer is unambiguous: "option 2" or `true` instead of a sentence the model reads
  again.
- The UI needs **tool-specific presentation** anyway: today every tool call is shown alike (name,
  arguments, result). The question tool is the first that needs its own, and the infrastructure it
  brings - a handler per kind of tool call, which knows how to show it and in which form an answer
  is given - is what later tools build on.

## Considerations

- **Waiting for the answer: along the approval flow.** A tool call that needs a user before it can
  complete exists already: one that requires approval (`ApprovalRequested`, `WaitingForApproval`,
  `ToolCallState`, `POST …/approvals`, issues 81, 155). An answer is an approval with a payload: the
  turn waits in the same way, survives a restart the same way (its state is stored), is cancelled
  and stopped the same way (issues 152, 161). The alternatives - the tool ends the turn and the next
  message is the answer, or a state of its own - either lose what the turn was doing or build a
  second waiting mechanism beside the one there is. To be decided: whether answering *is* an
  approval (the `ApprovalResponse` gains an answer) or a sibling with the same shape.
- **Who may answer.** For an approval anyone in the channel may (issue 14, open). A question is
  usually meant for the user the agent works with - the one whose message started the turn, or one
  named in the call. Whether others may answer, and what happens in a channel with several users, is
  to be decided, and should be decided together with 14.
- **The tool.** One tool, `ask_user`, with `kind` (`yes_no`, `choice`, `text`), the `question`, for
  `choice` the `options` and whether several may be chosen; or one tool per kind, whose schemas are
  simpler for the model. Its result is the answer, typed: `{"answer": true}`, `{"chosen": ["…"]}`,
  `{"text": "…"}`, or that the user declined to answer. A built-in component (`ask_user_tool`), like
  `sleep_tool`.
- **Prior art: Hermes' `clarify` and Claude Code's `AskUserQuestion`.** Both are one tool whose
  shape answers several of the questions above, and are a model for `ask_user`:
  - *Several questions in one call*: `questions` holds 1-5 entries; independent questions belong in
    one call, a question whose answer changes another is asked in a call of its own. The call still
    waits once, and its result is a list, `{"responses": [...]}` in the order of the questions.
  - *The kind follows from the shape, there is no `kind`*: without `choices` a question is free
    text, with `choices` a single choice, with `multi_select: true` a multiple choice. The schema is
    simpler for the model and cannot contradict itself. Decided: **yes/no is a choice with two
    options**, not a kind of its own - the UI may still show such a choice as two buttons, but the
    answer is the chosen option, not a boolean.
  - *An "Other" free-text row* is appended to every choice by the UI, so a user is not forced to
    decline an answer that was not foreseen. A choice's answer is therefore
    `{"chosen": [...], "other": "…"}`.
  - *A recommended option*: Hermes marks the first option as recommended. Better an explicit
    `recommended: true` on the option than a convention on the order that the presentation depends
    on.
  - *Limits* (at most 5 questions, 4 options each) are checked by the tool and reported as a tool
    error, not left to the UI.
  - *A short `header` per question* (Claude Code: at most 12 characters) labels the question where
    it is shown collapsed, e.g. an answered question in the history.
  - *Guidance in the tool description*: options go only into `choices`, never enumerated in the
    question text (they would be prose the user cannot click); independent questions go into one
    call. This belongs into `ask_user`'s description.
  - *Not taken over*: Hermes' `timedout`. Our question waits for good and survives a restart; a
    cancel or a stop ends the turn as for an approval. What is needed instead is an explicit
    "declined", per question or for the whole call, which reaches the model as the result so that
    it can go on. A timeout can be added later.
- **How the UI knows how to show a call.** By tool name is brittle - tools come from components and
  MCP servers, and names are configured. Better a presentation the tool declares, carried with the
  tool definition (`Tool` beside `visibility` and `classification`) and with the call's request to
  the UI: e.g. `presentation: question`. The UI then has a registry of handlers by presentation,
  with the default handler for everything else. The same mechanism could later present e.g. a plan
  or a file diff.
- **What a handler does.** It renders the pending call (the question and its controls), renders the
  answered call afterwards (the question and the answer given, in the history and on reload), and
  builds the answer in the form the call needs and sends it. The answer format is part of the
  contract between the tool and its handler, and should be in the model (`caelumora-model`), so that
  runtime, REST and UI agree.
- **What the channel sees.** The question and the answer are part of the conversation; whether they
  appear as the tool call and its result (as now for shown tools) or as messages of their own, and
  how they are classified (as the turn; the answer as the channel), is to be decided.
- **Other frontends.** The TUI shows a pending question at least as text and accepts an answer typed
  in the expected form.

## Done when

- An agent can ask a yes/no, a choice and a free-text question; its turn waits for the answer,
  survives a restart while waiting, and ends on a cancel or a stop like a turn waiting for an
  approval.
- The answer reaches the model typed, in the form the question asked for.
- The web UI shows a pending question with controls of its kind, sends the answer in the expected
  form, and shows question and answer in the history - through a handler chosen by the call's
  presentation, with the default presentation for every other tool.

## History

- **2026-10-01** — Opened at the maintainer's request.
- **2026-10-02** — Compared with Hermes' `clarify` and Claude Code's `AskUserQuestion`: several
  questions per call, the kind derived from the shape, an "Other" row, an explicit recommended
  option, a `header`, and an explicit "declined" instead of a timeout. Decided by the maintainer:
  yes/no is a choice with two options, not a kind of its own.
