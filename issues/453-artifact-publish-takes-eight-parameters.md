# 453 — Publishing an artifact takes eight parameters

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `ArtifactManager::publish` takes the caller, the content, its classification, the name, the description, the session and the channel one by one, and carries an `expect(clippy::too_many_arguments)`.

## The problem

```rust
pub async fn publish(&self, ctx: &SecurityContext, (size, content): (u64, ByteStream),
    classification: &Classification, name: &str, description: Option<&str>,
    session: SessionId, channel: ChannelId) -> ArtifactResult<(Artifact, u32)>
```

Counted with `self`, that is eight parameters; since issue 452 the method says so with
`#[expect(clippy::too_many_arguments, reason = "…")]`. The parameters form groups that travel
together, but nothing in the signature says so:

- **Where it is published, and by whom.** `ctx`, `session` and `channel` are, at the one caller
  (`artifact_tool`, `component.rs`), all read from `turn.situation()`: the agent and the channel
  its turn runs in.
- **What is published.** The content and its classification belong together: the doc comment
  says the caller vouches for the classification - that of the workspace the content was read
  from (`Turn::workspace_terminal`), never one a client names. That the two fit is a rule kept by
  the caller, not by a type.
- **What it is called.** `name` becomes a `ScopedName` in the session's scope inside `publish`;
  `description` goes with it.

## Considerations

- **Classified content as one value.** If reading a file of a workspace answered its content with
  the workspace's classification, the pair could not be put together wrongly, and `publish`
  would take one value. That touches the workspace's terminal, beyond the artifacts plugin - and
  is worth weighing for every other reader of a workspace's files that passes their
  classification on.
- **Where it is published.** `ctx`, `session` and `channel` come from the agent's situation. The
  manager could take what describes an agent's place in a channel - but `Situation` belongs to
  `caelumora-agents`, and the manager would then be bound to agents; a type of the runtime that
  names an actor's place (its `ActorRuntime` knows session and channel) may fit better.
- **Name and description.** Bundling only these two would be a type that only bundles parameters
  (AGENTS.md, "Few generic types, not many helpers"); they are worth a type only if it means more,
  such as the scoped name built by the caller.
- **Not a tuple of eight.** Whatever is chosen, the groups above are the candidates; a struct of
  all parameters would hide the problem, not solve it.

Done when `publish` takes the values in the groups that belong together, and its `expect` is gone.

## History

- **2026-10-23** — Issue created during step 1 of
  [452](452-guideline-rules-not-checked-by-lints.md), where the
  `allow(clippy::too_many_arguments)` on `publish` became an `expect`.
