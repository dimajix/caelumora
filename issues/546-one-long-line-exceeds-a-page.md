# 546 — A single long line goes past what a page of a text holds

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** Tools and approvals
**Summary:** A page of a text holds at least one whole line whatever its length, so a text with one long line - a minified JSON a fetch gave, a generated file - reaches the model whole, past the bound of 64 KiB, up to 5 MiB through `fetch_page`.

## The problem

`page_of` (`caelumora-agents/src/tool/page.rs`, issue [282](282-message-attachments.md)) cuts a text
at line ends: as many whole lines as `max_bytes` holds, *one at least*. The first line of a page is
taken whatever its length, because a page without it would not get anywhere:

```rust
if content.len() + line.len() > max_bytes && end >= start {
    break;
}
```

So the bound holds only for a text whose lines are short. All three of its users are affected:

- `fetch_page` (issue [544](544-web-access-tools.md)) gives plain text and JSON as they are. A
  minified JSON document is one line, and a response is accepted up to 5 MiB
  (`MAX_BODY_BYTES`): up to 5 MiB in one result.
- `read_file` (`max_output_bytes`, 64 KiB unless configured): a minified bundle, a generated file,
  a log written without line breaks.
- `read_attachment`: an attachment of up to `attachments.max_bytes`, 50 MiB unless configured.

Nothing behind it catches this: `max_output_bytes` belongs to the `workspace` component, and the
`ToolExecutor` puts no bound on the value a tool gives.

## Why it matters

- **The context.** One such result can fill the context of a model; the call fails or the
  compaction has nothing left to work with.
- **Every call sends it again** (see issue [465](465-attachments-in-the-context.md)): until a
  compaction leaves it out, the result is part of every call of the turn and of the turns after it.
- **It is not the agent's to avoid.** Nothing tells it beforehand how long a line is, and
  `line_count: 1` is exactly the call that gives the whole text.
- **Text from anyone.** Through `fetch_page`, whoever serves the page decides how large the result
  is, up to the limit of the fetch.

## Considerations

The difficulty is that a page is addressed by lines (`start_line`, `line_count`, `next_line`), and
the rest of a line that was cut has no line number.

- **Cut the line and address within it.** A page may end inside a line, and the result says where
  to read on: a `start_column` beside `start_line`, or a byte offset. Exact, and the numbers remain
  those of the text, which `read_file` needs: its lines are what `grep_files` and an edit refer to.
  It costs a parameter on all three tools, and `TextPage`, the read-on text and the web UI's
  `pageLines` learn of a page that begins or ends inside a line.
- **Count a long line as several.** `page_of` treats every `max_bytes` of a line as a line of its
  own, so that nothing about a page changes. No new parameter, but the numbers are no longer those
  of the text wherever a line is long, and they depend on `max_bytes`, which for `read_file` is
  configured; a line number from `read_file` would disagree with `grep_files`.
- **Cut the line and say so, the rest unreachable by this tool.** The simplest: at most `max_bytes`
  of a line, with a mark that it was cut. Enough where nobody reads such a line anyway; for
  `read_file` the agent still has the shell. For `fetch_page` and `read_attachment` the rest would
  be lost.
- **Refuse.** The call fails, naming the line and its length. Bounds the result, but a JSON document
  that is one line could not be read at all.

Cuts fall on a character boundary, as `content` is a string.

Whether a large text should be kept apart from the result at all, with a page of it given to the
model, is the question of issue 465 (its history of 2026-11-06), and with it of `ToolOutput::media`
(issue [512](512-comfyui-image-tool.md)). It does not replace this issue: a page of such a text is
still given through `page_of`.

## Done when

- No result of `page_of` holds more than its `max_bytes`, however long a line is.
- What was left out of a long line is said, and where it can still be read, how.
- `fetch_page`, `read_file` and `read_attachment` are covered by tests with a line longer than a
  page; TOOLS.md describes it.

## History

- **2026-11-08** — Opened at the maintainer's request, found while checking how `fetch_page` treats
  large results. Found by reading the code; not reproduced against a running server.
