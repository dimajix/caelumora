# 480 — The context panel calls its own texts a refusal

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** User interaction
**Summary:** `refusal(error)` in the context panel turns three error codes into texts of its own, under the name the web UI now gives to every error a request is at fault for.

## The problem

Since issue [473](473-web-ui-repeats-the-refusal-check.md), a refusal is in
[WEB-UI.md](../docs/WEB-UI.md) what `reportUnlessRefused` tells apart: any status below 500 but a
`401`. `caelumora-web-ui/src/components/ContextPanel.tsx:30` has a function `refusal(error)` that
means something narrower - the codes `context_forbidden`, `context_not_recorded` and
`context_unavailable`, each turned into a fixed text -, and its doc comment already says what it is:
why a context is not shown.

## Why it matters

One word for two concepts: a reader of `ContextPanel` takes `refusal` for the rule of `errors.ts`,
and a change of either reads as a change of both.

## Considerations

- **A. Rename it after what it answers**, e.g. `whyNotShown(error)`.

## Done when

- `refusal` in the web UI means only what WEB-UI.md calls a refusal.

## History

- **2026-10-28** — Opened at the maintainer's request from the review of issue 473.
- **2026-10-28** — Done by option A: the function is `whyNotShown`. From the review, the comments
  that called any failure a refusal - of an attachment's upload (`attachmentDraft.ts`,
  `AttachmentDraftView.tsx`, WEB-UI.md) and of a file's download (`ToolCallView.tsx`) - say
  "failed" or "error" instead.
