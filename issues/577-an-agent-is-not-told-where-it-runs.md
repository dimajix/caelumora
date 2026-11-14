# 577 — An agent is not told where it runs: its session's and channel's ids, the program's log

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Context and knowledge
**Summary:** No prompt part tells an agent the ids of its session and channel, nor - in the terminal program - where the program writes its log, so an agent asked to look into what went wrong cannot search the log for its own session with the terminal it is given.

## The problem

The ids of a session and a channel are what the log carries (`span_id`, `session`, `channel` on
every envelope's span, [CODING-GUIDELINES.md](../docs/CODING-GUIDELINES.md#correlation)), and what
`--sessions` and `/session resume` name a session by. No prompt part gives them to an agent. And
the terminal program writes its log to `tui.log` of its state directory (`Home::log`), which an
agent with a workspace in the directory the program was started in could read with its terminal -
if it knew where it is.

## Considerations

*   **A prompt provider of its own.** The ids are the kernel's to tell any agent; where the log
    lies only the program knows - the terminal's, not the REST server's, whose log goes to stderr.
    So a component of its own: a library of the terminal registering a kind (say
    `program_prompt_provider`) that a personality wires in, told the log's path by the program as it
    builds the kernel; the ids perhaps in `channel_briefing` beside the names (issue
    [575](575-an-agent-is-not-told-the-names-of-its-session-and-channel.md)).
*   **What the log holds.** Not only the agent's session: every session, user and channel of the
    program, their ids, the errors with their causes - message text at `trace` too, where the log
    is that verbose. Telling an agent where it lies invites it to read what its channel's
    classification may not admit ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md)), and what it
    reads through its terminal is no knowledge recorded as such. Perhaps the part is offered only
    where one person owns everything the program runs - the terminal's case - and says what the log
    holds; or a tool reads the log lines of the agent's own session alone.
*   **Its workspace.** The log lies outside the directory the program was started in; whether the
    agent's terminal reaches it depends on the storage (`local` runs as the user, Open Terminal in
    its container does not).

## Done when

- An agent is told the ids of its session and channel, and in the terminal program, as decided,
  where the log lies.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
