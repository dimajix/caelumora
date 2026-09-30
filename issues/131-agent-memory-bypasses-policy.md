# 131 — Agent memory decides what may be recalled without asking the policy

**Status:** done
**Area:** Context and knowledge

## The problem

Private notes of other channels and `read_notes` ask the `SecurityManager`
whether the reading channel may read the source
(`AccessQuestions::may_channel_read`, `ActionType::ReadChannel`). Agent
memory does not: `AgentMemory::recalled` (`caelumora-runtime/src/agent_memory.rs`)
filters the entries itself with `ChannelClassification::readable_from`,
which is what the ownership policy answers - but only that policy. Found
while rewriting `docs/CONFIDENTIALITY.md`
([129](129-documentation-structure.md)).

## Why it matters

- A deployment's policy is meant to decide every `ReadChannel` question
  (issue 71: each question has one place, and no policy can answer it
  differently from the action). With another policy - a stricter one, or an
  external one ([24](24-opa-security-manager.md)) - agent memory would still
  recall by the built-in rule.
- The same kind of knowledge is guarded two ways, so a change to the rule
  has to be made twice.

## Considerations

- **Ask the policy in `recalled`**, as the private notes do. The question
  needs the origin's full classification, which each memory entry carries.
  `recalled` then becomes async and needs the security manager - through the
  turn or the `SessionReader`, whichever is the narrow handle for it.
- **Leave it to [65](65-generic-component-state.md)**, where component
  state is to be guarded by the core rather than per component. Keeps the
  divergence until then.

## Done when

- Whether an agent-memory entry may be recalled is decided by the
  `SecurityManager`, like every other `ReadChannel` question.

## History

- **2026-09-30** — Opened, found while documenting confidentiality (129).
- **2026-09-30** — Decided with the maintainer: neither the `SessionReader`
  (a workaround - the memory reads no other channel, it asks a pure access
  question) nor the `Turn` (which knows nothing of security), but
  `AccessQuestions` directly, reached through the `ComponentContext`.
  Done: `ComponentContext` carries the `SecurityManager`;
  `AgentMemory::recalled` and `forget` are async and ask
  `may_channel_read` in the component's `SecurityContext` - the agent's -
  once per channel the entries come from, judging the reading channel by
  the turn's classification. `may_channel_read` takes a `SecurityContext`
  like the other questions instead of building one from an owner, and every
  read across channels is now asked in the name of the agent's owner: the
  `ChannelSessionReader` is made with the actor's context
  (`ActorRuntime::session_reader(context)`) instead of asking as the
  reading channel's owner. A test with a policy that refuses `ReadChannel`
  to anyone but the owner shows that the policy decides.
