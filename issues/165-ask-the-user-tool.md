# 165 — An agent cannot ask the user a question and wait for the answer

**Status:** in progress
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

## Decisions

Taken with the maintainer on 2026-10-07, after weighing the alternatives recorded under History.

- **Questions are a facet of a tool, not a tool of their own.** Before a call runs, the runtime asks
  its tool whether it has questions for a user (`ToolProvider::questions`, beside `describe` and
  `matcher`). The answers are handed to the tool's `execute` as a parameter of their own; the tool
  does with them what it wants. `ask_user` is only the simplest such tool: it formats the answers
  as its result.
- **The answers are not messages.** What the model learns of them is what the tool puts into its
  result - `ask_user` all of it, a tool asking for something sensitive nothing. This is a channel
  to a tool that bypasses the model. For the record, the result keeps the questions and the
  answers to them beside what the tool gave (`ToolCallResult::answers`, step 3a), into the history
  and the transcript, never to the model - unless they are sensitive.
- **The turn waits, it is not ended.** The call is parked like a call waiting for an approval: the
  turn is stored, survives a restart, and goes on with the result once the answers have come - a
  plan in progress included. The tool never blocks; `execute` runs only once the answers are there.
  A tool that waited inside `execute` would lose the question on a restart and would need a way to
  the agent's inbox around its run loop.
- **No approver.** A question is not decided by a strategy among several people: the first valid
  answer of someone asked settles it. The question goes from the agent to the users, the answer from
  a user to the agent alone. This is a deliberate difference to approvals, which go through the
  `ApproverActor` to everyone who may approve.
- **Approval first, then the questions.** A call whose tool requires approval and has questions is
  put up for approval first; only an approved call asks its questions. Whoever approves sees the
  arguments, never the answers. A place's `ask` rule concerns the approval, not the questions.
- **Prose instead of the form declines.** A message from someone asked, written while the question
  is open, declines the whole call; the message itself is then processed as usual.
- **`ask` is required.** `ask_user` names whom it asks, at least one handle. Asking everyone who
  may write would make a question fragile in a busy channel: any message of anyone would decline
  it.
- **Declining.** A single question can be declined within the answers; declining the whole call -
  by the button, or by writing instead - answers it as `declined` without running the tool.
- **Lifecycle and outcome are separated.** The state of a call in its turn becomes a lifecycle enum
  (`ToolCallStatus`) whose last variant holds the result; how a call ended is said by the result's
  `ToolResultStatus` alone. `ApprovalStatus` is again only the approver's decision.
- **Presentation is display only.** Whether a call asks is decided by `questions`, not by the
  presentation. `Tool::presentation` only says how a call and its result are shown in the history.
  Questions and answers need none of their own: every result carries their record, which the
  frame of every call shows (step 3a) - `ask_user` included.
