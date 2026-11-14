# 579 — Commands and mentions are not completed in the terminal

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui` completes nothing that is typed: commands - the channel's, the agents' and the terminal's own - their arguments, and the handles of participants after `@` all have to be typed out, though the terminal knows every one of them.

## The problem

The terminal knows what could be typed: the commands the user may see
(`UserActor::visible_commands`) and its own (`TerminalCommand`, issue
[570](570-the-terminal-s-commands-and-its-sessions.md)), the participants of the channel with their
handles (`Followed` keeps them to highlight mentions), the channels of the session, the sessions of
the directory and the templates. Neither way of drawing
offers any of it: `line` reads with rustyline without a helper, `full` edits the line itself
(`InputLine`), and Tab does nothing in either.

## Considerations

*   **What is completed.** After `/` the names and short forms of the commands, word by word as they
    are grouped; after a command its arguments where they are of a kind the terminal knows -
    channels for `/channel switch`, ids for `/session resume`, templates for `/session new`, agents
    for `/compact @agent`; after `@` the handles of the channel's participants and `@all`.
*   **One source, two ways of drawing.** What can follow what is typed is worked out once, beside
    the commands (`commands.rs`); `line` offers it through rustyline's `Completer` (and a `Hinter`
    for the rest of a single match), `full` through Tab and a short list above the line typed.
*   **Fresh enough.** Commands and participants change while the program runs - a component added, a
    participant joined -, so what is offered is read as it is completed, or kept up by what the
    channels show.

## Done when

- Tab completes commands, their arguments as decided, and mentions, in both ways of drawing.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
