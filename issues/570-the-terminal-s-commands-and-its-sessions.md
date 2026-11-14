# 570 — The terminal's commands follow no rule of commands, and reach no other session

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The commands `caelumora-tui` carries out itself - `/switch`, `/approve`, `/deny`, `/answer`, `/decline`, `/retry` - are named by no rule of commands and unknown to `/help`, the program ends on the bare words `exit` and `quit` - which can then never be said -, and once running it can neither start a session, nor continue, list or delete one.

## The problem

*   **A second command system.** `handle_line` (`caelumora-tui/src/main.rs`) recognises its
    commands by hand, beside the channel's `CommandSet`: their names follow none of the rules of
    [COMMANDS.md](../docs/COMMANDS.md#names) - groups, short forms, no name the beginning of
    another -, and `/help`, which the channel answers, does not list them.
*   **Ending by a word.** `exit` and `quit`, typed alone, end the program; there is no `/exit` or
    `/quit`, and the two words cannot be sent as a message.
*   **One session per run.** The session is chosen at the start (`--continue`, `--resume`,
    `--sessions`, or a new one from `--template`); to start another, continue or delete one, the
    program has to be ended and started again.

## Considerations

Decided (2026-11-14): the terminal's commands follow the rules of COMMANDS.md - each a
`CommandName`, grouped by what it acts on, with short forms for what is typed often, no name or
short form the beginning of another -, are listed by `/help` after the channel's, and are tried
before a line goes to the channel. Proposed set:

| Command | Short form | What it does |
|---|---|---|
| `/exit` | `/quit` | Ends the program, the session shut down in order. |
| `/channel switch <channel>` | `/switch` | Talks in another channel the user takes part in. |
| `/session list` | - | Lists the sessions of this directory, the one spoken in last first. |
| `/session new [template]` | `/new` | Starts a session from a template, `default` unless named, and talks in it. |
| `/session resume <id>` | `/resume` | Continues the session of this directory whose id begins so. |
| `/session delete <id>` | - | Deletes the session of this directory whose id begins so; not the one talked in. |

The approvals and questions (`/approve`, `/deny`, `/answer`, `/decline`) get their names with
issue [571](571-approvals-and-questions-in-the-terminal.md), `/retry` its own here.

*   A session is changed by opening the other first, then stopping the one left
    (`SessionManager::stop`) and following the channels of the new one.
*   Without a storage `here`, sessions are not listed, resumed or deleted by directory, as the
    options of the program refuse it today.
*   A session deleted can be restored, so deleting asks no confirmation.
*   `/channel switch` stands beside the channel's own group `channel`; no name of the terminal may
    begin or be the beginning of one of the channel's - to be checked against the commands the
    channel lists (`ChannelView::commands`).

## Done when

- The terminal's commands are named by the rules of COMMANDS.md and listed by `/help`.
- `/exit` and `/quit` end the program; `exit` and `quit` are messages.
- Sessions are listed, started, resumed and deleted while the program runs.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal, with the naming by COMMANDS.md
decided.

**2026-11-14** - Decided with the maintainer before the work began, in three steps: a restructuring
without a change of behaviour (`Command::help_line` in the domain for every list of commands, the
sessions of the directory as `Sessions`, the lines typed handled in a module of their own); the
terminal's commands named by COMMANDS.md and listed by `/help`; the commands on sessions. The
approvals and questions are named here already - `/approval approve` (`/approve`), `/approval deny`
(`/deny`), `/question answer` (`/answer`), `/question decline` (`/decline`) -, their behaviour left
to issue [571](571-approvals-and-questions-in-the-terminal.md), so that one set of commands
replaces the second system at once; `/retry` becomes the short form of `/agent retry`; `/exit`
stands without a group, as `/help` does, since ending the program acts on no entity of a channel.

**2026-11-14** - Step 1 done: `Command::help_line`, which the kernel's `/help` writes its lists
with; `Sessions`, which lists, starts, continues and resumes the sessions of the directory for the
options of the program; the lines typed handled in `commands.rs`. What tells that no session was
started yet is printed by the screen now, not past it.
