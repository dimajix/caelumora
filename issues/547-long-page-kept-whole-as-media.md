# 547 — A page too long for one result is fetched again for every part of it

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Tools and approvals
**Summary:** A page of the web too long for one result is fetched anew for every part the agent reads, may change between the parts, and can be neither searched nor handed on, because a tool can keep nothing but an image beside its result.

## The problem

`fetch_page` (issue [544](544-web-access-tools.md)) gives a page 64 KiB at a time and keeps nothing:
every further part fetches the whole URL again. So the text may differ from part to part, a long
page costs a request per part, and an agent that wants to search the page, or work on it, can do
neither: all it has is what is in its context.

A tool can hand over more than its value, `ToolOutput::media` (issue
[512](512-comfyui-image-tool.md)), but `AttachmentManager::create_media` takes images alone and
refuses anything else with `NotAnImage`. Everything behind it would serve a text as it is: what a
result gave is numbered and noted after the result's text (`a3`), `Message::carried` finds it, so
that `read_attachment`, `copy_attachment` and `attach` reach it, and a text would only be noted,
never given along (`ModelProfile::gives_along` is about images).

## Considerations

### Always, or only for what is long

- **Always keep what was fetched.** One way for every page; but every fetch lies in the channel's
  storage for as long as the channel does, also the many small ones, and a page that would have
  fitted costs the model one call more, the concern of issue
  [465](465-attachments-in-the-context.md).
- **Only above a threshold.** A short page stays in the result as it is today. This is what Claude
  Code does: a result inline up to a size, above it the whole in a file with a preview and a
  reference (a command's output from about 30,000 characters on, an MCP result from 50,000, with a
  preview of 2,000); it never does so for a short result.

Keeping a page saves context only if what is given instead is small. So above the threshold the
result is a preview, not the 64 KiB of a part.

### Where the threshold is configured

- **A parameter of the component `web_fetch`**: per personality, so that a small model can be given
  less than a large one, as `max_output_bytes` of `workspace` is.
- **A section of `AppConfig`** (`attachments`): one value for every agent, whatever its model.
- **A share of the model's context window**: fits each model, but the window is not known
  everywhere; that is issue 465's to settle, for all tools.

### Whether the tool can tell that keeping helps

It cannot find out at run time. `read_attachment` is offered only once the transcript numbered an
attachment, so a look at the tools of the turn says no at the first fetch; a personality may lack
the component `attachments` altogether; and without a storage of content `create_media` fails, which
fails the call. So it is configured, by whoever knows the personality has the rest.

### One tool, or every tool

The generic form lies under the tools, in the `ToolExecutor`, as in Claude Code, and is what the
unbounded results of MCP servers need. It needs the bound of issue 465 first, and a result is JSON
whose fields the presentations of the web UI read, so it cannot simply be cut. It builds on the
same step: a tool may keep a text.

## Decision

Decided with the maintainer: only above a threshold, configured on the component, `fetch_page` the
first to use it, the generic form left to issue 465.

The one parameter says both whether and from where, so that no flag stands beside a number:

```rust
// WebFetchParams
/// From how many bytes of its text on a page is kept whole, and a preview given in its stead; left
/// out, no page is.
#[serde(default)]
keep_whole_above_bytes: Option<usize>,

// caelumora-domain/src/content.rs
/// Whether a tool may give its model this: an image, or a text it reads a page at a time.
pub fn is_media(media_type: &str) -> bool;
```

- `AttachmentManager::create_media` keeps its signature and asks `is_media`; `NotAnImage` becomes
  `NotMedia`, and what speaks of images alone says so no longer (`ToolMedia`,
  `ActorRuntime::create_media`, `ToolExecutor::keep`, TOOLS.md).
- A first call of `fetch_page` - no `start_line`, no `line_count` - whose text is above the
  threshold gives a preview of 4 KiB through `page_of`, as a constant of the tool, and the whole
  text as `ToolMedia`, named after the page's host (`<host>.md`), so that no name has to be made
  safe. Its result says that the whole is kept and how it is read, in a text of the catalogue.
- A text whose first bytes do not read as text (`content::media_type`) is not kept: it is given a
  part at a time as today. `start_line` goes on working in any case.
- `Params::check` of `web_fetch` refuses a threshold below the preview.
- A storage of content that is missing fails the call, as for every tool that hands over media: the
  threshold says that it is there.

The web UI needs no change: the card of a call already shows what a result gave, and `web_page` the
lines it covers.

## Steps

1. Issue [546](546-one-long-line-exceeds-a-page.md), since the preview goes through `page_of` and a
   long first line would pass it.
2. A tool may keep a text (`is_media`), and `fetch_page` does so above the threshold.
3. The form under the tools, with issue 465.

## Done when

- A page above the configured threshold is kept whole beside a preview, and `read_attachment` and
  `copy_attachment` reach it; a page below it and a component without a threshold behave as today.
- TOOLS.md and COMPONENTS.md describe it, and the example seed's `nora-researcher` sets the
  threshold, with its skill `research` telling how such a page is read.

## History

- **2026-11-08** — Opened at the maintainer's request after checking how `fetch_page` treats large
  results and how Claude Code does: designed as above, the threshold configurable at the
  maintainer's wish.
