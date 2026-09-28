# 95 — Four ways to ask the `SecurityManager` for permission

**Status:** open
**Area:** Identity & access

## The problem

`SessionManager::may` (`session.rs:470-476`, error `StoreError`),
`ChannelLoop::check` (`channel.rs:381-387`, error `ChannelError::Forbidden`),
inline in `AgentActor::message_context` (`actor_impl/agent.rs:1171-1174`) and
in `AgentActor::publication` (`317-326`). Log texts and levels differ.

## Why it matters

Issue [71](71-access-helpers-on-entities.md) put the pure questions into
`AccessQuestions`; the *operations* that check as part of what they do
should at least ask the same way, so that a refusal reads the same in the
log wherever it happened.

## Considerations

A `require(ctx, action, refusal: E) -> Result<(), E>` next to
`AccessQuestions`, with one `info!` pattern; the three callers pass their
error type. The inline checks in `AgentActor` use it too.

## Done when

Every permission check in the runtime goes through one function.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.5).
