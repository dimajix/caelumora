# 600 — The REST API takes ids as bare UUIDs, and an id is made in two ways

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** The REST API reads the ids in its paths and in some request types as plain `Uuid`, though each names a kind with a type of its own (`SessionId`, `ChannelId`, `MessageId`, `ApprovalId`, ...), and across the workspace a new id is made as `XyzId(Uuid::new_v4())` beside `XyzId::new()`, which `define_id!` gives every id.

## The problem

Two places where an id is not yet what its type says, both found by the review of issue
[592](592-event-span-and-correlation-ids-are-bare-uuids.md):

- **The REST API.** About 95 extractors in `caelumora-rest-server/src/routes` read their ids as
  `Path<Uuid>` or `Path<(Uuid, Uuid)>` - of sessions, channels, messages, projects, groups,
  workspaces, attachments, agents - and turn them into the kind's id by hand
  (`MessageId(mid)`). In `caelumora-rest-api`, `SendApprovalRequest::approval_id`
  (`channel.rs`) and `CreateSessionRequest::template_id` (`session.rs`) are `Uuid`, though
  `ApprovalId` and `SessionTemplateId` exist. The actors in paths are typed already, by
  `ActorPath` (issue [590](590-a-session-lets-nobody-go.md), step 6a).
- **Two ways of making an id.** `define_id!` gives every id a `new()`; yet 24 places write
  `XyzId(Uuid::new_v4())` - among them `MessageId` in `caelumora-kernel/src/actor/user.rs` and
  `caelumora-agents/src/actor/external_agent.rs`, the rest mostly in tests
  (`caelumora-store/src/suite.rs`, `caelumora-domain/src/scope.rs`, `caelumora-store-json`).

## Why it matters

A channel's id passed where a session's is meant, in a handler, compiles; the extractor would
tell them apart for free. And a reader meets two ways of doing the same thing, of which one needs
`Uuid` imported only for it.

## Considerations

- **Paths:** `Path<SessionId>`, `Path<(SessionId, ChannelId)>` - `define_id!` derives
  `Deserialize` transparently, so the paths and what a client sends do not change. A malformed id
  is refused by the extractor as before, with problem details (`crate::error::Path`).
- **Not every `Uuid` is one kind's:** `AccessEntry::id` (`access.rs`) and the `entity_id` of
  what was read (`context.rs`) name an entity of the kind given beside them, and stay `Uuid`.
- **One change or two:** the second part is mechanical and touches tests across many crates; it
  may be a commit of its own.

## Done when

- No extractor and no type of the REST API takes as `Uuid` an id of one kind that has a type of
  its own.
- Nothing makes an id as `XyzId(Uuid::new_v4())`; `XyzId::new()` does.

## History

- **2026-11-18** — Opened from the review of issue
  [592](592-event-span-and-correlation-ids-are-bare-uuids.md), as the maintainer asked: its
  findings on ids in the REST API and on making ids, in one issue.
