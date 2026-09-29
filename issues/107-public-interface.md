# 107 — The public surface of the three crates is wider than their callers

**Status:** done
**Area:** Code structure

## The problem

The review counted, for every `pub fn` and `pub` type, the callers outside
its own file and outside tests. The result is three tables in
`work/2026-09-28-code-review.md` (3.3, 4.3, 5.3). In short:

- **No caller at all:** `Envelope::{forward, respond, with_event,
  with_security}`, `ActorEvent::from_message_event`, `ActorId::as_user_id`,
  `ChannelLoop::{actor_names, actor_by_name}`, `EventBus::{subscribe_all,
  subscribe_agent, subscribe_user}`, `EventSubscription::try_recv`,
  `InMemoryEventReceiver::prepend`, `EventReceiver::{drain_matching,
  take_buffered}`, `UserController::update_username`, `ApproverController`,
  `format_conversation`, `Digest::as_bytes`, `ToolId`, `STAGE_PLAN`/`STAGE_EMIT`,
  `SessionState` + four store methods, `Message::metadata`,
  `ToolExecutor::providers`, `ToolExecution`/`ExecutionStatus`,
  `MessageProcessor::name`, `ComponentContainer::empty`,
  `OllamaClient::with_think`, `Turn::transcript`, `ApiError::status`,
  `AppState.state_store`.
- **One caller, could be inlined or narrowed:** `SessionLoop::cancel_channel`
  ([102](102-rest-channel-lookup.md)), `SessionManager::load_session`
  (`open_session` checks `running`/`may` twice), `ChannelLoop::{channel,
  wakeup_actor, actor_ids}`, the `ChannelRuntime` delegators
  ([89](89-channel-runtime-delegators.md)), eight `ActorRuntime` methods used
  only by `actor_impl`, `RuntimeManager::{agent_states, new_moderator}`,
  `Turn::{has_plan, has_tool_calls, requested_state}` (peek + take → take),
  `Transcript::prefix` vs. `prefix_entries`, `Channel::new` (only fixtures),
  `Envelope::forward_with_response` (≡ `forward_with_event(FullResponseGenerated(m))`),
  `LLMError::describe_chain`, `viewer_id`, `may_read_channel` in REST,
  `TokenService::lifetime_secs`, the cookie and header constants.
- **Should be `pub(crate)`:** the half of `Turn` only `AgentActor` uses
  (`full_context`, `set_tool_calls`, `set_tool_approval_status`,
  `take_tool_calls`, `tool_calls`, `set_plan_progress`, `take_requested_state`,
  `next_turn`, `commit_turn`, `abandon_turn`, `send_event`, `event_context`,
  `saw_private_knowledge`); `McpError`/`McpResult`/`McpAuthConfig`/`McpToolConfig`;
  the `*_result` helpers in `tool.rs`; `parse_plan`, `PLANNING_SYSTEM_PROMPT`.
- **Signature shape:** `RuntimeManager::new -> Result` is infallible;
  `Actor::wakeup`/`cancel` return `Result` and are `async` but never fail or
  await (three `?` in `channel.rs` and a REST error path exist for that);
  `SessionLoop::channel_by_name`, `ControllerManager::new_agent_controller`,
  `SessionController::new` are `async` without `await`;
  `SessionLoop::get_session` and `SessionManager::get_session` share a name
  and mean different things; `Turn::agent_id`/`agent_name` are `pub` fields
  next to getters for everything else; `store.rs:103` has `pub mod tests`.

## What stays public on purpose

`SecurityContext::delegation_ids` (issue 12), `User::roles` and
`Scope::GroupScope` (23), `Recorder` (05), `ToolParameters::boolean` and
`ToolConfidentiality::CALL_ONLY` (complete vocabulary), `LLMError::redact_endpoint`
(named in AGENTS.md), `parse_input` (shared with the TUI), the component
API of `Turn` (65), `SessionReader` (65, 59, 18), `EntityStore` user methods
(25), everything a frontend calls.

## Why it matters

A `pub` that nothing calls is a promise the code does not need to keep, and
a reader cannot tell which half of `Turn` is for components.

## Considerations

One pass per crate, after the structural issues that remove or move the
items anyway (83, 89, 100, 102). Items with zero callers go first; the
`pub(crate)` pass follows; the signature changes last, because they touch
callers.

## Done when

