# 107 — The public surface of the three crates is wider than their callers

**Status:** open
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
