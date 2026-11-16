# 586 — The terminal shows a tool call by its raw arguments

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui` shows a tool call as its tool's name with its arguments as one shortened line of JSON, and its result by the first line of what it answered, so a shell command's output, a file read or written is never seen in the terminal - though the call has a description in words, put to the user when it is to be approved, and a presentation the web UI shows it by.

## The problem

Both ways of drawing of `caelumora-tui` (issue
[569](569-the-terminal-mixes-its-output-into-the-line-typed.md)) tell a tool call in two
lines, apart from one another: `<agent> calls <tool> with <arguments>`, the arguments as JSON
shortened to 160 characters (`call_line` in `screen/line.rs`, `Arguments` in `screen/words.rs`),
and later `<tool> answered: <first line> (and n lines more)` (`outcome`). Three things are
missing:

1.  **The description.** What a call does is described in words by its tool, `Tool::describe`
    over its arguments - `Run in the workspace:` and the command below it, not
    `{"command":"cargo test","wait_secs":30}`. The terminal shows this description only where
    the call is to be approved (`approval_lines`, `what_is_asked`), from
    `ApprovalRequest::description`. Every other call, and the same call once approved, is shown
    by its JSON.
2.  **The presentation.** The agent stamps every call with its tool's `ToolPresentation`
    (`ToolCall::presentation`, [TOOLS.md](../docs/TOOLS.md#built-in-tools)), and the web UI
    shows a call by it (`PRESENTATIONS` in `ToolCallView.tsx`): a shell command as `$ command`
    with its output, its exit code and whether output was dropped; a file edit as the text
    replaced and the text replacing it; a plan as its steps. The terminal knows no
    presentation and shows every call as `default`. The output of a command the agent ran is
    not to be seen in the terminal beyond its first line.
3.  **Room for more than a line.** Showing a command's output in full would fill the
    scrollback with what is mostly not read. The call and its result are two entries in the
    scrollback (`Printed::ToolCall`, `Printed::ToolResult`) with nothing between them that
    could be folded.

## Why it matters

Whoever works with an agent in the terminal cannot follow what it does: which command it ran
and what came of it, which file it changed and how. They have to open the web UI for that, or
trust the agent's own account. The description they saw when they approved a call is gone
from the call itself.

## Considerations

### Where the description comes from

`ToolExecutor` describes every call it admits (`tool.describe(&call.arguments)` in
`caelumora-agents/src/tool/mod.rs`), but keeps the description only in the `ApprovalRequest`,
which goes to those who may approve it. The `ToolCall` that reaches the channel does not carry
it, and the terminal cannot describe the call itself: the tool is the agent's.

- **A. The agent stamps the description on the call**, as it stamps visibility and
  presentation - "what applied then counts". The description is made from the arguments, so
  it is shown exactly where they are: where the arguments are withheld (`Value::Null`, issue
  [569](569-the-terminal-mixes-its-output-into-the-line-typed.md)), so is the description. The
  call has to be described before it is published, not only on admission; a call whose
  arguments the tool refuses has none. *Cost:* a field of `ToolCall`, stored in the
  transcript; the description is computed once and taken by the `ApprovalRequest` from the
  call. The web UI could show it too, in the frame of `ToolCallView` - to be decided with this
  issue, not beside it.
- **B. An event of its own carrying the description.** A second way for what belongs to the
  call; rejected unless A turns out not to fit.

Recommended: A.

### Presentations in the terminal

The web UI's pattern is followed: one handler per presentation name the terminal knows, every
other shown as `default`, and `quiet` left out where it succeeded, as the web UI does
(`withoutQuietCalls`). The words of each handler belong to `screen/words.rs`, so that both ways
of drawing tell the same; each draws them its own way. Which presentations the terminal knows
at first is to be decided - `shell` at least, the file presentations (`file_read`,
`file_write`, `file_edit`) likely next.

What a presentation shows of a result is read from its content: for `shell` the JSON the tool
builds from `CommandState` (`caelumora-api/src/terminal.rs`) - `output`, `status`,
`exit_code`, `truncated`. The web UI reads it in `commandResult`, by field names of its own. A
terminal reading it by hand as well would repeat the structure a second time in Rust; the result
should be read by a type the tool writes it with, which the terminal - that lists
`caelumora-tool-workspace` - can deserialise into.

### Folding

In the full-screen view a call and its result become one entry: a head - who calls what, by
its description, how it ended - and below it what its presentation shows, folded by default.
A click on the head unfolds and folds it. The mouse is the view's already: a click without
dragging is told apart from marking text (`take_mouse`, `take_click`, issues
[571](571-approvals-and-questions-in-the-terminal.md),
[576](576-text-cannot-be-selected-with-the-mouse-in-the-full-screen-terminal.md)); what is
marked has to stay marked, and the scrollback must not jump when an entry above what is seen
changes its height. A description may run over several lines, as `run_command`'s does; the
head shows its first, the rest belongs to what is folded. Whether a key folds and unfolds as
well - for whoever does not use the mouse - is to be decided.

The line-wise view, which a screen reader follows, cannot fold what it has printed. It shows
the head as today's line, by the description, and no more; whether it is to print a command's
output in full is to be decided.

The result arrives after the call, and the call may never be answered (a turn given up): the
entry stands with its head alone until its result comes, joined by the call's id.

## Done when

- A tool call is shown in both views by its description where its arguments are shown, and as
  today where they are not.
- The full-screen view shows a call and its result as one entry, folded by default, unfolded
  and folded by a click on its head; unfolded, a `shell` call shows its command, its output,
  its exit code and whether output was dropped.
- Every presentation the terminal does not know is shown as `default`, `quiet` as in the web
  UI.
- [WEB-UI.md](../docs/WEB-UI.md) and [TOOLS.md](../docs/TOOLS.md) say where the description of
  a call comes from; the module docs of `caelumora-tui`'s `screen` say how it shows calls.

## History

2026-11-16: Opened at the maintainer's request: the description shown for an approval should
show at every call, the presentations of the web UI - a command's shell output above all -
should be supported, and the output folded and unfolded with the mouse, folded by default.

2026-11-16: Planned with the maintainer, in four steps, each reviewed and committed alone:

1.  A `ToolCall` is made in one place: `ToolCall::new(id, function_name, arguments)` where a model
    made it, `..make_tool_call(name)` in tests - so that a field of its own changes one place.
2.  The result of a shell tool is written by a type, `CommandResult` of
    `caelumora-tool-workspace` - named as `CommandState` and `CommandStatus` are, after the
    command run in the shell, not the shell -, which the terminal reads it back with;
    `SHELL_PRESENTATION` is made public.
3.  The description stands at the call (option A). `Tool::describe` answers
    `ToolResult<Option<String>>`, by default `None`: 32 of 48 tools did not describe a call and
    answered their tool's text for the model, which says nothing of the call - where a call has
    no description, its arguments are shown. `ToolExecutor::admit` stamps it, the one place that
    holds the tool; a call answered at once, and a partial message, has none.
    `ToolCall::withhold_arguments` withholds both, in `Message::published` and `names_only`.
    `ApprovalRequest`, `ToolCallStatus::WaitingApproval`, `ToolCallState::pending` and
    `Admission::Ask` lose their description and read the call's: approval requests are not
    redacted (CONFIDENTIALITY.md), so nobody sees more or less than before. Both views of the
    terminal head a call by its description in one line; the web UI shows it on the approval
    card - which showed only the arguments - and in its `default` presentation.
4.  The full-screen view shows a call and its result as one entry, folded by default, unfolded
    and folded by a click on its head, all of them at once by Ctrl-O. A result joins the newest
    call of its id that waits for one in its channel: call ids may repeat (APPROVALS.md). The
    terminal knows `shell` and a `default` that shows the arguments and the result whole; the
    file presentations are left to an issue of their own. The line-wise view prints no output
    in full.

2026-11-16: A step before step 3, from the review of step 1: `ToolMatcher::exact` and
`ToolMatcher::proposed` took a `ToolCall` but read only its tool and arguments, so that
`proposed_matcher` built a call no model made only to hand it on. They take what they read -
`exact(tool, arguments)`, `proposed(tool: &ToolDescription, arguments)` -, and
`proposed_matcher`, left a mere wrapper, is gone: the tools call `ToolMatcher::proposed`.

2026-11-16: Two more steps before step 3, from the review of the last: the tests describe a tool
from `make_tool_description` of `caelumora_domain::tool::testing`, naming only what they differ
in, as they make a call from `make_tool_call`; and `ToolMatcher::exact`, which outside its module
only tests call, is the module's own.

2026-11-16: One more step before step 3, from the review of the last: `ToolParameters::none()`
names the parameters of a tool that takes nothing, which `turn_end`, `list_skills` and
`make_tool_description` wrote out as an object of empty lists.
