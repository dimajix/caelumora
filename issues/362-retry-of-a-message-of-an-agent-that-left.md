# 362 — The channel admits a request to answer a message again for an agent that left, and drops it unseen

**Status:** open
**Area:** Sessions and channels

## The problem

`ChannelLoop::admission` (`channel/run.rs`) decides two kinds of `RetryRequested` differently:

- **A turn** (issue 235) is admitted only for an agent taking part:
  `author.as_agent_id().is_some() && self.actor_by_id(*author).is_some()` - since
  [350](350-lookups-written-out-again.md), `self.agent_actor(*author).is_some()`.
- **A message** (issue 234) is admitted if `message.sender_id == *author &&
  author.as_agent_id().is_some()`: the agent said the message. Whether it still takes part is not
  asked.

A request for the message of an agent that left the channel is therefore let in, and the moderator
routes it to nobody (`PolicyModerator::route_to_agent` answers no route for an agent that is not
there). The sender is told nothing - no `MessageRejected`, no `RetryRejected` -, while the same
request for a turn of that agent is refused as `Invalid` and its sender learns so. The
`WakeupRequested` sent ahead of it (`UserActor::send_retry`) is admitted as well and wakes the
channel's agents for a request none of them receives.

`UserActor::request_retry` asks the question before it sends (`NotAnAgentsMessage`), so a user of
the REST API and the web UI meets the gap only in a race: the agent leaves between the check and the
admission. The admission is meant to be the channel's one decision of what enters it (issue 304),
whoever sends; it should not rely on the sender having asked.

## Why it matters

A request that is admitted and then delivered to nobody is the one outcome the sender cannot tell
from a slow agent: the user's frontend waits for a `RetryAccepted` or `RetryRejected` that never
comes. And the two kinds of retry answer one question - is the agent named here to answer - two
ways.

## Considerations

- **Ask the same of both.** The message arm becomes `message.sender_id == *author &&
  self.agent_actor(*author).is_some()`; a request for an agent that left is refused as `Invalid`,
  and its sender is told by `MessageRejected` as of every refusal. The doc of `admission` says "of a
  message an agent of the channel said". The test
  `a_change_to_a_message_enters_the_channel_only_from_its_author_as_it_stands` in `channel/run.rs`,
  which covers which requests are not admitted, gains the case of a message whose agent left.
- **Leave the admission, answer in the moderator.** `route_to_agent` finding nobody could answer a
  `RetryRejected` in the channel's name. That puts a decision about what enters the channel after
  the entry, which 304 moved into the admission; not recommended.

A behaviour change, if a small one: a request that was dropped unseen is refused visibly.

## Done when

- A request to answer a message again names an agent taking part, or it is not admitted, as a
  request to answer a turn again.
- A test shows it.

## History

- **2026-10-14** — Noticed while resolving [350](350-lookups-written-out-again.md), which left it
  as it was since it asked for no behaviour change.
