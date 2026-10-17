# 109 — `unwrap`, `unsafe`, infallible `Result`s, idle locks and clones

**Status:** done
**Area:** Maintenance
**Epic:** [74](74-refactoring-epic.md)
**Summary:** `unwrap`, `unsafe`, infallible `Result`s, idle locks and per-chunk clones outside tests.

## The problem

Against the rules in AGENTS.md, outside tests:

- **`unwrap`/`expect`:** `actor_impl/agent.rs:385` `c.result.unwrap()` (goes with
  [100](100-tool-executor-table-and-result.md)); `approval.rs:324` `.to_std().unwrap()`;
  `transport_impl/memory.rs:55` `remove(i).unwrap()`; `component.rs:215, 230, 245` (topological
  sort; provably safe, but `entry()`/`if let` cost nothing); `auth.rs:139`. `digest::to_json`
  (`digest.rs:164-166`) hashes `Null` on a serialisation failure without a word - in an integrity
  feature a justified `expect` is more honest than a silent fallback.
- **`unsafe`:** `transport_impl/memory.rs:49-50`, avoidable with `pub type EventPredicate = for<'a>
  fn(&'a Envelope) -> bool` in `transport.rs`.
- **Infallible `Result`, `async` without `await`:** `RuntimeManager::new`,
  `ChannelLoop::spawn_actors`, `handle_stopping`; `SessionLoop::channel_by_name`,
  `ControllerManager::new_agent_controller`, `SessionController::new`;
  `MemoryStore::read_lock`/`write_lock` return `StoreResult` but always `Ok` (30 `?` for nothing),
  and `get_user`/`find_user_by_name` clone the whole map instead of one entry (`memory.rs:306,
  312`).
- **Locks and clones:** `ChannelLoop::state: Arc<Mutex<…>>` and `processing_state: Arc<AtomicU8>`
  are never cloned; `route_event` clones the whole envelope (`channel.rs:658`) though
  `route_event_impl` only reads; `record_turn` clones `turn_messages` twice
  (`agent_turn.rs:332-333`); `AgentActor::label` fetches `tool_executor.tools()` per streamed chunk;
  `McpToolProvider::strip_prefix` builds `format!("{}__")` per call.
- **Logging:** `DefaultToolExecutor::execute` (`tool.rs:198-205`) puts five ids into the message -
  they belong on the span; `ollama.rs:407` logs every message id of a request at `debug!` - size,
  not list.

## Why it matters

Each is small; the rules exist so that the set stays small.

## Considerations

One pass, or while touching the file. Where a structural issue removes the code (100 for the
`unwrap`, 76 for the channel atomic), let it.

## Done when

`grep -n 'unwrap()\|expect(' --include=*.rs` outside `#[cfg(test)]` finds only justified cases with
a comment; no `unsafe` in the runtime.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, section 6).
- **2026-09-29** — The `c.result.unwrap()` in the agent actor is gone with
  [83](83-turn-logic-on-agent-actor.md).
- **2026-09-29** — Done in one pass. `unsafe` in `transport_impl/memory.rs` replaced by
  `transport::EventPredicate` (`for<'a> fn(&'a Envelope) -> bool`), and the buffer search no longer
  unwraps. The approver's timeout uses `to_std()` as the "already past" test. The component build's
  topological sort works on the descriptors themselves (no lookups by id, no `unwrap`, and the start
  order follows the configuration instead of a hash map). `digest::to_json` `expect`s with its
  justification; the cookie `expect` in `auth.rs` got a comment. `RuntimeManager::new`,
  `ChannelLoop::spawn_actors` and `SessionLoop::channel_by_name` no longer return a `Result`/are no
  longer `async`; the `MemoryStore` lock helpers return their value, and the state getters clone one
  entry instead of the whole map. `route_event_impl` takes the envelope by reference; the agent
  fetches its tools once per model call instead of per chunk; `McpToolProvider::strip_prefix` no
  longer formats (`NAME_SEPARATOR`). `DefaultToolExecutor::execute` logs tool and initiator as
  fields (the rest is on the turn span); Ollama logs the number of messages and the redacted
  endpoint instead of every id and the URL. Already gone before this pass: `handle_stopping`, the
  infallible controller constructors, `ChannelLoop`'s `Arc`ed state and atomic, the double clone in
  `record_turn` (one clone remains - transcript and history each keep their own copy). Left as they
  are, deliberately: `find_user_by_name` in both stores is a filtered `list` (so it needs the list
  permission, like the JSON store) - a lookup under the lock would change what it checks; and
  `unsafe` remains only in an Ollama test around `env::set_var`, which Rust 2024 requires.
