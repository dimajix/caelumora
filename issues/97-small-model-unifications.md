# 97 — Small duplications in the model and the runtime core

**Status:** open
**Area:** Maintenance

## The problem

Each of these is small; together they are the review's "same thing, two
shapes" list for the model and the runtime core:

- Six default functions for `Confidentiality::Public` (`channel.rs:235`,
  `template.rs:80/84`, `agent.rs:122`, `message.rs:98/102`, `tool.rs:101`).
- `ToolId` and `SessionTemplateId` hand-written instead of `define_id!`;
  `ActorId::Approver(Uuid)` instead of an `ApproverId` (hence
  `approver.id.0` in four places); the macro implements `Into<Uuid>` instead
  of `From`.
- Three shapes of "forbidden"/"not found": `StoreError::PermissionDenied`
  (unit), `SecretError::PermissionDenied(String)`, `ChannelError::Forbidden
  { action, channel }`; `StoreError::NotFound { entity, id }` vs.
  `SecretError::NotFound(String)`. `ChannelError` is constructed only by the
  runtime and belongs there.
- `Envelope::forward_with_response(m)` ≡ `forward_with_event(FullResponseGenerated(m))`.
- `ModeratorActors` uses `DashMap` behind `&mut self`; `add_actor` sorts by
  downcast although `ActorId` carries the answer; `remove_actor(Arc<dyn
  Actor>)` needs only the id; `replace_role` + `remove_tools` clone the
  messages twice.
- `FilteringEventSender::send` drops silently, `try_send` returns
  `Err(Filtered)`, which `route_event_impl` then filters out as the normal
  case (`channel.rs:702`).
- `SessionLoop` holds "my channels" three times (`channels: DashMap`,
  `tasks: Mutex<HashMap>`, `start_tx/rx`).
- `MemoryStore::read_lock`/`write_lock` return `StoreResult` but always
  `Ok` (30 `?` for nothing); `get_user` clones the whole map.
- `Message::prev_id` is left out of the digest "because the chain
  expresses it", yet `ollama.rs:377` and `agent.rs:397` set it - the field
  or the setters should go (issue [08](08-retry-and-edit-past-answers.md)
  forks by `MessageId`, not `prev_id`).

## Why it matters

None of these is worth its own issue; not recording them means finding
them again.

## Considerations

Do them in one pass or opportunistically while touching the file. Where an
open issue speaks (08 for `prev_id`), follow it.

## Done when

The list above is worked through; what was deliberately kept is noted here.

## History

- **2026-09-28** — From the structure review
  (`work/2026-09-28-code-review.md`, 3.2.8, 3.2.10, 3.4 F, section 6).
