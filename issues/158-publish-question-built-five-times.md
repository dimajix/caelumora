# 158 — Whether something may flow into a channel is asked in five places by hand

**Status:** open
**Area:** Identity and access management

## The problem

`ActionType::Publish { classification, actor, channel }` is built and
passed to `SecurityManager::check` in five places: `AgentActor::turn_flow`,
`GuardedToolExecutor::admitted`, `UserActor` when it sends, and twice in
`ChannelLoop` (`reclassify`, and the gate every message passes). AGENTS.md:
a pure access question is a method of `AccessQuestions`.

## Why it matters

A question asked by hand in five places is five places to change when it
changes - [138](138-confidentiality-policy-in-ownership.md) will move the
rule behind it.

## Considerations

- `AccessQuestions::may_publish(context, actor, classification, channel)`
  beside `may_channel_read` and `may_call_tool`; the five places call it.

## Done when

- `ActionType::Publish` is built only in `AccessQuestions`.

## History

- **2026-09-30** — From the review of the agent actor
  (`work/2026-09-30-code-review-agent.md`, A9), epic
  [151](151-agent-refactoring-epic.md). Beyond the review's subject, but
  found there.
