# 561 — The terminal shows only the first channel of a session

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui` always talks in the first channel of a session and cannot change to another, so whoever takes part in several channels of a session can reach only one of them from the terminal, and does not learn what happens in the others - not even that an approval waits there.

## The problem

The terminal opens or starts a session and takes its first channel, "the template's first channel,
whatever it is called" (`caelumora-tui/src/main.rs`, `session_loop.channels().into_iter().next()`).
`run_repl` is given that one `ChannelLoop`, takes the user's actor in it, and runs until the program
ends. A session resumed with `--continue` or `--resume` comes back in its first channel as well.

A session of several channels is reached from the terminal only in part. That grows with issue
[560](560-commands-are-not-grouped-by-what-they-act-on.md), which brings `channel create` and
`channel join`: a user who creates or joins a channel by command cannot talk there afterwards.

## What is wanted

`channel switch <channel>`, proposed with the commands of 560, makes the terminal talk in another
channel of the session. The maintainer decided that it is an issue of the terminal of its own,
rather than part of 560: it changes what a frontend shows, not the session.

## What exists

- **The terminal's own commands.** `/approve`, `/deny`, `/answer`, `/decline` and `/retry` are
  handled by the terminal before anything goes to the channel; every other command is sent with
  `UserActor::execute_command`.
- **What the terminal keeps of a channel.** `run_repl` holds, besides the user's actor and the
  stream it watches (`UserActor::watch`), the approvals pending in the channel, the last answer and
  the turn under way or given up for `/retry`, the questions open and the one being answered, and
  the handles of the participants - all of one channel, as variables of the loop.
- **The channels of a session.** `SessionLoop::channels` lists them; `ChannelLoop::user_actor`
  gives the user's actor in a channel they take part in.

## Considerations

### What happens in the other channels

- **Nothing is shown.** The terminal watches the channel it talks in alone. An approval or a
  question asked of the user elsewhere stays unseen, and the agent waits there until the user
  changes to it by chance.
- **A notice.** The terminal watches every channel of the session the user takes part in, and of
  the others tells only what waits for the user - an approval, a question, perhaps a mention -
  with the channel's name.
- **Everything, marked.** Every line of every channel, with the channel's name in front. A
  terminal of several busy channels would be hard to read.

### What the terminal keeps per channel

The state of `run_repl` belongs to one channel. Switching needs it once per channel - kept while
the user is elsewhere, so that `/approve` of an approval of the channel left still finds it, or
dropped and rebuilt from the channel on return. Gathering it into a type of its own is a
restructuring before the switch, without a change of behaviour.

### Which channels

- Only those the user takes part in, so that switching joins nobody: `channel join` first, as 560
  has it.
- Or switching to one the user does not take part in joins it, where a `Join` entry lets them.

The channel is named as 560 decides for the names of channels; how a name with spaces is typed
follows 560 too.

### The name of the command

The terminal's own commands are matched before the channel's. With the groups of 560, the group
`channel` would be the session's in part and the terminal's in part, and a `channel switch` the
session came to offer would be hidden by the terminal's. `/help` of the session does not list the
terminal's commands. Whether they keep words of their own (`/switch`), take the group `channel`, or
the terminal's commands get a group of their own is to be decided, with 560's `CommandName` and
short forms.

### Where it starts

Starting in another channel than the first - by an argument, or in the channel spoken in last when
a session is continued - would be the same choice made at start. Nobody has asked for it yet.

## Done when

- In the terminal, the user can change to another channel of the session they take part in, and
  talk there.
- What is asked of the user in another channel is not left unseen, as decided above.
- The decisions above are recorded here.

## History

- **2026-11-12** — Issue created at the maintainer's request: `channel switch`, proposed with the
  commands of 560, is a feature of the terminal and goes into an issue of its own. Nothing else is
  decided yet.
