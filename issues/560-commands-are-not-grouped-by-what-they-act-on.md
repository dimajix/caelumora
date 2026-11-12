# 560 — The commands are not grouped by what they act on

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** The commands of a channel are single words, one per operation, so that every new operation needs a word of its own that says neither what it acts on nor which commands belong with it, and beyond `/add` and `/create-agent` nobody can manage the channels, participants and agents of a session by command.

## The problem

Issue [555](555-no-command-brings-an-agent-into-a-channel.md) gave the channel `/add` and
`/create-agent`. With them the built-in commands are `/help`, `/wakeup`, `/nudge`, `/add` and
`/create-agent`, and of the agents `/skill`, `/skills` and `/compact`
([COMMANDS.md](../docs/COMMANDS.md#built-in-commands)). Each is one word, and each new operation
needs another: `/add` does not say whom it adds or to what, and `/create-agent` is a verb and a
noun run together. Listing who takes part, removing someone, leaving a channel, creating, renaming,
describing or deleting a channel, listing or deleting agents: none of it can be done by command,
only through the REST API and the web UI's dialogs.

## What is wanted

The maintainer proposes to group the commands by the kind of entity they act on, as many command
line tools do (`gh pr list`, `docker network create`):

- `participant list`, `participant add <participant>`, `participant remove <participant>`
- `agent list`, `agent create <...>`, `agent delete <agent>`
- `channel list`, `channel join <channel>`, `channel leave <channel>`,
  `channel create <channel>`, `channel delete <channel>`, `channel switch <channel>` (for the
  terminal), `channel describe <channel> <description>`, `channel rename <channel> <new-name>`

Short commands for what is done often are to stay, as a second name beside the grouped one.

This takes up the names 555 decided - `/add` becomes `participant add`, `/create-agent` becomes
`agent create` - and the removal by command that 555 left out (`/remove`). Starting a sub-agent by
command (issue [558](558-no-agent-hands-a-task-to-a-sub-agent.md)) would belong to the group
`agent`.

## Why it matters

A command line of a chat is typed while talking; what can be done there and what cannot decides
whether someone leaves the conversation for a dialog. A group gives every kind the same verbs
(`list`, `create`, `delete`, `rename`), lets `/help channel` show what belongs together, lets
completion go word by word, and keeps names from colliding as the commands grow.

## What exists

- **Commands.** `Command` (`caelumora_domain::command`) has a `name: String`; `parse_input` takes
  the name to the first whitespace and hands the rest of the line to the command unparsed. A
  `CommandSet` (`caelumora_api::command`) finds a command by its name and refuses two of one name
  when it is built, once per session, in the name of its owner. A component may contribute several
  commands, as `skills` contributes `/skill` and `/skills`. The `CommandExecutor` asks
  `ShowCommand` and `ExecuteCommand` for each command; the command is given the issuer in
  `CommandContext`.
- **Where the name goes.** As a `String` into `CommandReply` and its REST response, the events of
  a command (`actor/event.rs`), the description of the actions
  (`caelumora-security/src/action.rs`), and into what is stored: the history records what was
  carried out - by the session only `/wakeup`, besides the commands of the agents - and refused
  (`history/entry.rs`), and an agent's command turn keeps its command (`CommandRequest`,
  `turn.rs`).
- **Component kinds.** The built-in commands are components of the kinds `help`, `wakeup`, `nudge`,
  `add` and `create-agent`, the default of `AppConfig::commands`; a session keeps the commands of
  its template in `SessionTemplate::commands`. A kind is not a command's name.
- **The terminal's own commands.** `caelumora-tui` handles `/approve`, `/deny`, `/answer`,
  `/decline` and `/retry` itself and sends every other command to the channel. It runs in one
  channel (`run_repl`).
- **The operations.** `AgentManager::create` and `delete`; `SessionLoop::add_participant` and
  `join`; `ChannelLoop::add_participant`, `join` and `remove_participant`;
  `SessionLoop::create_channel` and `delete_channel`; `ChannelController::update_name` and
  `update_description`. Removing someone from the session has no operation.

## Considerations

### Short forms: in the command or in a configuration

- **In the command.** A `Command` declares its other names (`aliases`); `participant add` declares
  `add`. The `CommandSet` finds the entry by each of them and refuses an alias that collides with a
  name like two equal names. An alias is the same entry: the same rights, the same description, and
  `/help` shows it with the full name.
- **In a configuration** - of the user, as git and gh have it, or of the server or the session. An
  alias is then a replacement of text before the command is looked up: a second mechanism with
  rules for collisions of its own, which has to know the commands. There is no place for settings
  of a user today.

AGENTS.md asks for fewer ways of doing the same thing; a short form is deliberately a second way,
for what is done often in a chat, and is named as that exception.

### Groups: a tree of commands or names of several words

- **A tree.** `Command` has a list of subcommands. The group has a description of its own, but it
  belongs to one component, so no other component can add `channel export` or `agent compact`, and
  what `ShowCommand` means for a group that does nothing itself has to be decided.
- **Names of several words.** The name of a command is its path of words; a group is its first
  word, nothing of its own. Every component may contribute to every group; every command of a
  group is an entry with rights of its own; `/help channel` lists the entries beginning with
  `channel`; a short form is another name.

As a path, the name is a type of its own, `CommandName`: it is checked when made - no empty path,
no empty word, no word with whitespace - and it answers whether it begins with a group, rather
than every caller splitting a string. Written out and stored it is the string of its words
(`"channel list"`), so the history, the events and the REST types stay readable.

If no name or alias is the beginning of another, the `CommandSet` finds at most one entry for a
line, without a rule that the longest wins. `parse_input` then only takes off the slash, and the
`CommandSet` splits the line into the command and its arguments by its names. What is recorded is
the command's name, not the short form typed.

### Who sees what

The issuer could be given to `ProvidesCommands::commands`, so that a component answers which of its
commands someone may use. But `commands` is asked once per session, to build the `CommandSet` in
which two of one name are found at start, and who may see and run a command is decided by the
`CommandExecutor` asking the policy, as the `ToolExecutor` does for tools (issues 127, 280, 516).
With a name of several words every command of a group is its own `Command`, asked for on its own.

That `/help` hides a command from whoever may see but not run what it does - `channel delete` from
whoever may not delete the channel - was left open in 555, since `ShowCommand` carries only the
channel's attributes. A command could declare the action it needs, for the executor to ask as well.

### The commands of an agent

`/help` stays without a group. Where `/wakeup` and `/nudge` go - `agent wakeup`, `participant
nudge` - is open. `/skill`, `/skills` and `/compact` are carried out by an agent and found in its
own `CommandSet`; as `agent compact @bob` the group `agent` would hold commands of the session and
commands of an agent, and today a name in both keeps the agent from starting.

### `channel switch`

It changes what a frontend shows, not the session: a command of the frontend, like the terminal's
`/approve`. The terminal runs in one channel; switching needs it to hold several, which is a
feature of its own. `/help` of the server does not know commands of a frontend, so the group
`channel` would be in part the server's and in part the terminal's. Whether it is part of this
issue or one of the terminal is open.

### Names and arguments

- `describe` means "show the details" in many command line tools (kubectl). `channel describe
  <channel>` could show a channel, and the description be set by another word - `channel topic`,
  as in IRC and Slack, or `channel set-description`.
- Two names with spaces are separated by the last `as` (issue 555): `/channel rename Old as New`.
  A description may contain "as" itself, so that rule does not carry it. Commands on a channel
  could act on the channel they are issued in when no channel is named: `/channel rename New`.

### What the commands do

- Whether `participant remove` removes from the channel alone or from the session too; the latter
  has no operation.
- How `channel join` - `join` by one's own act, where a `Join` entry lets one - differs from
  `participant add` with oneself.
- Which agents `agent list` lists: the session's, or all that can be reached from the channel, as
  `participant add` finds them.
- Whether `agent delete` deletes an agent taking part in channels.
- Whether `channel leave` asks for more than removing oneself.

### Stored data

`/add` and `/create-agent` answer privately and are not recorded, so renaming them changes nothing
stored. Renaming `/wakeup` or a command of an agent does: the history and the command turns keep
their names (`wakeup`, `compact`, `skill`). Renaming the component kinds is not asked for; were it
done, sessions would keep the old kinds in their commands. No code is kept for old stored data;
what has to be deleted is said with the change.

### Steps

As restructuring before extending: first the groups as a mechanism, with the existing commands
renamed (`/add` to `participant add`, `/create-agent` to `agent create`) and their short forms,
without a new operation; then the new commands, a group at a time; `channel switch` last, or in an
issue of the terminal.

## Done when

- A command's name is a `CommandName` of one or more words, and a command may have short forms;
  the `CommandSet` refuses a name or short form that is the beginning of another.
- The existing commands are in their groups, the often used ones with a short form.
- The commands of the groups above exist but `channel switch`, each asking nothing its operation
  does not ask; the security suite covers them.
- `/help` lists them by group, and `/help <group>` the commands of a group.
- COMMANDS.md describes groups, names and short forms.
- The decisions above are recorded here.

## History

- **2026-11-12** — Issue created at the maintainer's request, building on 555, with the list of
  commands above. Decided with the maintainer:
  1. Commands are grouped by the kind of entity they act on; short forms of the often used ones
     stay beside them.
  2. A short form is declared by its command (`aliases`), not configured; aliases of a user are
     left for when someone asks for them.
  3. The name of a command is a `CommandName`, its path of words, checked when made and written as
     the string of its words; no name or alias is the beginning of another; the name is recorded,
     not the short form typed.
  4. Who may see and run a command stays the `CommandExecutor`'s question to the policy;
     `ProvidesCommands` is not given the issuer, and every command of a group is asked for on its
     own.

  Open: the commands of an agent and `/wakeup`, `/nudge`; whether a command declares the action it
  needs for `/help`; `channel switch`; the word for setting a description; arguments with two
  names and the channel as default; what each command does, as listed above.
- **2026-11-12** — Decided with the maintainer: `channel switch` is a feature of the terminal and
  went into issue [561](561-the-terminal-shows-one-channel-of-a-session.md); it is no part of
  this issue.
- **2026-11-12** — Decided with the maintainer, the plan of the first step, in two parts:
  1. 1a, without a change of behaviour: `CommandName`, a value of the domain built as `Name` is,
     its words of `a-z`, `0-9` and `-`; `CommandName::fixed` for the names the code gives, checked
     by `CommandSet::new`. `Command` gets `aliases`. `parse_input` takes off the slash alone, and
     the `CommandSet` finds the command a line begins with (`find`) and refuses two that collide;
     an agent's command must not collide with one of the session. The executor, the user's actor,
     the REST request (`line` instead of `name` and `args`), the web UI and the terminal pass the
     line. Events, the history and REST keep the name as a string, the name and not the short form
     typed.
  2. 1b: `/add` becomes `participant add` and `/nudge` `participant nudge`, `/wakeup` becomes
     `agent wakeup` and `/create-agent` `agent create`; `add`, `nudge` and `wakeup` stay as short
     forms, `create-agent` does not. `/help` stays without a group. The commands of an agent stay
     as they are in this issue. A component per group (`participant`, `agent`, later `channel`)
     contributes its commands.
- **2026-11-12** — Step 1a done, as planned but for one thing: the `CommandExecutor` finds the
  command a line begins with (`find`, its name and the arguments) and runs it by its name
  (`execute`), rather than running the line and answering the name, since the user's actor needs
  the arguments as well for what it announces. The REST request carries `line`. One change of
  behaviour comes with it: `/ help`, a blank after the slash, runs `/help` rather than being refused
  as an empty command, since the line is trimmed before the command is sought.
- **2026-11-12** — Step 1b done: `/participant add` (`/add`), `/participant nudge` (`/nudge`),
  `/agent create` and `/agent wakeup` (`/wakeup`), contributed by the components of the kinds
  `participant` and `agent` (`command/participant/`, `command/agent/`), which replace the kinds
  `add`, `nudge`, `create-agent` and `wakeup`. `/help` shows the short forms, describes a command
  by either, and lists the commands of a group (`/help agent`). The web UI's completion takes the
  words of a name typed after the caret when a command is chosen. Renaming the kinds, which the
  section on stored data above left open, comes with it: a configuration whose `commands` name the
  old kinds no longer builds and is to be changed, and a stored session or template whose
  `commands` name them is to be deleted or changed. The history keeps `wakeup` as the name of a
  wakeup carried out before; it stays readable.
- **2026-11-12** — Decided with the maintainer, for the group `participant`:
  1. `participant remove` removes from the channel alone (`ChannelLoop::remove_participant`); a
     removal from the session has no operation and is left to an issue of its own.
  2. Removing oneself asks `UpdateChannel` as removing anyone does; leaving without that right is
     for `channel leave`, which needs a right of its own.
  3. Neither `participant list` nor `participant remove` has a short form.
  4. `participant list` shows who takes part now: name, handle, user or agent, an agent's
     personality; no state, no former participants.

  Step 2 done: `/participant list` (through `ChannelView::participants`, in the issuer's name) and
  `/participant remove`; the security suite covers what both ask.
- **2026-11-12** — Decided with the maintainer on the review of step 2: `participant remove` seeks
  whom it removes among those `participant list` lists, as the issuer reads the channel, so that it
  also removes a participant that cannot run; and the commands reach the channel's session and the
  issuer's views of it in one place. Done in two steps:
  1. Without a change of behaviour: `CommandContext::session_loop`, the session with the views in
     the issuer's name, for `participant add`, `participant list` and `agent create`; what they
     do in the issuer's name is asked in the views' context.
  2. The rule by which a text names a participant is `channel::named`, over the participants it is
     given; `named_participant` takes them, and `ChannelLoop::participants_named` is gone.
     `agent wakeup` and `participant nudge` give the actors running, `participant list` and
     `participant remove` those the issuer reads taking part (`taking_part`).
- **2026-11-12** — Decided with the maintainer, for the group `agent`:
  1. `agent list` lists the agents of the session the issuer may read - those taking part in it,
     in any channel or none, and those lying in its scope or its channels' -, by a new
     `SessionView::agents`.
  2. `agent delete` seeks whom it deletes among them, as `participant remove` does among those
     `participant list` lists; an agent of the issuer or a global one not in the session is
     deleted in the web UI.
  3. An agent taking part may be deleted by command: it rests in its channels and can be
     restored, so nothing is asked again.
  4. Neither has a short form.

  Step 3 done. How a text names one of several is `value::name::named` in the domain, over
  anything with a name, the handle being the name's; `command::named_one` - formerly
  `named_participant` - finds the one a command names and words that it did not, for the
  participants of a channel and the agents of a session alike. `AgentView` gives its name, handle
  and personality as `ParticipantView` does, and `Views::agents_of` lists agents by a query, for
  `Views::agents` and `SessionView::agents`.
- **2026-11-12** — Decided with the maintainer, for the group `channel`:
  1. `channel describe [channel]` shows a channel; what it is for is set by `channel summary` and
     `channel briefing`, its two parts, each keeping the other.
  2. `channel rename`, `summary` and `briefing` act on the channel they are issued in, and
     `describe` on it unless a channel is named: no command takes two names in a line.
  3. `channel create <name>` makes a channel as `ChannelSpec::new` does, with its issuer alone.
  4. `channel leave` needs a right and an operation of its own: issue
     [562](562-nobody-can-leave-a-channel.md).
  5. No command of the group has a short form.
  6. In two steps: first `list`, `describe`, `rename`, `summary` and `briefing`; then `create`,
     `delete` and `join`.

  Step 4a done: the first five. `CommandContext::session_view` and `channel_view` give the
  issuer's view of the session and of the channel, for the groups `participant`, `agent` and
  `channel` alike. A text that does not fit - a name, a summary, a briefing - is refused before
  anything is changed. `channel summary` and `briefing` write both parts back, so a change of the
  other part made at the same moment is lost; the maintainer judged it too rare to change
  `update_description` for.
