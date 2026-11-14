# 578 — The full-screen terminal is drawn in few colours

**Status:** open
**Type:** feature
**Priority:** low
**Area:** User interaction
**Summary:** `caelumora-tui`'s full-screen view draws nearly everything in the terminal's own colour - senders bold, the rest dimmed or plain, colour only for mentions, approvals and errors -, so who said what, the user's own lines, tool calls and the status line are hard to tell apart at a glance.

## The problem

`styled.rs` (`caelumora-tui/src/screen/full/`, issue
[569](569-the-terminal-mixes-its-output-into-the-line-typed.md)) styles what is printed: a sender
bold, a step of a plan, notices, tool calls and results dimmed, mentions cyan, an approval's and
questions' headers yellow, errors red, the spinner cyan. Everything else - the messages of every
agent and person alike, the user's own lines, the rule, the status line, the line typed - is in
the terminal's foreground colour.

## Considerations

*   **What gets colour.** A colour per participant, kept for the session, so that two agents' lines
    differ; the user's own lines in one of their own; tool calls and results apart from what was
    said; the status line and the rule on a background of their own; the prompt.
*   **Which colours.** The terminal's sixteen, which follow its theme - light or dark -, rather than
    fixed ones that may vanish on one of them. `NO_COLOR` turns colour off, as other programs do.
*   **Where it is kept.** Within `styled.rs`, the one place that styles; a theme of its own only
    where a second set is asked for.
*   `line`, meant for screen readers, keeps writing without colour (issue 569, step 6).

## Done when

- The full-screen view sets apart by colour who said what, the user's own lines, tools, and its
  status line, in colours that hold on light and dark terminals.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