- **Prepared for sensitive questions.** No tool asking for something sensitive is built here, but
  everything it needs is: a tool says of its questions whether they are *sensitive*, and that
  changes what is stored, logged, shown and to whom (see
  [Sensitive questions](#sensitive-questions)). This issue is done only once the runtime keeps
  those rules, tested with a sensitive test tool.

## Plan

### The model (`caelumora-model`)

A new module `question.rs`, the contract between tools, runtime, REST and UI:

- `Questions` - what a call asks: `questions: Vec<Question>`, whom it asks (`ask: Vec<Addressee>`)
  and whether it is `sensitive`. Set by the tool, never by the model: a model can only fill in
  what a tool such as `ask_user` lets it fill in.
- `Question` - `header` (at most 12 characters), `question`, `choices: Vec<Choice>` and
  `multi_select`. Without choices it is free text; with choices a single choice; with
  `multi_select` a multiple choice. Yes/no is a choice with two options.
- `Choice` - `label`, an optional `description`, `recommended: bool`.
- `Addressee` - `Handle(String)` as a model writes it (`ask_user`), or `Owner`, the agent's owner,
  for a tool that decides itself whom it asks. The runtime resolves them to `ActorId`s when the
  call is admitted; an unknown handle makes the call invalid.
- `Answers` - the questions they answer, who answered (`ActorId` and name, set by the runtime) and
  one `Response` per question, in their order:
  `Chosen { chosen: Vec<String>, other: Option<String> }`, `Text { text }` or `Declined`. The
  "Other" row of a choice is `other`.
- `Questions::check()` - the limits (1-5 questions, 2-4 choices each, the header's length),
  reported as invalid arguments. `Questions::check_answers(&Answers)` - one response per question,
  of the kind the question asks, chosen labels among the choices, one only without `multi_select`.
- `QuestionId` (`define_id!`) - the runtime's id of an open question. The answer refers to it, not
  to `ToolCall::id`: that one is the model's, optional, and some backends repeat it across turns.
- `QuestionRequest` - what the users are sent: `id`, session, channel, `agent_id`, the
  `tool_call` (as published, by its visibility), the `Questions` with resolved addressees.
- `ToolResultStatus::Declined`, with its fixed text in `crate::tool` (`declined_result`).

`Answers` has a `Debug` that shows the number of responses, never their content. Answers are
logged nowhere but explicitly, and those of sensitive questions not at all.

### The state of a call (`caelumora-model/src/tool.rs`, `turn.rs`)

```rust
pub struct ToolCallState {
    pub tool_call: ToolCall,
    pub status: ToolCallStatus,
}

pub enum ToolCallStatus {
    /// A person decides whether it may run; it asks `questions` afterwards.
    WaitingApproval { id: ApprovalId, description: String, explicit: bool, suggested: ToolMatcher, questions: Option<Questions> },
    /// A person gives the answers it runs with.
    WaitingUser { id: QuestionId, questions: Questions },
    /// It may run, with the answers if it asked.
    Ready { answers: Option<Answers> },
    /// It ended; how is said by the result.
    Finished(ToolCallResult),
}
```

- What only an approval needs - `description`, `explicit`, `suggested`, the `ApprovalId` - lives in
  `WaitingApproval` alone, no longer on every call.
- `ApprovalStatus` loses `Pending` and is `Approved | Denied | Timeout`, the approver's decision.
- The answers of a sensitive question are never serialized (`skip_serializing_if` on their
  sensitivity). Nothing is lost by it: a turn resumed in `ExecutingTools` answers every `Ready`
  call as `not_executed` anyway (`prepare_resume`).
- Turns stored in the old, flat form (`id`, `status`, `description`, ..., `result`) are read through
  `#[serde(from = ...)]` with the old form as a private type.
- `ActorState::WaitingForApproval` becomes `WaitingForUser`, with a serde alias for stored states.
- `TurnState::pending_approvals` reads `WaitingApproval`; a new `TurnState::pending_questions`
  reads `WaitingUser`.

### Tools (`caelumora-runtime/src/tool.rs`)

- `ToolProvider::questions(name, params) -> ToolResult<Option<Questions>>`, by default `None`; an
  error is invalid arguments, as with `describe`.
- `ToolProvider::execute(name, params, answers: Option<&Answers>, turn)` and
  `ToolExecutor::execute(call, answers, turn)`: every implementation gets the parameter; those
  that do not ask ignore it.
- `Tool::presentation` (`Default` so far), stamped on `ToolCall::presentation` by the agent like
  `visibility`, so that the history can be shown as the call was made.
- `GuardedToolExecutor::admit`, in this order: the policy, a place's `deny` rule, `describe`, then
  `questions` and `Questions::check`, and the addressees resolved - each failure answered at once.
  Then: approval if the tool or a place's rule asks for it (`WaitingApproval`, keeping the
  questions), else the questions (`WaitingUser`), else `Ready`. `Admission` gets the variant for
  questions.
- `GuardedToolExecutor::execute` hands the answers through; the result is made and classified as
  for every call, in the one place.

### The flow (`caelumora-runtime`)

1. **Asking.** `AgentActor::request_approvals` sends `ApprovalRequested` for `WaitingApproval` and
   the new `QuestionRequested(QuestionRequest)` for `WaitingUser`. `ToolCalls::decide` turns an
   approval into `WaitingUser` if the call has questions - and returns the requests to send - or
   into `Ready`; a denial or timeout into `Finished` with the result at once.
2. **Routing.** The moderator sends `QuestionRequested` to the users of the channel - only to the
   addressees if sensitive - never to the approver or other agents. On a restart the agent sends
   the open questions again from its turn, with the same ids (as `request_pending_approvals`).
3. **Waiting.** `handle_waiting_for_user` (today `handle_waiting_for_approval`) waits for approval
   decisions, for `QuestionAnswered` and `QuestionDeclined` - routed from the user to the asking
   agent only - and for a message from someone asked while a question is open. A cancel or a stop
   ends the waiting as now.
4. **Answering.** `UserActor::send_answer` checks the new action `AnswerQuestion` - who may write in
   the channel - with `Enforcement::require` and sends the answer. The agent takes it only if its
   sender is an addressee of the question, the question is still open and
   `Questions::check_answers` accepts it; else it is left out and logged, without its content.
   Who answered is set into the `Answers` by the agent from the envelope, never from the event's
   content. The first accepted answer settles the call (`Ready { answers }`); later ones find no
   open question.
5. **Declining.** `QuestionDeclined`, or a message from an addressee, answers the call as
   `declined` (`Finished`). The message stays the next to be processed after the turn - whether by
   leaving it queued or by taking it in again is decided in the step.
6. **Closing.** Once a question is no longer open - answered, declined, or abandoned as the turn
   ends - the agent sends `QuestionClosed { id }` to the users it asked, so that every frontend
   closes its form; `withdraw_approval_requests` becomes the one place that closes both kinds.
7. **Running.** `ExecutingTools` runs the `Ready` calls with their answers; `collect` builds the
   tool message once every call is `Finished`, and the turn goes on in `Planning`.

The snapshot of a member gets `pending_questions` beside `pending_approvals`, filtered per viewer:
a sensitive question only for its addressees.

### Sensitive questions

A tool says per call whether its questions are sensitive (`Questions::sensitive`); `ask_user` never
is, since its answers go to the model. For sensitive questions:

1. **Not stored.** The answers are kept in memory only, until the tool ran; they are not in the
   stored turn, and the result keeps no record of them - not even who declined.
2. **Not logged.** Not even at `trace`; `Answers`' `Debug` hides them for every question.
3. **Only to those asked.** The request, the snapshot entry and `QuestionClosed` reach only the
   addressees; only their answer counts.
4. **Whom it asks is the tool's.** A sensitive tool does not take its addressees from the model
   (e.g. `Addressee::Owner`), so that the model cannot choose who is asked for a password.
5. **The UI says where the answer goes.** For a sensitive question: "goes only to the tool <name>,
   the agent does not see it", input masked; otherwise: "goes to the agent". The flag comes from
   the tool, so a model cannot make an `ask_user` question look like a trustworthy tool's.
6. **The tool's result does not repeat them** - the tool's responsibility, documented in
   [TOOLS-AND-COMMANDS.md](../docs/TOOLS-AND-COMMANDS.md).
7. **Only the in-memory transport.** A transport that keeps envelopes
   ([122](122-several-instances.md)) must not keep answers to sensitive questions.

### REST and frontends

- `POST /sessions/{session}/channels/{channel}/questions/{id}` with `agent_id` and either
  `answers` or `declined: true`; errors as `ApiError`. The snapshot gains `pending_questions`;
  `ToolResultStatus` gains `declined`. Additions only, compatible with the web UI.
- **Web UI.** A form for `QuestionRequest`s, generic over `Questions`: buttons for a choice with
  two options, a list for one or several, a text field, the "Other" row, the recommended option
  marked, a "decline" button, masked input and the hint where the answer goes. Only addressees get
  the controls; others see the question. A registry of handlers by `ToolCall::presentation`, with
  today's rendering as the default handler. The frame of every call shows the record of its
  questions and answers, where its result has one - `ask_user` like every other tool that asks.
- **TUI.** A pending question shown as text, answered by typed input in the expected form.

### Steps

Each a step of its own, to be reviewed separately; 0 to 2 change no behaviour.

0. **The state of a call.** `ToolCallStatus` with the result in `Finished`; `ApprovalStatus`
   without `Pending`; reading the old stored form; `WaitingForApproval` renamed to
   `WaitingForUser` (alias), the web UI and the TUI along.
1. **Presentation.** `Tool::presentation`, stamped on `ToolCall`; the web UI's registry of handlers
   with the default handler.
2. **Questions in the model and the tool traits.** `question.rs` with its checks and tests;
   `ToolProvider::questions`; the answers as a parameter of `execute` throughout.
3. **Waiting for answers.** `admit` with questions and approval first; the events and their
   routing; `AnswerQuestion` and `UserActor::send_answer`; waiting, declining by prose, closing,
   asking again after a restart; `pending_questions`; the REST endpoint; `declined`; the rules for
   sensitive questions. Tested with a test tool, a sensitive one among them; the security suite
   gets: an answer of someone not asked is left out, a sensitive question and its snapshot entry
   reach no one else, `AnswerQuestion` is required. Documented in TOOLS-AND-COMMANDS.md,
   AGENT-TURN.md, SECURITY.md, REST-SERVER.md.
3a. **The record of the answers.** `ToolCallResult::answers`, kept by the runtime as a call
   finishes - with the questions, who answered or declined and what - unless the questions are
   sensitive; published as `content` is, left out where it is the owner's; never given to a model
   (`ToolCallResult::for_the_model`).
4. **`ask_user_tool`.** The tool with `questions` (array of 1-5) and `ask` (handles, required, at
   least one); its description tells the model to put options only into `choices`, never into the
   question text, and to ask independent questions in one call; its result
   `{"answered_by": ..., "responses": [...]}`. Into the table of built-in tools.
5. **Web UI.** The form, the record of questions and answers in the frame of every call, the REST
   client, the pending questions in `ChatView` and in the snapshot.
6. **TUI.**

### Open questions

- **Where handles are resolved.** `admit` needs the channel's `MentionDirectory`, which today the
  `ChannelLoop` holds; how the guard reaches it is decided in step 3.
- **The names of the events** (`QuestionRequested`, `QuestionAnswered`, `QuestionDeclined`,
  `QuestionClosed`) are working titles.

## History

- **2026-10-01** — Opened at the maintainer's request.
- **2026-10-02** — Compared with Hermes' `clarify` and Claude Code's `AskUserQuestion`: several
  questions per call, the kind derived from the shape, an "Other" row, an explicit recommended
  option, a `header`, and an explicit "declined" instead of a timeout. Decided by the maintainer:
  yes/no is a choice with two options, not a kind of its own.
- **2026-10-07** — Discussed with the maintainer; decisions and plan above. Weighed and left:
  answering as an approval with a payload, decided by the `ApproverActor` (its strategies, timeout
  and personal rules do not fit a question, which the first valid answer settles); `ask_user` ending
  the turn and the answer coming as a normal message built by the UI (simple, but a plan in
  progress would be lost and the model could not go on with the answer); the tool waiting inside
  `execute` (lost on a restart, and no way to the agent's inbox); extending `ApprovalStatus`, then a
  lifecycle enum beside `result` (outcome and lifecycle mixed, or two sources of one truth). The
  maintainer's proposal - the runtime asks the tool for its questions, the answers go to the tool's
  `execute`, not into history or transcript - opens a channel to tools that bypasses the model, for
  sensitive information; this issue prepares for such a tool without building one. Decided by the
  maintainer: approval before the questions; a message instead of the form declines the call;
  `ask_user`'s `ask` is required.
- **2026-10-07** — Step 0 done, without a change of behaviour: `ToolCallState` is the call and its
  `ToolCallStatus` (`WaitingApproval` with the approval's id, description, `explicit` and
  `suggested`; `Ready`; `Finished` with the result); `ToolCalls::decide` finishes a denied or
  timed-out call at once, `to_run` and `record` go by the call's place in the step.
  `ApprovalStatus` lost `Pending`. `ActorState::WaitingForApproval` is `WaitingForUser`, the web UI
  along. Stored states of the old form are not read: old data may still be deleted.
- **2026-10-07** — Step 1 done, without a change of behaviour: `ToolPresentation` (only `Default`
  so far; `Question` comes with `ask_user` in step 4, together with a test of the stamping) on
  `Tool` and stamped on `ToolCall` by `label` as the visibility is. In the web UI `ToolCallView`
  draws the frame of every call and leaves its content to the handler of its presentation
  (`PRESENTATIONS`), the default one being what `MessageView` showed before.
- **2026-10-07** — Step 2 done, without a change of behaviour: `caelumora_model::question` with
  `Questions`, `Question`, `Choice`, `Addressee`, `Answers` and `Response`, their limits and checks
  (`Questions::check`, `Questions::check_responses`) and `QuestionError`; `Answers` and `Response`
  show no content in `Debug`. `ToolProvider::questions` (by default none) and
  `ToolExecutor::questions` (no default, so that no executor drops them); the answers are a
  parameter of `execute` from the guard through the executor to the provider. Nothing asks yet:
  the agent runs every call with none. `QuestionId` comes with the waiting in step 3.
- **2026-10-07** — Step 3 done: a call that asks waits for the answers, survives a restart, and
  goes on with them. Decided while building, beyond the plan:
  - The answers are never stored, sensitive or not: `ToolCallStatus::Ready` skips them, since a
    call ready when its turn is resumed is answered as not executed anyway. Sensitivity decides
    whom the questions are shown to, not whether answers are kept.
  - A message instead of the form is declined by the user's actor, not by the agent: the agent's
    inbox can only be searched with a fixed test (`EventPredicate`), which cannot know whom the
    questions were put to. The `UserActor` keeps the questions shown to its user and, before a
    message of theirs, sends `QuestionDeclined` for those put to them; it also refuses an answer
    that cannot count before sending it, and does not echo the answers to the user's frontends.
  - Addressees are found by the channel (`ChannelLoop::put`, `Questions::resolve`) - among its
    users only, an agent answers no questions - and kept as `Addressee::Actor`. The guard reaches
    the running channel through a `ChannelHandle`, the handle the channel's runtime already held
    to it, now a type of its own and given to the guard when it is made.
  - `AnswerQuestion` carries the channel's attributes and is granted as `RetryMessage` is, to
    whoever takes part; that the one answering was asked is the agent's check. The snapshot shows
    the open questions under `Detail::APPROVALS` beside the approvals.
  - The REST endpoint is `POST …/questions/{qid}` with `{responses}` or `{declined: true}`.
- **2026-10-07** — Step 3a, at the maintainer's request: the answers to questions that are not
  sensitive are kept for the record, with the result of their call (`ToolCallResult::answers`) -
  into the transcript and the history, not as messages of their own, which would ask agents to
  respond and count as what users said. `Answers` carries the questions it answers; a declined call
  keeps who declined (`Answers::declined`); sensitive questions leave no record
  (`Answers::record`). The record is published as `content` is and dropped where that is the
  owner's (`Message::published`). The Ollama backend gave the model the whole result as JSON, and
  would have given it the record too: it now gives `ToolCallResult::for_the_model`. With the record
  on every result, `ToolPresentation::Question` is not needed: the frame of every call shows
  questions and answers, `ask_user`'s too, so step 4 adds no presentation and the stamping test of
  step 1 waits for a presentation that needs one. Plan adjusted.
- **2026-10-07** — Step 4 done: the component `ask_user_tool` with the tool `ask_user` - its
  arguments `questions` and `ask` (handles, required) are its questions, never sensitive; its
  result is `{"answered_by", "responses"}`, classified as the channel. Its description tells the
  model to put the possible answers into `choices`, never into the question, to ask independent
  questions in one call, and that a declined call means going on with what the user wrote. A
  handle that names no user of the channel is answered as invalid arguments at once, naming the
  handle. In the tables of components and built-in tools.
- **2026-10-07** — Step 5 done: the web UI shows the open questions above the approvals
  (`QuestionCard`) - from the snapshot, kept up by `QuestionRequested` and `QuestionClosed` -
  with controls only for those asked: choices as buttons, one or several, the recommended one
  marked, an "Other" field, a text field without choices, skipping a question, declining all;
  sensitive questions masked and said to go to the tool alone. Answers are sent with
  `api.answerQuestions`. The frame of every call (`ToolCallView`) shows the record of its
  questions and answers. The response type is `QuestionResponse` in TypeScript, where `Response`
  is fetch's. Type-checked and built; not tried against a running server with a model.