Every `pub` item in the three crates has a caller outside its file or a
comment saying why it is public.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.3, 4.3, 5.3).
- **2026-09-29** — `ToolId` removed with [112](112-id-types.md).
- **2026-09-29** — The actor-only half of `Turn` is `pub(crate)` with
  [83](83-turn-logic-on-agent-actor.md); `Turn::take_requested_state`,
  `set_tool_calls`, `take_tool_calls` and `last_message` are gone
  ([85](85-turn-state-in-agent-channel-state.md), 83).
- **2026-09-29** — Signature shape. Already gone with earlier issues:
  `RuntimeManager::new` can fail now (`verify`, `set_defaults`), so its
  `Result` stays; `Actor::wakeup`/`cancel`, `SessionLoop::channel_by_name`
  and `SessionController::new` are synchronous; `new_agent_controller` and
  `SessionManager::get_session` no longer exist. Changed now:
  `SessionLoop::get_session` is `session()`, and `ChannelLoop::channel()`
  returns the `arc_swap::Guard` like it instead of a copy - callers that
  need an owned `Channel` or keep it across an `await` clone it themselves;
  `Turn::agent_id`/`agent_name` are private fields behind getters; the tests
  in `caelumora-model/src/store.rs` are a private `mod tests`.
- **2026-09-29** — No caller at all. Already gone: `SessionState` and its
  store methods, `ToolExecution`/`ExecutionStatus`, `ToolExecutor::providers`,
  `MessageProcessor::name`, `UserController::update_username`,
  `ApproverController`, `AppState.state_store`; `ActorId::as_user_id` has a
  caller now (`SessionManager::create_from_template`). Removed now:
  `Envelope::{forward, respond, with_event, with_security}`,
  `ActorEvent::from_message_event`, `format_conversation`,
  `Digest::as_bytes`, all three `STAGE_*` constants
  (the stages are facets since [99](99-component-factory-and-tags.md)),
  `ChannelLoop::{actor_names, actor_by_name}`,
  `EventReceiver::{drain_matching, take_buffered}`,
  `InMemoryEventReceiver::prepend`, `OllamaClient::with_think`,
  `ComponentContainer::empty`. Left as they are, by decision:
  `EventBus::{subscribe_all, subscribe_session, subscribe_channel,
  subscribe_agent, subscribe_user}` and `EventSubscription::try_recv` - the
  bus is not touched in this issue - `Turn::transcript`, which only a
  test calls, and `Message::metadata`, which stays part of the message.
- **2026-09-29** — One caller. Already gone: `SessionLoop::cancel_channel`
  ([102](102-rest-channel-lookup.md)), `SessionManager::load_session`,
  `Turn::has_tool_calls`, `viewer_id` and `may_read_channel` in REST.
  Changed now: `Envelope::forward_with_response` is gone and
  `forward_with_event` is `forward(event)`, the one way to pass an envelope
  on; `Transcript::prefix` returns the entries, like `Chain::prefix`, and
  the copying variant only tests used is gone; `Channel::new` is inlined
  into `Channel::build`, its only caller outside of tests - every other
  channel, like every session, is built as a literal. Kept, for symmetry
  or by decision: the `ChannelRuntime` delegators (rejected in
  [89](89-channel-runtime-delegators.md)), `RuntimeManager::{new_moderator,
  new_recorder, agent_states}` among their siblings, `ChannelLoop::{wakeup,
  wakeup_actor, actor_ids, channel}`, `Turn::requested_state` next to
  `request_state`, `LLMError::describe_chain` next to `redact_endpoint`,
  and the REST server's constants and `TokenService::lifetime_secs` (a
  binary crate, where `pub` means the crate anyway). The `ActorRuntime`
  methods belong to the `pub(crate)` pass.
- **2026-09-29** — Should be `pub(crate)`, conservatively. Changed: the
  MCP types (`McpError`, `McpResult`, `McpAuthConfig`, `McpToolConfig`),
  `parse_plan` and `PLANNING_SYSTEM_PROMPT` are private to their files;
  `unsuccessful_result` is private, the results the runtime produces itself
  (`denied_result`, `timed_out_result`, `not_executed_result`,
  `forbidden_result`) are `pub(crate)`. Kept: `successful_result`, which a
  `ToolExecutor` component needs ([127](127-tool-executor-not-replaceable.md));
  the rest of `Turn`, the component API (65), although `context`,
  `send_event`, `messages` and `transcript` have only actors as callers and
  `session_id` none. The `ActorRuntime` methods only actors use are left
  to [128](128-actor-runtime-in-components.md): components reach them
  through `ComponentContext::actor_runtime`, which is a question of what a
  component may do.
- **2026-09-29** — Done: every item of the four lists is changed, kept
  with its reason above, or handed to [128](128-actor-runtime-in-components.md).
