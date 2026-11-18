# 599 — A history built by hand names no thread of its channel

**Status:** done
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

## Decision

Option 1 (2026-11-18, with the maintainer), with the whole context rather than the thread alone:
`Recorded::new` takes the context its entry happened in, as `Recorded::of` takes the envelope's -
its session, its channel, its thread and its chain -, and so do the history's methods that record
without an envelope: `push`, `extend` and `record`, and the changes `modify`, `revoke`,
`reclassify` and `fit_into`. A thread alone would have left a context naming a channel's line but
no channel, a combination no envelope has, and `Envelope::is_shown_in_line` reads the line off the
context's channel. Of the history's methods, the sender is the entry's
(`HistoryEntry::named_sender`), not the context's: one context serves a whole test history, and
what is delivered again from it comes from its sender. A test makes the context with
`actor::testing::context_in`, the line of a channel in a session of its own, or with
`EventContext::new` where it knows the session. So nothing the history records by hand happens
where nobody said, and `Recorded::new` makes no context of its own.

Holding the line's id by the channel's has a consequence worth knowing: the nil thread is the line
of the nil channel (`ThreadId::line(ChannelId::default())`), so a test whose channel is nil read
its history through the line by chance.

In two steps:

1. The context is named: `Recorded::new` and the history's methods above take an `EventContext`;
   the channel's settings record a fitting in its own context; the line helpers the tests of the
   views were given (`in_line_of`, `said_in_line`) are left to what the history and
   `Recorded::new` offer.
2. What only tests record by hand moves into a guarded `impl` block of `history::testing`: of the
   history's methods, every one but `fit_into` and `enter` has no caller in production.

## Done when

- An entry recorded by hand happens in a thread its maker names, or in its channel's line, and no
  test history lands in a thread of no channel.

## History

- **2026-11-18** — Opened from issue 557, step 6 of 2b, where two tests of the views were found
  reading nothing through the line, as the maintainer asked.
- **2026-11-18** — Decided with the maintainer: option 1 for every entry recorded by hand, in two
  steps, first with the thread alone, then - for the context it left half filled - with the whole
  context (see Decision).
- **2026-11-18** — Step 1: the context is named. `Recorded::new` and the history's methods take
  it, the channel's settings record a fitting in the channel's own context, and every test history
  is recorded in a context of its channel - its line, or another thread where a test says so;
  `AgentContextView::of_state` takes the channel it builds its view in, which was the nil channel.
  Three tests had held the nil thread as the one a hand-built entry happens in - one of what a
  reader of an entry is given, one of how a store keeps an item, and the stores' suite on who
  takes part -; they hold the context's thread now. A test holds what the history's methods put
  into the head of what they record, and CODING-GUIDELINES.md names the rule for the tests.
- **2026-11-18** — Step 2: what only tests record by hand - `push`, `extend`, `record`, `modify`,
  `revoke` and `reclassify` - is an `impl` block of `history::testing`, guarded as the module is;
  the history's own API keeps `enter` and `fit_into`, the channel's ways of recording. Done.
