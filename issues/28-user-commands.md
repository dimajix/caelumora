# 28 — User commands

**Status:** in progress
**Area:** User interaction

## The problem

Everything a user types into a channel becomes a message to the agents. There
is no way to address the *system* instead of the model — to ask what the
session can do, to wake an agent that has gone idle, or to have the context
compacted. What exists for that today is either a REST endpoint the web UI
calls implicitly (`send_message` in `caelumora-rest-server/src/routes/channels.rs`
wakes the channel on every message) or nothing at all.

The feature: a message that starts with a slash is a command, not a message.
`/<name>` followed by optional parameters is dispatched to a command
implementation instead of being appended to the conversation. The first
command is `/help`, which lists every command the user may run. Obvious next
ones are `/wakeup` to wake an agent and `/compact` to compress the context.

Commands are components: a `CommandProvider` contributes one or more commands,
exactly as a `ToolProvider` contributes tools, and both running a command and
seeing that it exists are permission-checked.

## Why it matters

- It gives the user a control surface that does not go through the model.
  Asking an agent to "please compact your context" costs a turn, lands in the
  history, and may or may not be obeyed; a command is deterministic.
- It is the natural place for operations that already exist in the runtime but
  have no user-facing entry point (`Channel::wakeup`, cancel, later retry from
  [08](08-retry-and-edit-past-answers.md) and compaction from
  [16](16-context-compression.md)), without adding a bespoke REST route and a
  bespoke UI button for each.
- Making commands components keeps it open-ended: a deployment adds a command
  the same way it adds a tool, without touching the channel or the frontends.

## Considerations

### Where a command is recognised

The slash has to be detected before the text becomes a message — otherwise it
is in the history and every agent has already seen it. That means at the point
where user input enters the channel (the user actor / `send_message`), not in
an agent's context providers. Doing it once there also means the TUI, the web
UI and any future frontend behave identically instead of each parsing on its
own.

Ambiguities that need a rule rather than a surprise:

- **A message that genuinely starts with a slash** — a path like
  `/etc/hosts is missing`. An escape (`//` sends a literal `/`) is the usual
  answer.
- **An unknown command** must be rejected with an error to the user, *not*
  silently forwarded as a message. Forwarding would turn every typo in a
  command into a prompt, and would make the set of commands a user may not see
  observable by trial and error.
