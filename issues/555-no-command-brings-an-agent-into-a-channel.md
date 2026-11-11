# 555 — No command brings an agent into a channel

**Status:** done
**Type:** feature
**Priority:** medium
**Area:** User interaction
**Summary:** Someone writing in a channel cannot make an agent of a personality or bring an agent - a global one, their own, or one not yet in the session - into the channel by a command; both can be done only through the REST API, as the web UI's dialogs do.

## The problem

Whoever is working in a channel and finds that another agent is needed has to leave the line they
are typing in: the built-in commands are `/help`, `/wakeup`, `/nudge`, and of the agents `/skill`,
`/skills` and `/compact` ([COMMANDS.md](../docs/COMMANDS.md#built-in-commands)). None of them makes
an agent or changes who takes part.

## What is wanted

A way, by command, to get an agent into the channel the command is issued in. The maintainer named
what has to be thought of:

- The agent may be a **global** one, not only one of the session or of the issuer.
- The agent may **not exist yet**, and first has to be made from a personality.
- The agent may **not be in the session yet**, and has to be taken into it before it can be added
  to the channel.

The maintainer's first thought is two commands, `create-agent` and `add-agent`; that is to be
discussed.

## What exists

The operations are all there, so this issue is about a frontend of them, not about new rules:

- `AgentManager::create` makes an agent of a personality in a scope, asking `CreateAgent`, whether
  the maker may read the session or channel it is to lie in, and `GetPersonality` (issue
  [227](227-agent-management.md)). `POST /agents` is its route.
- `SessionLoop::add_participant` takes someone into the session and, with a channel named, into
  that channel at once, asking everything before anything is stored; one already in the session
  needs only the right to change the channel (issue [221](221-adding-session-participants.md)).
  `SessionLoop::join` is the same by one's own act, where a `Join` entry lets them.
- `SessionView::candidates` finds whom one could take in, by a prefix of the name, for whoever may
  change the session. `scope::find_by_name` answers which of a name is found from a place, the
  innermost first (issue [178](178-scoped-lookup-by-name.md)).
- Commands are `ChannelCommandProvider` components, one file per command in
  `caelumora-kernel/src/command/`; `/nudge` and `/wakeup` are the closest pattern. `/wakeup` shows
  how a command leaves the permission of what it does to the operation that does it
  (`WakeChannel`).

## Considerations

### One command or two, and for whom

- **A. `/create-agent` and `/add-agent`**, as the maintainer suggests. Each does one thing and says
  so; a new agent takes two lines. Nothing has to be taken back: if adding fails, the agent is
  there and the issuer sees why.
- **B. One command** that finds the agent or makes it, as `template::agent` does. One line, but
  whether a name is an agent or a personality has to be decided from what is found, a typing error
  can make a new agent, and an agent made whose adding then fails has to be taken back. Today the
  web UI's shortcut deletes it, and a template takes the whole session back (issue 227); the
  command would be a third place for that rule.
- **C. As A, but `/add` instead of `/add-agent`**, taking any participant: users and agents are
  treated alike unless a technical reason forces a difference, and `/nudge` was widened from agents
  to every participant for that reason (issue 278). `SessionLoop::add_participant` already takes
  either, and `candidates` finds either.

Recommendation: C. `/create-agent` is only about agents, because only agents are made so.
`/create-agent` would be the first command with a `-` in its name; `parse_input` allows it, since a
name runs to the first whitespace.

### How a name is found

`ChannelLoop::agents_named` and `participants_named`, which every command uses
([Naming an agent](../docs/COMMANDS.md#naming-an-agent)), look among those taking part in the
channel, which is exactly who the new one is not. Adding needs a lookup from the channel as a place:
channel, session, the issuer's own, global, the innermost first, among what the issuer may read.
That is the rule of issue 178: a global agent is found, and one of the same name nearer shadows it.
An agent the issuer may not read is answered like one that does not exist, so that the command
tells nobody what exists. `/create-agent` needs the same for the personality. Whether a user is
found by the same rule has to be settled with option C; a user lies in the global scope.

### What is typed

A name may hold spaces (`Name`), and a command gets the rest of the line unparsed. `/nudge` and
`/wakeup` take one name, so the whole rest is the name. `/create-agent` takes two - the personality
and the name of the new agent -, so it needs a rule: quoting, a word between them
(`/create-agent Reviewer as Bob`), or the personality's name as the default for the agent's. The
rule should be one that later commands can use too.

### Where a new agent lies

`AgentManager::create` takes the scope; the command has to choose or be told:

- **The session's**, as the web UI's shortcut in `AddParticipantDialog` does (issue 227): it goes
  with the session. It also keeps the session from being deleted by someone who may not delete the
  agent (`ScopeNotEmpty`), so the issuer's agent can stand in the way of the session's owner.
- **The issuer's**: it stays, with its memory, for later sessions, and survives the session.
- **The channel's**, or **global**, the last only for whoever `CreateAgent` allows it.

Whether that is an argument from the start or only one fixed default is to be decided; nothing
without a user (AGENTS.md).

### Session and channel

The maintainer's third case needs no step of its own: `SessionLoop::add_participant` with the
channel named takes the agent into both, in that order, and asks for the session's right only if it
is not in the session yet. The question left is whether a command should be able to add to the
session *without* adding to the channel; a command is issued in a channel, and nobody has asked
for it so far. An agent the channel refuses stays in the session, as for the dialog.

`join` is the other way in: someone who may not change the session, but whom a `Join` entry lets
bring in themselves or their own agent. Whether `/add-agent` covers that, and how it decides which
of the two applies, is to be decided; leaving it out at first is possible.

### How much it sees of the history

`add_participant` takes a `HistoryAccess`, all by default. An argument for it, or always all, as
`join` does: to be decided. The default has to be said in the command's description, since what is
shared cannot be taken back. What the agent may read of other sessions comes with it (issue
[143](143-joining-a-channel-with-foreign-knowledge.md)).

### Answer and record

The channel already tells everyone, and records in the name of whoever added it, that the agent
joined (`ParticipantJoined`). A channel-wide `CommandExecuted` would say it twice; the answer should
be private, like `/nudge`. `/create-agent` changes nothing in the channel and answers privately as
well.

### Who may

Not `owners_only`: what may be done is what `UpdateSession` and `UpdateChannel` allow, which an
access list can give to others than the owner, and the operations ask it themselves, as `/wakeup`
leaves it to `WakeChannel`. So everyone who reads the channel sees the commands, and those without
the right are refused (`CommandError::Forbidden`). Whether `/help` should hide them from those is
open; `ShowCommand` carries only the channel's attributes.

### What a command reaches

`CommandContext` gives a command the issuer's actor, the channel and the executor. The channel
reaches its session (`ChannelLoop::session_loop`, within the kernel); how a command reaches the
`AgentManager` and the agents' and personalities' stores is for the plan, since what a channel can
reach is what its runtime offers (issue 360). The lookup may belong beside `SessionView::candidates`
or `Participation` rather than in the command.

### Not part of this

- Agents acting so: commands are for people, an agent would need a tool.
- Removing someone by command, `/remove`, though it would be the obvious counterpart.
- Inviting someone who can decline: issue [342](342-invitations.md).

## Done when

- With `/add`, in a channel, an existing user or agent - an agent global, the issuer's, or of the
  session - can be brought into that channel, also when not in the session yet.
- With `/create-agent`, an agent can be made of a personality the issuer may read.
- The commands ask nothing the operations do not ask, and a name that cannot be used is answered
  without telling what exists; the security suite covers it.
- COMMANDS.md lists them, and describes how a name is looked up beyond those taking part.
- The decisions above are recorded here.

## History

- **2026-11-10** — Issue created at the maintainer's request, with the three cases and the
  suggestion of two commands. Looked at what exists: every operation is there (221, 227), and what
  is missing is the commands, a lookup of a name beyond the channel, and a rule for two names in
  one line. Nothing is decided yet.
- **2026-11-10** — Decided with the maintainer: option C, two commands. `/create-agent` makes an
  agent of a personality, `/add` brings any participant - a user or an agent - into the channel it
  is issued in. The other questions above are still open.
- **2026-11-10** — Decided with the maintainer:
  1. `/add` finds an agent from the channel as a place, the innermost first, and a user by their
     exact name. A name that finds several is refused with their names; nobody is picked.
  2. `/create-agent <personality> [as <name>]`; without `as`, the agent is named after the
     personality.
  3. A new agent lies in the session's scope, without an argument for now.
  4. `/add` shares the whole history, without an argument for now, and its description says so.
  5. `/add` is `add_participant` alone: `join` is left out for now.
  6. Everyone who reads the channel sees both commands; who lacks the right is refused on running
     them.
- **2026-11-11** — Decided with the maintainer, refining decision 1:
  1. `SessionView::candidates` takes a `NameMatch` instead of its `prefix`, an enum of the domain
     with two variants, `Prefix` and `Exact`, both as names are compared - a pattern for other
     searches as well. With `Prefix` it stays the search of the dialog: only for whoever may change
     the session, from `MIN_PREFIX` characters on, at most `CANDIDATES` of each kind. With `Exact`
     it is what `/add` asks: without that guard, since one whole name tells nothing the stores do
     not let one read anyway, and without a limit, which must not cut off what was named.
  2. A name that finds several is not refused as such, since a name is all one can type, and a
     handle is derived from it: the nearest from the channel wins. The ranks are the channel's
     scope; the session - its participants, wherever they lie, and the agents of its scope; the
     issuer's own agents; the global scope, its agents and the users. An agent of another user's
     scope that is not in the session is not found (issue 178). One that is shadowed is not
     reached by the command; the dialog still lists it.
  3. Several on the same rank - a user and an agent of one name in the global scope, two
     participants of the session of one name - are refused with their names; the web UI is the
     way to them. A word naming the kind (`user:`, `agent:`) is left for when it is needed.

  Proposed and still open: `Exact` as a condition of `Filter` (`Filter::name_matching`, replacing
  `name_prefix`), so that the stores are asked by it; how `/create-agent` reaches the
  `AgentManager` (as a service of the kernel, held by its component); lifting `/wakeup`'s
  `channel_error` into `command/mod.rs` as a step of its own; `as` split at its last occurrence;
  how a found candidate's place reaches `/add` for the ranks.
- **2026-11-11** — Decided with the maintainer, the points left open above, as the plan of three
  steps:
  1. Without a change of behaviour: `command::command_error` - a refusal of an operation a command
     ran is the command refused, anything else a failure - out of `/wakeup`, for `/nudge` as well;
     and `scope::innermost`, the choice of the innermost among what was found, out of
     `find_by_name`, which is built on it.
  2. `NameMatch` is a condition of `Filter` (`Filter::name_matching`, replacing `name_prefix`), and
     `candidates` takes it; with `Exact` it ranks what it found as decided above, giving the
     participants of the session the session's rank. The REST route passes `Prefix`.
  3. The `AgentManager` is a service of the kernel, held by the component of `/create-agent`; the
     personality is to be found by a new `Views::personality_named`, from the channel as a place;
     `as` is split at its last occurrence; both commands join the default set.
- **2026-11-11** — Step 1 done. Between it and step 2, as the maintainer decided: `found_from` is
  built on `find_by_name`, so that the choice of the innermost is made in one place, and the test
  helper `channel::testing::command_error` is called `as_command_error`, so that `command_error`
  in the kernel means one thing.
- **2026-11-11** — Step 2 done: `NameMatch` (`value::name`) with `Prefix(String)` and
  `Exact(Name)`, a whole name being a `Name`; `Filter::Name` replaces `NamePrefix`, made by
  `Filter::name_matching`. `SessionView::candidates` takes it, and with `Exact` answers whoever
  asks, without a limit, the nearest as decided. The REST route passes `Prefix`.
  Decided with the maintainer on the review: why a candidate could not be taken in is told only
  to whoever may change the session, since it says whether they are cleared for the channel.
- **2026-11-11** — Step 3 done: the `AgentManager` is a service of the kernel; `/add` and
  `/create-agent` are built-in commands of the default set (`command/add.rs`,
  `command/create_agent.rs`), the personality found by `Views::personality_named`
  (`ViewError::NoPersonalityNamed`). The security suite covers both; COMMANDS.md describes them.
  The issue is done.
  Decided with the maintainer on the review, refining decision 3: an agent made of a personality
  of the channel's scope - which makes agents only there - lies in the channel's scope.
