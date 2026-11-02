# 527 — The approval strategy `DenyOnFirst` decides as `All` does

**Status:** open
**Type:** refactoring
**Priority:** low
**Area:** Tools and approvals
**Summary:** `ApprovalStrategy::DenyOnFirst` and `ApprovalStrategy::All` decide every request alike - approved once all who may approve have approved, denied by the first who denies -, so that a configuration offers two names for one rule.

## The problem

`ApprovalStrategy::decide` (`caelumora-domain/src/approval/mod.rs`) gives the two strategies the
same two conditions in a different order:

- `All`: approved once `approved_count >= user_count`, else denied once `denied_count > 0`.
- `DenyOnFirst`: denied once `denied_count > 0`, else approved once `approved_count >= user_count`.

The two conditions never hold at once: the answers are counted per approver (`responses` is keyed
by `ActorId`), so all approvers having approved leaves none who denied. The order of the checks
makes no difference, and the kernel's approver records the same answers as the decision's for both
(`PendingApproval::decision`). They differed while `All` and `DenyOnFirst` carried a
`user_count > 0` check of their own, which issue
[353](353-rules-of-the-model-computed-in-the-runtime.md) removed when a request without approvers
came to be denied before any strategy is asked.

Found in the review of the last step of issue [526](526-large-modules-of-the-domain.md), whose doc
comments now state the same rule twice.

## Considerations

- **Remove `DenyOnFirst`.** One rule, one name. It touches the enum and its tests, the web UI's
  type (`caelumora-web-ui/src/types/index.ts`, `ApprovalStrategy`), APPROVALS.md (the decision,
  and whose answers a decision names), and stored data that names it - templates, channels -,
  which has to be deleted (no compatibility code).
- **Give it a rule of its own.** If something else was meant - denied by the first who denies, but
  approved by the first who approves, say -, that is a new behaviour, to be decided on its own;
  issue [13](13-approval-modes.md) describes the strategies only as they are.

Recommended: remove it.

## Done when

- There is one strategy for "all approve, the first denial denies", and the documents and the web UI
  name only it.

## History

- **2026-11-02** — Opened at the maintainer's request, from the review of the last step of issue
  [526](526-large-modules-of-the-domain.md).
