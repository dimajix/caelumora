# 278 — A user cannot ask an agent to respond without writing a message

**Status:** open
**Area:** User interaction

## The problem

An agent responds only when the moderator sends it a `ResponseRequested` - because a message was
said and the policy chose it, or because the message mentioned it. A user has no way to ask an
agent to respond to the conversation as it stands without saying something new: `/wakeup`
(`caelumora-runtime/src/command/wakeup_command.rs`) ends an agent's sleep, but asks nothing of it,
so the woken agent waits for the next message (see [AGENT-TURN.md](../docs/AGENT-TURN.md), *Begin*).
Asking again (issues 234, 235) replaces an answer the agent gave, which is something else.

Situations in which this is missing:

- an agent was woken with `/wakeup`, or the channel after a cancel, and should take up the
  conversation where it stood;
- the policy chose another agent, or nobody (the chain between agents reached its limit, issue
  200), and the user wants this one to answer as well;
- an agent ended its turn early - it went to sleep, a tool failed - and should simply continue.

Today the user writes "@sassy?" for it, which puts a message without content into the history, in
every transcript and in what the agent answers to.

## Why it matters

Being asked to respond is the one thing that makes an agent act, and it is the only one a user
cannot do directly. The workaround of an empty mention costs a message in the history for what is
a request about the conversation, not a contribution to it.

## Considerations

**Name.** "Anstupsen" is *to nudge* (or, more bluntly, *to poke*). `/nudge [agent]` says what it
does without suggesting that something is written or repeated; `/poke` reads as playful, `/respond`
and `/ask` as if the user were the one answering or asking something specific.

**Which pattern it follows.** Two existing paths ask something of one agent:

- *Asking again* (issue 234): `UserActor::request_retry` checks `RetryMessage`, sends a
  `WakeupRequested` ahead and then a `RetryRequested { author, .. }`, which the moderator routes to
  `author` alone (`PolicyModerator`, `route_to_agent`), and which the channel wakes the agent for
  before delivering (`route_event_impl`). The request carries the user's context, so the turn acts
  with the authority of the user who asked - as it does after a user's message.
- *A channel command* (`/wakeup`): the command calls an operation of the `ChannelLoop`
  (`wakeup_actor`), which checks `WakeChannel` itself.

The nudge is an envelope in the user's name to one agent, like the retry; so it belongs on the
retry's path: a `UserActor` operation that checks the permission and sends the request, and the
command `/nudge` as its frontend (the REST API and the web UI could offer it as an action on a
member later). `ResponseRequested` itself carries no addressee - the moderator decides where it goes
- so the user's request needs either an addressee in the event (e.g. `ResponseRequested { agent:
Option<ActorId> }`, `None` as the moderator sends it now) or an event of its own that the moderator
turns into the `ResponseRequested` to that agent. The latter keeps "only the moderator asks for a
response" true and lets a moderator refuse or log it; the former is less code. To be decided.

**Permission.** A user who may write in the channel can already achieve it with a mention, so the
nudge asks nothing beyond that - but a policy may want to tell them apart (a reader who may ask for
another answer but not write). `RetryMessage` is described as "an act on the conversation, like
cancelling or waking it"; a nudge is the same kind of act. Options: reuse `RetryMessage` (renamed to
something covering both, e.g. `RequestResponse`), or a new `ActionType`. Either way a test in the
security suite, since the turn acts in the user's name.

**A working agent.** A `ResponseRequested` to an agent with a turn under way is taken in after the
turn and begins another one - the agent answers twice. A nudge should rather tell the user that the
agent is working already and send nothing (`ChannelLoop` knows the member's state, issue 243).

**A quiet channel.** After a cancel the channel asks nobody, and drops a `ResponseRequested`
(`route_event_impl`). A nudge is deliberate, like the retry, which sends a `WakeupRequested` ahead
of it for that reason; the same is right here. Whether it ends the channel's quietness for the
others is the question `WakeupRequested` already answers - it does not have to be decided anew.

**Without an argument.** `/nudge` alone could ask the moderator to choose as if the last message
had just been said - "somebody answer this". Useful after `/wakeup`, but it can be added later; the
first step is one named agent, looked up as issue [277](277-command-agent-by-name.md) settles it.

**Recommendation:** `/nudge <agent>` as a channel command over a `UserActor::request_response`
built like `request_retry`, an event of its own that the moderator routes to that agent alone as a
`ResponseRequested`, the permission shared with asking again under a name that covers both, and a
refusal for an agent that is working already.

## Done when

- `/nudge sassy` makes Sassy respond to the conversation as it stands, in the name of the user who
  asked, without a message in the history; a sleeping agent is woken for it, and a quiet channel
  does not swallow it.
- Nudging an agent that is working is answered with that, not with a second turn.
- The permission is checked and covered by the security suite; the command is documented in
  [TOOLS-AND-COMMANDS.md](../docs/TOOLS-AND-COMMANDS.md) and enabled by default next to `/wakeup`.

## History

- 2026-10-09: Opened at the maintainer's request: next to `/wakeup`, a command that only sends a
  `ResponseRequested` to an agent.
