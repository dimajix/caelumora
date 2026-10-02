# 199 — An external agent's report wakes every agent

**Status:** done
**Area:** Sessions and channels

## The problem

When a task an external agent follows ends, `ExternalAgentActor::speak` says the result in the
channel on the agent's own account and sends a `WakeupRequested` ahead of it, "as a user's message
is, so that the channel's sleeping agents hear it" (CHANNELS.md). The channel answers a
`WakeupRequested` with `wake_for_delivery`, which wakes *every* actor of the channel and ends a
quiet channel.

That contradicts the rule decided in [45](45-agent-mentions.md): a sleeping agent that is not
explicitly addressed keeps sleeping. An agent that said good-bye (`stop`, `bye`) is brought back by
another agent's report that has nothing to do with it, and a channel a user made quiet with a
cancel is woken without any user acting.

The reason given for it does not hold: a sleeping agent hears messages anyway.
`AgentActor::handle_waiting` perceives a `FullResponseGenerated` asleep or awake, and
`forward_routes` delivers to every agent regardless of its state. Sleep only stops it from
answering.

## Why it matters

- Sleep is the agent's own decision to leave the conversation; only a person (a message, a
  mention, `/wakeup`) or an explicit mention by an agent should end it.
- A quiet channel is ended only by an action that was permitted to a user; a remote task finishing
  is none.

## Considerations

- **Drop the `WakeupRequested`** from `speak`. Sleeping agents still hear the report; with the
  waking of issue 45 (the channel wakes whom the moderator asks to respond), an agent the report
  mentions is still woken.
- **What the report makes others do** while the channel is quiet: `route_event_impl` asks nobody to
  respond in a quiet channel, so the report is delivered and answered by nobody, as an agent's
  message in a quiet channel is today.
- **Order with 45:** the dependency only goes one way. Dropping the `WakeupRequested` is safe
  before 45; a mention in a report wakes the mentioned agent only once 45 is done.

## Done when

- An external agent's report wakes no agent that it does not address, and does not end a quiet
  channel.
- CHANNELS.md no longer gives sleeping agents' hearing as the reason for a wakeup.
- A test covers a sleeping agent that hears a report and stays asleep.

## History

- **2026-10-02** — Issue created while discussing [45](45-agent-mentions.md).
- **2026-10-03** — Done. `ExternalAgentActor::speak` no longer sends a `WakeupRequested`: the
  report is delivered to every agent, sleeping or not, and wakes only an agent it mentions, which
  the channel wakes because the moderator asks it to respond (45). A quiet channel stays quiet.
  The test of a followed task's report now expects no `WakeupRequested`; that an agent not asked
  keeps sleeping is covered by `the_channel_wakes_only_whom_the_moderator_asks_to_respond`.
  CHANNELS.md no longer gives sleeping agents' hearing as the reason for a wakeup.
