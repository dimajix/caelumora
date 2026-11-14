# 569 — The terminal mixes what a channel shows into the line typed, and hides what agents do

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui` prints what a channel shows into the line being typed after `>>`, shows nothing while an agent works - neither that it does, nor its tool calls and their results -, and offers one way of drawing alone, which suits neither a screen reader nor whoever wants a full-screen view.

## The problem

What the maintainer found using the terminal:

1. **No feedback.** After a line is sent, the next prompt appears at once; nothing tells that an
   agent began to answer, works, or runs tools, until its answer is complete.
2. **Output in the line typed.** rustyline reads in a thread of its own with the prompt `>> `,
   while `run_repl` prints what the channel shows with `println!("\r{line}")` into that line;
   `handle_line`, `answer_line`, `acting`, `send_line` and `print_error` print the same way
   (`caelumora-tui/src/main.rs`).
3. **Tool calls are invisible.** They reach the user's actor - as messages carrying `tool_calls`
   and `tool_results`, redacted by their sender (`Message::published`) -, but `said`
   (`caelumora-tui/src/followed.rs`) answers `None` for a message without words and attachments,
   so nothing is printed of them. `StateChanged`, `ResponseStarted` and the streamed
   `PartialMessageGenerated` reach it as well and are passed over.

The kernel needs no change: everything the web UI shows of an agent at work reaches the terminal
already.

## Considerations

Three ways of drawing were weighed:

*   **(A) rustyline's `ExternalPrinter`**, which prints above the prompt and keeps the line being
    typed: fixes the second point alone.
*   **(B) An inline region at the bottom**, redrawn below the terminal's own scrollback. Redrawn
    status lines, a spinner and streamed text are read again and again by a screen reader, so it
    is not the accessible way it seems; and it serves a full-screen view less well than (C).
*   **(C) A full-screen view** in the alternate screen: the conversation above, scrolled by the
    program alone, the status and the line typed below - as Claude Code does.

Decided (2026-11-14): the terminal gets two ways of drawing, chosen by `--screen line|full`, `full`
by default, `line` wherever the terminal is no TTY:

*   **`line`** is linear and meant for screen readers: every line written once, no animation, no
    cursor movement, prefixes in words rather than colours and symbols; input through rustyline,
    whose `ExternalPrinter` prints above the prompt. What agents do is told by a line at each
    change - an answer begun, tools run -; streamed text is not shown, only the complete message.
*   **`full`** is (C), with `ratatui` on `crossterm`: the conversation scrolled with Page Up/Down,
    Shift+arrows and the mouse wheel, following its end unless scrolled up; at its end the answers
    under way with the text they streamed so far, growing in place; below, what is asked of the
    user, the status line, and the line typed.

Both share everything but drawing and reading keys. The cut: the shared code says *what* is shown,
each way of drawing *how*:

*   **What is printed** is an entry with a meaning - a message with its sender and mentions, a tool
    call, a tool result with its status, a notice, an error, an approval, questions -, not a styled
    line, so that `line` can say in words what `full` shows by colour and indentation.
*   **The answers under way** - agent, turn, state, since when, the text streamed so far - are kept
    by the channel followed, from `ResponseStarted`, `StateChanged`, the partial messages,
    `TurnFinished` and `TurnGivenUp`, as the web UI keeps its `answers`; they take the place of
    today's `under_way`, which `/retry` reads. Partial messages may be lost; the complete message
    is what counts. `full` draws them, `line` prints their changes, which the shared code works
    out.
*   **What is asked of the user** - an approval, questions - and the commands are shared whole
    (issues [570](570-the-terminal-s-commands-and-its-sessions.md),
    [571](571-approvals-and-questions-in-the-terminal.md)).
*   **A screen** is a trait of the terminal with an implementation per way of drawing: it shows
    what is printed, the answers under way and what is asked, and reads the next input.

Of tool calls, both show the tool, its arguments in one shortened line - or that they are not
shown, where they were withheld -, and how the call ended: its result shortened, or that it was
denied, forbidden, timed out, failed or withheld. A call presented `quiet` is left out where it
succeeded, as on the web UI.

What `full` costs: to scroll with the mouse wheel the program captures the mouse, so text is
selected with Shift held in most terminals; and what was shown is gone from the screen once the
program ends. So a channel the terminal opens, continues or switches to shows its history so far
(`ChannelView::history_page`), in both ways of drawing.

Steps, each reviewed on its own:

1.  Restructuring without a change of behaviour: entries in place of strings, one place that
    prints, and the screen as a trait with `line` drawing as today.
2.  `line` as decided: `ExternalPrinter`, tool calls and results, the changes of the answers under
    way, the line typed shown as sent.
3.  `full`: the conversation scrolled, the status line, the text streamed.
4.  The history of a channel as it is opened, in both.

## Done when

- Nothing the channel shows lands in the line being typed, in either way of drawing.
- Both ways of drawing show that an agent works, its tool calls and their results; `full` shows
  the text it streams.
- `--screen line|full` chooses the way of drawing, `full` by default, `line` without a TTY.
- A channel opened shows its history so far.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal; the two ways of drawing and the
cut between them decided before the work began.

**2026-11-14** - Step 1 done: what the terminal prints is `Printed`, by its meaning, and only a
`Screen` prints it; `LineScreen` writes as before.

**2026-11-14** - Step 2 done: `line` writes above the line being typed through rustyline's
`ExternalPrinter`, tells an answer as it begins, and shows tool calls and how they ended - a call
presented `quiet` only where it did not succeed. The answers under way are `UnderWay`, kept by the
channel followed. The review of the step found that `main` returns without shutting the session
down where the channels of a session it opened cannot be followed (`Channels::new`); planned as
step 5: whatever fails once a session is opened ends through its shutdown.

**2026-11-14** - Step 3 done: `full`, the default of `--screen`, draws the whole window with
`ratatui` - what was printed scrolled by the user alone, staying where it is as text streams, the
text the answers under way streamed at its end, a status line of who answers and where they stand,
and the line typed. `line` is taken wherever input or output is no terminal. The answers under way
are read anew as the terminal catches up. The words both ways of drawing share are theirs
(`screen/words.rs`).

**2026-11-14** - Step 4 done: as the user begins to talk in a channel - at the start, continued,
or switched to -, the terminal prints what was said among the last 50 items of its history, in
both ways of drawing; a message the channel shows after is not printed twice. Who joined, commands
and turns given up are told live alone: they carry no id to tell them from what the channel shows
after. Planned as step 6, on the maintainer's word: `line` tells in words what it marks by symbols
today - `helper> `, `--- … ---`, `[working]`, `[channel]` - and drops the colour of mentions.

**2026-11-14** - Step 5 done: what the user does once a session is opened - its channels followed,
the talk in them - is `talk`, after which the program shuts the session down however it ended,
also where the user takes part in no channel of it. The signals that end the program are caught
from before a session is opened, not only once the talk began, so that one sent as the program
starts ends it through the shutdown too. Planned as step 7, on the maintainer's word,
from the review of step 4: the channels followed are found by their id in one place, not by their
position in `Channels::take_in_shown` and `Channels::unfollow_if_ended` each.

**2026-11-14** - Step 6 done: `line` tells in words what it marked by symbols - `helper: `,
`helper, working: `, `In side: `, `Error: `, notices without dashes - and writes mentions without
colour. Left as they are: the choices of a question (`1) label`), which issue
[571](571-approvals-and-questions-in-the-terminal.md) makes choices of its own, the cut mark of a
result shortened, and rustyline's prompt `>> `.

**2026-11-14** - Step 7 done, without a change of behaviour: the channels followed are found by
their id in one place (`Channels::followed`, `Channels::followed_mut`), no longer by their position
in `take_in_shown` and `unfollow_if_ended` each. Every step of the issue is done; what the full
view shows of other channels is issue
[572](572-the-full-screen-terminal-tells-nothing-of-other-channels-at-work.md).
