# 77 — `ParticipantTemplate::instantiate` swallows store errors

**Status:** done
**Area:** Persistence

## The problem

`caelumora-model/src/template.rs:235`:

```rust
} else if let Ok(agent) = context.entity_store.find_agent_by_name(&context.security, name, &sc).await {
```

`Backend`, `PermissionDenied` and `NotFound` are treated alike: the branch falls through and creates
a second agent of that name.

## Why it matters

A store that is briefly unavailable, or a caller who may not read the agent, ends up with a
duplicate agent instead of an error - silently.

## Considerations

- Minimal: `match … { Ok(a) => a, Err(StoreError::NotFound { .. }) => create, Err(e) => return
  Err(e.into()) }`.
- Structural: issue [88](88-template-instantiation-in-the-model.md) moves the find-or-create out of
  the model; this fix should not wait for it.

## Done when

Only `NotFound` leads to creating the agent; every other error is returned. A test with a failing
store covers it.

## History

- **2026-09-28** — Found in the structure review (`work/2026-09-28-code-review.md`, section 1, error
  3).
- **2026-09-29** — Done with the minimal fix: `instantiate` matches the lookup and creates the agent
  only on `StoreError::NotFound`; every other error is returned. A test covers both: an agent is
  made once and found the second time, and a refused lookup (`FindAgent` denied) is an error instead
  of a second agent. The structural move out of the model stays with
  [88](88-template-instantiation-in-the-model.md).
