# 109 — `unwrap`, `unsafe`, infallible `Result`s, idle locks and clones

**Status:** open
**Area:** Maintenance

## The problem

Against the rules in AGENTS.md, outside tests:

- **`unwrap`/`expect`:** `actor_impl/agent.rs:385` `c.result.unwrap()` (goes
  with [100](100-tool-executor-table-and-result.md)); `approval.rs:324`
  `.to_std().unwrap()`; `transport_impl/memory.rs:55` `remove(i).unwrap()`;
  `component.rs:215, 230, 245` (topological sort; provably safe, but
  `entry()`/`if let` cost nothing); `auth.rs:139`. `digest::to_json`
  (`digest.rs:164-166`) hashes `Null` on a serialisation failure without a
  word - in an integrity feature a justified `expect` is more honest than a
  silent fallback.
- **`unsafe`:** `transport_impl/memory.rs:49-50`, avoidable with
  `pub type EventPredicate = for<'a> fn(&'a Envelope) -> bool` in
  `transport.rs`.
- **Infallible `Result`, `async` without `await`:** `RuntimeManager::new`,
  `ChannelLoop::spawn_actors`, `handle_stopping`; `SessionLoop::channel_by_name`,
  `ControllerManager::new_agent_controller`, `SessionController::new`;
  `MemoryStore::read_lock`/`write_lock` return `StoreResult` but always
  `Ok`.
- **Locks and clones:** `ChannelLoop::state: Arc<Mutex<…>>` and
  `processing_state: Arc<AtomicU8>` are never cloned; `route_event` clones
  the whole envelope (`channel.rs:658`) though `route_event_impl` only reads;
  `record_turn` clones `turn_messages` twice (`agent_turn.rs:332-333`);
  `AgentActor::label` fetches `tool_executor.tools()` per streamed chunk;
  `McpToolProvider::strip_prefix` builds `format!("{}__")` per call.
- **Logging:** `DefaultToolExecutor::execute` (`tool.rs:198-205`) puts five
  ids into the message - they belong on the span; `ollama.rs:407` logs every
  message id of a request at `debug!` - size, not list.

## Why it matters

Each is small; the rules exist so that the set stays small.

## Considerations

One pass, or while touching the file. Where a structural issue removes the
code (100 for the `unwrap`, 76 for the channel atomic), let it.

## Done when

`grep -n 'unwrap()\|expect(' --include=*.rs` outside `#[cfg(test)]` finds
only justified cases with a comment; no `unsafe` in the runtime.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, section 6).
