# 553 — Of a page fetched, only what was made of it can be kept

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Tools and approvals
**Summary:** `fetch_page` keeps at most the Markdown it made of a page, so the bytes the server sent - links, tables, what the conversion left out - are gone after the call, and whoever needs them has to fetch the page again, which may have changed.

## The problem

Issue [547](547-long-page-kept-whole-as-media.md) keeps a long page whole, but what it keeps is the
text the tool made of it: decoded by its charset, and, for HTML, converted to Markdown without
scripts, styles, navigation and what else `text::page` leaves out. The body as the server sent it is
dropped in `Client::fetch` (`caelumora-tool-web/src/component/web_fetch/client.rs:226`).

Issue [552](552-a-tool-result-has-no-bound.md) settled what keeping means: what a tool gave is
stored byte for byte, and everything after that is interpretation. By that measure the Markdown is
an interpretation, and the original of it is not kept.

## Why it matters

- What the conversion dropped cannot be got back: an agent that finds a table mangled, or needs an
  attribute or a link the Markdown lost, can only fetch the page again.
- A fetch again may give something else, the concern 547 was opened for.
- In the workspace, the original can be worked on with real tools (`copy_attachment`), where the
  Markdown can only be searched.

## Considerations

### When

- **Only where the page is kept anyway**, above `keep_whole_above_bytes`: no new parameter, and the
  many small pages are not stored twice. But a short page, where the conversion may have lost just
  as much, has no original.
- **On every fetch, or by a parameter of its own**: covers the short pages, at the price of storing
  every fetch for as long as the channel lasts, the reason 547 decided against keeping always.

### What exactly

- **The bytes of the body**, before `encoding.decode`, are the original. A page in another charset
  than UTF-8 then does not read as text: `read_attachment` says so by its bytes, and
  `copy_attachment` still reaches it. The charset the response named would be lost unless it is
  told beside it.
- **Only HTML**, since plain text and JSON are given as they are and the original would be a copy
  that differs at most in its charset.

### How it is given

Beside the Markdown, as one more entry of the tool's output, named `<host>.html`, as 552 gives the
text and the JSON of an MCP result as two attachments of one result. If a kind of attachment with
several representations of one thing is ever made, this is its second user, beside 552's and the
parts of issue [208](208-generated-artifacts.md). A page is at most 5 MiB, as the client refuses
larger ones. It is downloaded as `application/octet-stream`, as anything but an image, so a browser
never renders what anyone may have written.

## Done when

- Where it was decided that an original is kept, a fetched page of HTML has its body byte for byte
  beside its Markdown, reached with `copy_attachment` and, where it is UTF-8, `read_attachment`.
- TOOLS.md and COMPONENTS.md describe it.

## History

- **2026-11-09** — Opened at the maintainer's request, as a follow-up of issue
  [552](552-a-tool-result-has-no-bound.md), which it builds on.
