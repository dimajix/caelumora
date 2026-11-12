# 553 — Of a page fetched, only what was made of it can be kept

**Status:** done
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

## Decision

Decided with the maintainer:

- **When.** On every fetch of a page asked for whole, not only above `keep_whole_above_bytes`,
  and only where it can be kept: where the agent's work could keep what the tool gives at
  `public` - a storage of content configured, the agent a participant of its channel, the storage
  cleared for it. Where it cannot, the page is given as before. A part asked for keeps nothing: it
  is fetched only after the page was asked for whole, and would store the same bytes again, since
  what is kept is not shared by its digest. Unlike the Markdown above the bound, which still fails
  the call where it cannot be kept: whoever sets the bound knows that there is a storage (issue
  552), while the original is kept without anyone asking for it.
- **What.** The body as the server sent it, before it is decoded, of HTML alone. The charset the
  response named is not told: nothing would read it, and HTML mostly names its own.
- **How it is given.** As one more ordinary entry of the tool's output, `<host>.html`, beside the
  Markdown, which is the value of the result or `<host>.txt` above the bound. No kind of attachment
  with several representations: `ToolCallResult::files` already holds what belongs to one result.
- **Who knows whether it can be kept.** The tool asks before, through its turn
  (`Turn::can_keep_content`), the agent's runtime (`ActorRuntime::can_keep_content`) and the
  attachments (`AttachmentManager::can_keep_content`), with the files it would hand over, the
  original among them. It checks what `create_content` checks before it keeps anything
  (`admit_files`, `storage_for`) - the caller, the number of the files, their names and sizes, the
  classification and the storage -, keeping nothing, so that a yes holds but for a storage that
  fails. The executor does not drop what cannot be kept: what a tool gives would
  then depend on how keeping went, and a preview would say the whole was kept where it was not.
- **Where the bytes come from.** `text::page` takes the body and its encoding, decodes it, and
  keeps the body of HTML on the `Page` (`original`), so that what is HTML stays known to `text`
  alone.

```rust
// caelumora-api/src/runtime.rs
/// Whether `create_content` would keep `files`, what a tool gives for a call the agent makes for
/// `initiator`, classified `classification`.
fn can_keep_content(
    &self,
    initiator: ActorId,
    classification: &Classification,
    files: &[ToolContent],
) -> bool;
```

## History

- **2026-11-09** — Opened at the maintainer's request, as a follow-up of issue
  [552](552-a-tool-result-has-no-bound.md), which it builds on.
- **2026-11-12** — Decided with the maintainer as above, and implemented. Beyond the decision: the
  security suite holds that `can_keep_content` asks what `create_content` asks (M4).
- **2026-11-12** — Changed with the maintainer in the review: `can_keep_content` takes the files,
  so that its yes is binding - the number, the names and the sizes were not asked before, and an
  original too large or one too many failed the call. `create_content` checks the sizes before it
  keeps any, too (`admit_files`). Done.
