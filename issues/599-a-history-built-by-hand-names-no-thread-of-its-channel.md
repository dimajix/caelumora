# 599 — A history built by hand names no thread of its channel

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** An entry recorded without an envelope (`Recorded::new`, and with it `History::push`, `extend` and `record`) takes its context from `EventContext::default()`, so it happens in the nil thread - a thread of no channel -, and since a page of a channel's line asks for its line (issue 557), a test history built so is silently left out of it: two tests of the views passed while reading nothing they were meant to.

## The problem

What a channel records comes in an envelope made for it (`Recorded::of`), whose context names the
channel's line unless it was made for another thread (`EventContext::new`, issue
[557](557-a-conversation-cannot-go-on-two-ways-at-once.md)). What is recorded without one -
`Recorded::new`, used by `Recorded::delivered` and so by `History::push`, `extend` and `record` -
builds its context from `EventContext::default()`: its `thread_id` is `ThreadId::default()`, the
nil id, the line of no channel.

For an entry of the whole channel - a change of a message, which `modify`, `revoke` and
`reclassify` record so in production - the thread does not count (`Recorded::thread` is `None`).
For an entry of a thread - a message said, a join, a turn's end - it does: since step 6 of 557's 2b
a page of a channel's line (`ChannelView::history_page`), what is new to a user
(`ChannelView::unread`) and the REST API's channel history ask for the line
(`Filter::shown_in_line`), and an entry of the nil thread is none of it.

In production nothing records such an entry. In the tests many do: some thirty histories are built
by `History::push`, `extend` and `record`, and test helpers record by `Recorded::new` (an agent's
context in `view/context.rs`, the agents' `channel_history` tests). Two tests of the views
(`what_is_new_to_a_user_is_counted_from_what_they_read`,
`what_a_channel_holds_is_read_through_its_nodes_and_a_participants_context_by_who_may`) read their
messages through the line after step 6 and found none; they were given line helpers of their own
(`in_line_of`, `said_in_line`) in the same change. A test asserting that something is *not* read
passes either way.

## Why it matters

A test whose fixture lands where nothing reads it passes for the wrong reason, and the next rule
that reads by thread makes more of them, found only where an assertion is positive.

## Considerations

- **`Recorded::new` takes the thread** (or the context) it happens in, and the history's helpers
  their channel: `History::push` becomes `ChannelState::say` or takes a `ThreadId`. Every caller
  says where; the nil thread is no default any more.
- **`History` knows its channel's line**, and records what has no context there. Fewer changes
  at the callers; a `History` without a channel - an agent's context in a test - needs one.
- **`ThreadId` without `Default`**, so that `EventContext` has none either: the compiler finds
  every place that builds a context without saying its thread - also outside the tests.
- **Leave it and document it**, holding the trap by a test that a pushed message is of no line:
  cheapest, but every new fixture can fall into it again.

## Done when

- An entry recorded by hand happens in a thread its maker names, or in its channel's line, and no
  test history lands in a thread of no channel.

## History

- **2026-11-18** — Opened from issue 557, step 6 of 2b, where two tests of the views were found
  reading nothing through the line, as the maintainer asked.
