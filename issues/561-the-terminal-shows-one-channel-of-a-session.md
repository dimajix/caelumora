# 561 — The terminal shows only the first channel of a session

**Status:** in progress
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
- **2026-11-12** — Decided with the maintainer:
  1. Of the channels it does not talk in, the terminal tells what waits for the user - an
     approval, questions put to them, a mention of them -, with the channel's name. So it follows
     every channel the user takes part in, from the start: a watch shows only what comes after it
     began (`UserActor::watch`), and what `/retry` gives again cannot be rebuilt.
  2. What the terminal keeps of a channel is kept per channel, in a type of its own, made in a
     step before the switch, without a change of behaviour.
  3. `/approve`, `/deny`, `/answer` and `/decline` with an id act in the channel where it waits,
     without a switch; without one, in the channel talked in.
  4. Only the channels the user takes part in are switched to; switching joins nobody.
  5. The command is the terminal's own word, `/switch <channel>`: the group `channel` is the
     session's since 560, where `/help channel` lists it and the `CommandSet` keeps names apart.
     The channels are listed by `/channel list`.
  6. Joining or creating a channel by command does not switch to it; the terminal tells how to.
  7. Not raised in the issue, reachable since 560 and 562: when the user leaves the channel talked
     in, or it is deleted, the terminal says so and talks in none until `/switch`; and it starts
     in the first channel the user takes part in, not the session's first, which they may have
     left.

  In three steps: the type per channel; the first channel taken part in and a channel that ends;
  then following every channel, `/switch`, the notices, and the channels joined meanwhile, which
  the terminal learns of from the event bus.
- **2026-11-12** — Step 1 done, without a change of behaviour: `Followed` (`caelumora-tui`,
  `followed.rs`) holds what the terminal keeps of a channel - the user's actor, the approvals and
  questions open, what `/retry` asks for - and takes in what the channel shows, answering the lines
  to print. The watch and the questions being answered line by line stay in `run_repl`; what the
  user types is handled by `handle_line`.
- **2026-11-12** — Decided with the maintainer on the review of step 1: two findings become steps
  of their own. The steps are now: 2, an approval a rule of the user answered is told again; 3,
  the first channel taken part in and a channel that ends; 4, `Followed` answers which open
  questions are put to the user, for `/answer` and `/decline` alike, so that the terminal no longer
  knows the user beside it; 5, following every channel and `/switch`.

  Step 2 done: "answered by your rule" was never printed, since the user's actor shows a rule's
  answer in the user's name and the terminal passed over all it sent so; it lets that answer
  through, and no longer tells another user's answer by a rule as the user's. As in the web UI,
  a rule's answer is the user's: it cannot be given again, as after `/approve`.
- **2026-11-13** — Step 3 done. The terminal starts in the first channel of the session the user
  takes part in, and refuses to start if there is none; `Followed::new` answers `None` for a
  channel the user takes no part in. Following a channel ends as the channel stream of the REST
  server ends (`ChannelFollower`, `docs/CHANNELS.md`, issue 386): where a run of the user's actor
  began or ended (`Shown::Started`, `Shown::Ended`), the terminal asks the views in the user's
  name whether the channel still lists them (`Followed::ended`). If not - they left, were removed,
  or the channel was deleted -, it says so and talks in no channel; a line typed then, but `exit`,
  `quit` or an empty one, is answered that it takes part in none. A first plan followed the event
  bus for it; the maintainer decided for the pattern of the REST server on review. The bus is left
  to step 5, for the channels joined meanwhile.
- **2026-11-13** — Step 4 done. `Followed` holds the views in the user's name, whose viewer is the
  user it follows the channel for (`Followed::new(channel, views)`), and answers which open
  questions are put to them (`Followed::questions_put_to_user`), for `/answer` and `/decline`
  alike: `/decline` of questions put to others is answered in the terminal, as `/answer` was,
  instead of being refused by the user's actor. The terminal no longer knows the user beside
  `Followed`. Decided with the maintainer: a view that cannot be read ends following the channel,
  as it ends the channel stream of the REST server. Two findings become steps of their own before
  the switch: 5, `ChannelWatch` answers only the error that can occur; 6, whether a user still
  takes part in a channel is asked in one place. The switch is step 7.
- **2026-11-13** — Step 5 done. A `ChannelWatch` holds its outbox, which so never closes: it fails
  only by falling behind, and answers so (`Lagged`, `caelumora_kernel::actor::user`) rather than
  tokio's `RecvError`, whose `Closed` every caller handled though it never came. `try_recv`
  answers `None` where nothing was shown, as `EventSubscription::try_recv` of the event bus does.
