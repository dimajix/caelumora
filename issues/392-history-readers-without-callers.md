# 392 — Readers of a history that nothing calls, and comments that describe an older history

**Status:** done
**Area:** Code structure
**Epic:** [391](391-history-and-transcript-review-epic.md)
**Summary:** `History::messages`, `items` and `recorded_context_of` read a history held in memory, which nothing in production does any more, and two comments of the agent's run loop describe a history that keeps only final messages.

## The problem

Since [379](379-history-stored-as-it-stands.md) every reader of a history in production reads the
items as they stand from the store (`HistoryStore`, `StandingItems`), and since
[387](387-agent-history-window-without-compaction.md) an actor holds only a window of its history.
What read a whole history in memory is still there, called by tests alone:

- `History::messages()`, `History::items()`, the private `folded()` and the free `item()`
  (`caelumora-model/src/history.rs`): the active path folded into messages or items. Their callers
  are tests, the test helper `ChannelLoop::history()` in `caelumora-runtime/src/channel/testing.rs`
  and the test helper `given_up_turns` in `caelumora-agents/src/message.rs`. The test in
  `store/history.rs` shows that `StandingItems::of(history.all_entries())` gives the same items.
- `HistoryEntry::context_of(id)` and `AgentChannelState::recorded_context_of(id)`: what a message
  was produced from, read from the history held in memory. Production reads it through
  `HistoryStore::said` and `HistoryEntry::context` (`AgentContextView::context_of`); the two are
  called by tests alone.
- `Chain::new()`: only tests call it, `Default` does the same.

Beside them, two doc comments in `caelumora-agents/src/actor/agent.rs` (`end_turn`, about line
195, and `handle_planning`, about line 509) say that the history "only keeps genuinely final
messages" and that internal messages are "excluded from history in `end_turn`". Since
[258](258-what-was-said-is-lost.md) and [260](260-turn-messages-into-the-transcript.md) every
message a turn says goes into the history as it is said, internal steps included.

Smaller, in the same area: `JsonFileStore::page` (`store/json/histories.rs`) filters by `from` and
`keep` while it reads the segments, and `page_of` filters by both again; `HistoryEntry::context`
matches with `_` where its neighbours `said`, `changed` and `is_item` name every variant.

## Why it matters

- A reader that only tests call is a second way of reading the same thing, kept right for nobody;
  the next change to how a history is read has to be made there too, or the tests drift from what
  runs.
- The two comments tell whoever reads the agent's run loop the opposite of what the code does.

## Considerations

- The test helpers that fold a history in memory can read `StandingItems::of(history.all_entries())`
  instead, or fold with `history::fold`, which stays for `said_in_turn` and `catch_up`. The helper
  `given_up_turns` needs items with their turn ends; `StandingItems` gives them.
- `recorded_context_of` and `context_of` can move into a `cfg(test)` module beside their tests,
  if the tests of `ContextResolver` are shorter with them than with `HistoryEntry::context`.
- Nothing here changes behaviour or stored data.

## Done when

- No method of `History`, `HistoryEntry` or `AgentChannelState` reads a history in memory into
  messages or items unless production calls it; what tests need lives with the tests.
- The two comments in `actor/agent.rs` say what the history holds, or are gone.
- AGENT-TURN.md (*What is held in memory*) still lists what reads the history in memory, and the
  list matches the code.

## History

- **2026-10-16** — Opened from the review of the history and the transcript
  ([391](391-history-and-transcript-review-epic.md)).
- **2026-10-16** — Done. `History::messages`, `items`, `folded`, `item`, `HistoryEntry::context_of`
  and `AgentChannelState::recorded_context_of` are gone; what the tests read of a history held in
  memory is `history::testing` (`items_of`, `messages_of`, `recorded_context`), as the coding
  guidelines place shared fixtures: `history.rs` became `history/mod.rs` beside `testing.rs`, and
  `caelumora-model` got a `test-support` feature, which that of `caelumora-runtime` turns on, so
  that the tests of the agents and the plugins have it too. `items_of` folds the chain itself, so
  the store suite still checks the store's items against the chain folded, not against
  `StandingItems`. `Chain::new` is gone; `HistoryEntry::context` names every variant. The comments
  of `end_turn` and `handle_planning` say that internal messages are kept like every other. The
  double filter of `JsonFileStore::page` stays: the one in its loop decides when to stop reading
  segments, the one in `page_of` serves both stores. No stored data changes.
