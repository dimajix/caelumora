# 192 — Small duplicates left from issue 179

**Status:** open
**Area:** Maintenance

## The problem

Found by a cleanup review of the issue-179 series, each small, none worth an issue of its own:

- `InternalAgentMember` and `ExternalAgentMember` (`view/member.rs`) implement `MemberState`
  (`name`, `introduction`, `agent`) and `AgentMember::attributes` identically; only the reads of
  their state differ.
- `member_state` and `Views::name_of` (still used by `SessionView`) each `match` over `Participant`
  with the same lookups for a user, an agent and an approver.
- `agent_memory.rs` builds the reader's `Place` twice by hand, though the views' `Viewer` already
  holds it (`viewer().place()`).
- "Not found means none" is written out by hand in five places (`gone_is_none`,
  `not_found_as_none`, the views, `user_roles`); one method on `StoreResult` would do.
- `unheard` clones the whole history and the whole transcript to find one id, and repeats what
  `missing_after` does.

## Done when

- Each of the above is in one place.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