- **Parameters.** Whitespace-split arguments are enough for `/wakeup <agent>`,
  but not for anything taking free text. Handing the provider the raw
  remainder of the line and letting it parse is simpler and more general than
  a shared argument grammar; a declared parameter schema (like `Tool`'s) is
  still worth having for `/help` and for completion in the UIs.

### `CommandProvider`, analogous to `ToolProvider`

Roughly: `commands() -> Vec<Command>` (name, description, parameter
description) and `async execute(name, args, context) -> CommandResult`, plus an
executor that aggregates the providers of a session the way
`DefaultToolExecutor` aggregates tool providers, and is where the permission
check happens.

Differences from tools that should not be papered over:

- **No `TurnContext`.** A tool runs inside an agent's cognitive turn; a command
  runs outside of it, often precisely *because* no turn is running (`/wakeup`).
  Its context is the session, the channel, the issuing user's
  `SecurityContext` and, where relevant, a target agent — not a turn's state.
- **Name collisions** between providers have to be detected at session start
  and reported, not resolved by registration order.
- **Commands are for users, tools for agents.** Letting the model invoke
  commands would re-open every question about approvals that tools already
  answer. Out of scope here; if an agent needs the capability, it gets a tool.

### Permissions: execute and show

Two new `ActionType` variants, both keyed by command name:

- **`ExecuteCommand`** — checked before `execute`. A denial is an error to the
  user, like an unknown command.
- **`ShowCommand`** — governs discovery. `/help` lists only commands the user
  may see, and completion in the UIs uses the same filter. Separating the two
  allows commands that are visible but not runnable ("ask an admin") as well as
  the reverse; whether the reverse is useful or just confusing is worth
  deciding when the first real policy is written.

With `DummySecurityManager` both are always granted, so this is inert until
[23](23-acls-in-security-attributes.md) / [24](24-opa-security-manager.md)
exist — but the check sites must be there from the start, as for tools. In a
multi-user channel the question from [14](14-approval-privilege.md) recurs:
may any participant `/wakeup` or `/compact` a shared agent? It should be
answered by the same rule, not a second one.

### Where the output goes

`/help` produces text for the issuing user and nobody else; it is not part of
the conversation and must not enter `MessageHistory`, or it ends up in the
next model context. Other commands (`/compact`) have an effect everyone in the
channel should know about. So a result is either private to the issuer or a
channel event — and both frontends need a way to render it. That is a REST API
change and has to stay compatible with `caelumora-web-ui`. Whether an executed
command is recorded belongs with [05](05-postgres-event-recorder.md): it is
exactly the kind of action an audit trail should contain.

### The first commands

- **`/help`** — lists the visible commands with description and parameters;
  `/help <name>` shows one. Needs nothing beyond the provider mechanism, which
  makes it the right first command to build the mechanism against.
- **`/wakeup [agent]`** — wakes one agent, or all agents in the channel when
  none is named. `Channel::wakeup` and `ActorEvent::WakeupRequested` already
  exist; this is mostly wiring.
- **`/compact`** — triggers compression of the working context. Blocked by
  [16](16-context-compression.md): the command is the manual trigger for its
  mechanism, not a mechanism of its own, and it inherits 16's constraint that
  the stored history is not mutated.

## Done when

- A slash-prefixed message is dispatched as a command in one place, for every
  frontend, and never enters the message history; `//` escapes a literal slash.
- Unknown and denied commands produce an error to the issuing user and are not
  forwarded as messages.
- `CommandProvider` exists as a component facet, registered through the
  `ComponentRegistry` like `ToolProvider`, with name collisions reported.
- `ExecuteCommand` and `ShowCommand` exist in `ActionType` and are checked on
  execution and on listing respectively.
- `/help` and `/wakeup` are implemented as command providers; `/compact` is,
  once [16](16-context-compression.md) provides something to trigger.
- TUI and web UI render command output, private or channel-wide as the command
  declares.

## History

- **2026-09-21** — Issue created. Commands modelled as components via a
  `CommandProvider` analogous to `ToolProvider`, with separate execute and show
  permissions; `/help` first, `/wakeup` and `/compact` as the next candidates.
- **2026-09-23** — [30](30-user-actor-for-authenticated-user.md) added
  `ActionType::WakeChannel`, checked in `ChannelLoop::wakeup`. `/wakeup` is
  its first caller; until then the permission exists but nothing asks for it.
  A command's own permissions ("execute command", "show command") are still
  this issue's to define.
- **2026-09-24** — `/help` and `/wakeup` implemented; `/compact` stays open
  until [16](16-context-compression.md) provides something to trigger. Three
  decisions depart from or sharpen the text above:
  - **Recognised by the frontend, run by the user's actor.** The frontends
    decide whether a line is a command (`caelumora_model::command::parse_input`,
    mirrored in the web UI's `command.ts`) and send it to its own REST
    resource (`GET`/`POST .../commands`); the message API stays plain text.
    That gives up "recognised in one place" for the lexical rule only —
    leading `/`, `//` as escape, a few lines per frontend. What a command
    means, who may run it and what it answers is still decided in one place.
    The command is run by `UserActor::execute_command`: it always happens on
    behalf of a particular person, and the actor is what acts for that person,
    as for messages and approvals — so a channel-wide answer is announced by
    the actor itself, and only people can run commands. A first version made
    it a channel operation, `ChannelLoop::execute_command(user, ...)`; that
    needed the channel to make the user's actor announce the result on its
    behalf, which showed the method was on the wrong object.
    To reach its channel and the session's commands the actor needs a way
    back: a `ChannelRuntime` now sits between `RuntimeManager` and
    `ChannelLoop`, is the only source of `ActorRuntime`s, and gives each actor
    the `CommandSet` and a `Weak` handle to its channel.
  - **Two configuration places.** `AppConfig::commands` for the global set
    (default `/help` and `/wakeup`, an empty list switches them off) and
    `SessionTemplate::commands` for commands that belong to what a session
    is for — an integration, or the prompts of an MCP server offered as
    commands. Per-channel configuration was considered and rejected: the
    scenarios for it were either about *who* may run a command, which is
    `ShowCommand`/`ExecuteCommand`, or could be decided by the command at
    run time (`/wakeup` in a channel without agents says so). Both sets are
    built once per session; a name provided twice keeps the session from
    starting.
  - **Output.** A private answer goes back to the caller only; a channel-wide
    one is announced by the issuer's actor as `ActorEvent::CommandExecuted`,
    which moderators route to users only and which is never stored. It is
    returned to the caller as well, because the channel does not route an
    envelope back to its sender — the TUI would otherwise not see its own
    `/wakeup`.

  The TUI keeps its own `/approve` and `/deny`, which answer an approval by
  id and are not channel commands; [14](14-approval-privilege.md) is where
  approving may become one. Whether an executed command is recorded is still
  [05](05-postgres-event-recorder.md)'s question.
- **2026-09-24** — Aligned with tools: `CommandSet` is now
  `CommandExecutor`, the counterpart of `ToolExecutor`, and like it receives
  ready providers instead of building them from a configuration; building
  moved to `RuntimeManager::new_command_executor`, which owns the global and
  the session's configuration. It stays a struct - `ToolExecutor` is a trait,
  but there is no second implementation of the command side to justify one.
  Commands already report two components providing the same name; tools do
  not yet, which is [37](37-tool-name-collisions.md).
