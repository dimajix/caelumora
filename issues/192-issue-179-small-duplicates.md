# 192 — Small duplicates left from issue 179

**Status:** done
**Area:** Maintenance
**Summary:** Small duplicates were left from issue 179 in the member views, the reader's place, "not found means none" and `unheard`.

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
- **2026-10-02** — Done, as decided with the maintainer. The member states were one already since
  issue 189: `MemberState` is implemented once, on `AgentMemberState`, and the two kinds differ only
  in reading their state. "Not found means none" is `StoreResultExt::optional` in
  `caelumora_model::store`, used at all seven places (`gone_is_none` and `not_found_as_none` are
  gone). `agent_memory.rs` takes the reader's place from the turn's viewer. `unheard` had become
  `unoffered` with issue 196 and no longer repeats `missing_from`; what it computed on the history
  is `MessageHistory::messages_after`. `member_state` and `Views::name_of` keep their own `match`:
  three lines that log differently, which the maintainer chose to leave.
- **2026-10-02** — An eighth "not found means none" was left in `seed.rs` (`missing`); with issue
  191 it became `.optional()?.is_none()`.
