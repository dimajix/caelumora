# 114 — Moderator internals: a `DashMap` behind `&mut self`, a downcast for a known kind, two clones per message

**Status:** done
**Area:** Maintenance
**Epic:** [74](74-refactoring-epic.md)
**Summary:** `ModeratorActors` uses a concurrent map behind `&mut self`, downcasts for a kind the id already tells, and clones every message twice.

## The problem

`ModeratorActors` (`caelumora-runtime/src/moderator_impl/mod.rs:99-101`) uses a `DashMap` although
every access is through `&mut self` and the channel already serialises the moderator behind
`Mutex<Box<dyn Moderator>>`. `add_actor` (`113-124`) sorts actors by downcasting to `UserActor`/
`ApproverActor`, although `ActorId::User/Agent/Approver` carries the answer. `remove_actor(Arc<dyn
Actor>)` needs only the id (`moderator.rs:81`). `replace_role` + `remove_tools` (`134-135`) clone
the messages twice.

## Why it matters

None of it is wrong; each is a reader's question ("why concurrent?", "why a downcast?") with no
answer in the code.

## Considerations

`HashMap`; match on `ActorId`; `remove_actor(ActorId)`; one pass that rewrites role and strips
tools. One file.

## Done when

`ModeratorActors` has no concurrent map, no downcast, and one pass over the messages.

## History

- **2026-09-28** — Split off from [97](97-small-model-unifications.md) (structure review, 3.2.10).
- **2026-09-29** — Done. `ModeratorActors` keeps plain `HashMap`s, files an actor by its `ActorId`
  instead of downcasting it (an id that is neither user, agent nor approver is logged and not
  routed), and `Moderator::remove_actor` takes the id. `replace_role` and `remove_tools` are one
  `as_user_messages`, one clone per message. On the way: the old order - roles replaced first, then
  tool messages filtered by role - could never filter by role, since every message was a user's by
  then; only the empty content kept tool messages out. The one pass filters them by role as
  intended. A test states what the others hear.
