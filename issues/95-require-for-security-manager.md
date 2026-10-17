# 95 — Four ways to ask the `SecurityManager` for permission

**Status:** done
**Area:** Identity and access management
**Epic:** [74](74-refactoring-epic.md)
**Summary:** The runtime asks the `SecurityManager` for permission in four different ways with four different log lines.

## The problem

`SessionManager::may` (`session.rs:470-476`, error `StoreError`), `ChannelLoop::check`
(`channel.rs:381-387`, error `ChannelError::Forbidden`), inline in `AgentActor::message_context`
(`actor_impl/agent.rs:1171-1174`) and in `AgentActor::publication` (`317-326`). Log texts and levels
differ.

## Why it matters

Issue [71](71-access-helpers-on-entities.md) put the pure questions into `AccessQuestions`; the
*operations* that check as part of what they do should at least ask the same way, so that a refusal
reads the same in the log wherever it happened.

## Considerations

A `require(ctx, action, refusal: E) -> Result<(), E>` next to `AccessQuestions`, with one `info!`
pattern; the three callers pass their error type. The inline checks in `AgentActor` use it too.

## Done when

Every permission check in the runtime goes through one function.

## History

- **2026-09-28** — From the structure review (`work/2026-09-28-code-review.md`, 3.2.5).
- **2026-09-29** — Done, as agreed with the maintainer. There were more than four: besides the
  session, channel and agent, also user commands, the tool executor, the `SecretManager` and the
  store's `Access`. All of them now check with `Enforcement::require(context, action, refusal)` in
  `caelumora_model::security`, an extension trait on every `SecurityManager` next to
  `AccessQuestions`; each passes its own error. `SessionManager::may` and `ChannelLoop::check` are
  gone. Every refusal is logged alike by the new `ActionType::describe`, which names the action and
  what identifies its object but no content (a tool call by its function, not its arguments). The
  log level is `debug!`, not the `info!` the considerations named: some refusals are routine - a
  store answers "not found" for what its caller may not see, and a snapshot reads every channel of a
  session - and a refusal is detail of one operation, not a lifecycle transition. Two pure questions
  that called `check` directly moved to `AccessQuestions`: `may_publish` (the agent's `publication`)
  and `may_call_tool` (`ToolExecutor::is_permitted`). `check` is left to those two traits and to
  filters (a store's list, the event bus's dispatcher, the commands a user is shown). AGENTS.md
  names `require` for operations.
