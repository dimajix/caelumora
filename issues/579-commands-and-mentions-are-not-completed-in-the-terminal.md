# 579 — Commands and mentions are not completed in the terminal

**Status:** in progress
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

**2026-11-15** - From [583](583-a-command-has-no-object-of-its-own.md): a command's parameters are
described (`CommandParameter`: name, whether required, keyword), but not what they take; the kind
of a parameter - a participant, an agent, a channel, a personality, an id, a text - comes with
this issue, the first to read it, and completing an argument is done from it.

**2026-11-15** - Decided with the maintainer before the work began: a parameter takes a kind
(`ParameterKind`) - text, a participant or an agent by its handle, a channel, a personality, a
command, a session or a template -, set for each parameter by what its command's parser takes;
what completes the line is worked out once (`Typeable::complete`) from a snapshot the talk keeps
fresh - at its beginning, after every line, at a switch of channel and where participants or
channels change - and each way of drawing is offered it; only the first parameter is completed, the
rest of the line being its argument; `line` lists the candidates at a second Tab and shows no hint,
for a screen reader; in three steps: the kind, the source of completions with `line`, and `full`.

**2026-11-15** - Step 1 done: `ParameterKind` in `caelumora-domain`, a field of `CommandParameter`
left out where it is text, and the web UI's type of it; the parameters of the commands that take
someone, a channel, a personality, a command, a session or a template say so.

**2026-11-15** - Decided with the maintainer after step 1's review: an agent of the session, as
`/agent list` shows it (`SessionAgent`), is a kind of its own beside an agent taking part in the
channel (`Agent`): `/agent delete` acts on the agent itself and finds it among the session's agents
as `/agent list` lists them, so that what is offered, listed and deleted is one circle;
`/agent wakeup`, `/agent usage` and `/channel leave` act on an agent in the channel. Each kind names
what its commands search among, without a scope of its own: no second kind is searched in several
reaches yet.

