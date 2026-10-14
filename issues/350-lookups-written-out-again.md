# 350 — The same lookup is written out again at several places around channels and actors

**Status:** open
**Area:** Code structure

## The problem

A handful of values are fetched the same way, with the same error, at several places each:

- **The actor's channel, or that it is gone.** `UserActor` writes
  `self.actor_runtime.channel_loop().ok_or_else(|| anyhow!("Channel {} is gone", …))` eight times
  (`actor/user.rs:177`, `:222`, `:251`, `:273`, `:298`, `:325`, `:354`) and once as "no longer
  exists" (`:475`), once more as an `is_none()` check (`:138`).
- **The attributes of an agent as stored.** `controllers.agent_controller(id).await` followed by
  `.load().security_attributes.clone()` in `actor/user.rs:441`, `actor/approval.rs:76`,
  `channel/run.rs:314`; `channel/member.rs:53` reads the controller for the same reason. The
  comment "as stored - never as the answer says" is repeated with it.
- **Permission with the channel's attributes.** Five methods of `channel/control.rs` (`cancel`,
  `wakeup`, `cancel_actor`, `interrupt_tool_call`, `wakeup_actor`) and `reclassify` begin with
  `let channel = self.channel.load();` followed by
  `self.require(context, ActionType::X(&channel.security_attributes), "…")`.
- **"The author is an agent of this channel."** `UserActor::request_retry` (`actor/user.rs:300`)
  and `request_retry_of_turn` (`:326`) check `author.as_agent_id().is_none() ||
  !channel_loop.actor_ids().contains(&author)` and answer with two different errors
  (`NotAnAgentsMessage`, `NotAParticipant`); `ChannelLoop::admission` asks the same of a retry
  (`channel/run.rs:301`, `:307`).
- **The attributes of a session found.** `Found::session()` (`session/manager.rs:71`) clones the
  whole `Session`, and `stop_session`, `join` and `delete_session` call it for its
  `security_attributes` alone.

## Why it matters

Each repetition is one place more where the error text or the rule drifts: the two spellings of
"the channel is gone" already differ, and the retry check names two errors for one condition. A
reader of `UserActor` sees the same three lines before every operation instead of the operation.

## Considerations

- `UserActor::channel(&self) -> Result<Arc<ChannelLoop>>`, private, used by every operation; the
  `is_none()` check in `answer_questions` becomes a call of it.
- `ControllerManager::agent_attributes(id) -> StoreResult<SecurityAttributes>`, with the "as
  stored" comment once; the four call sites use it. `Member::of` keeps its controller, since it
  needs more than the attributes.
- `ChannelLoop::require` takes `impl FnOnce(&SecurityAttributes) -> ActionType<'_>` and loads the
  attributes itself; the six callers become one line each.
- `ChannelLoop::agent_actor(id) -> Result<Arc<dyn Actor>, ChannelError>` answering
  `NotAParticipant`; `request_retry` keeps `NotAnAgentsMessage` for a message said by no agent, as
  its doc promises, and asks `agent_actor` for the rest.
- `Found::attributes(&self) -> SecurityAttributes` beside `session()`, or instead of it where only
  the attributes are needed.

## Done when

- None of the five lookups above is written out at more than one place.
- No behaviour or error type changes; the tests of `UserActor`, the channel and the session manager
  pass unchanged.

## History

- **2026-10-14** — From the review of agents, sessions and channels, epic
  [349](349-core-review-epic.md).
