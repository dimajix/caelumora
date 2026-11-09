# 552 — What an MCP tool gives back goes into the context whole, however long

**Status:** in progress
**Type:** feature
**Priority:** medium
**Area:** Tools and approvals
**Summary:** The result of an MCP tool is put into the model's context whole, whatever its size, is sent again with every call of the turn, and cannot be read on or searched, while `fetch_page` can already keep what is too long for one result beside a preview.

## The problem

Issue [547](547-long-page-kept-whole-as-media.md) lets `fetch_page` keep a page too long for one
result as a text beside a preview, and left the form for every tool to issue
[465](465-attachments-in-the-context.md). That issue is about what of its attachments a model is
given along, a question of another kind, so this one takes the tools' results over from it.

Of the tools there are, these bound what they give, each in its own way: `read_file`,
`read_attachment` and `fetch_page` give a page at a time, `run_command` cuts its output at
`max_output_bytes` and keeps the end, and `search_web` gives a bounded number of results. An MCP
server is bound by nothing: `call_tool_result_to_value`
(`caelumora-tool-mcp/src/component/tool.rs:210`) takes the whole of its text or of its structured
content. A server that answers with megabytes fills the context, and what is cut off nowhere is
sent again with every call of the turn, until a compaction.

## Considerations

### Where the bound lies

- **A parameter of the component, as `fetch_page` has one.** `mcp_server` gets
  `keep_whole_above_bytes`, and the logic is shared with `web_fetch` rather than written twice: the
  preview, what is kept, the text that says how to read it. Small, and whoever sets it knows that
  the agent has `attachments` and that there is a storage of content.
- **In the `ToolExecutor`, for every tool**, as in Claude Code. One place, but a result is JSON
  whose fields the presentations of the web UI read, so it cannot simply be cut; it would have to
  apply to tools without a presentation of their own, or be told apart per tool.

MCP is the one case without a bound today, and `run_command`, which loses the beginning of a long
output, the only other that could gain from it.

### What is kept

A structured result is JSON, which a preview cuts in the middle; `content::media_type` takes it for
text. Whether it is kept as it is and previewed as text, or only text results are kept, is to be
decided. Images and resources of a result are issue [208](208-generated-artifacts.md).

### Which threshold

Bytes on the component, as for `fetch_page`, or a share of the model's context size, which is known
after an agent's first call (`AgentChannelState::used`, where a backend reports it), and which the
compaction reads. The same question for `fetch_page`, once it is answered here.

## Done when

- A result of an MCP tool above a configured bound is kept whole beside a preview and reached with
  `read_attachment` and `copy_attachment`; a result below it and a component without a bound behave
  as today.
- The logic is shared with `fetch_page`.
- TOOLS.md and COMPONENTS.md describe it.

## Decision

Decided with the maintainer: a parameter of the component, in bytes, as `fetch_page` has one. The
context size is no measure: only the backend of Ollama reports it, and not before the first call,
when a large result may already arrive; issue 465 chose a bound that needs no context size for the
same reason.

**Keeping is not interpreting.** What a tool gives is stored byte for byte; everything after that -
which label it gets, whether it can be read as a text, how a frontend shows it - is interpretation.
So `AttachmentManager::create_content` no longer refuses what is neither an image nor a text, as
`create` never did, and `is_media` and `NotMedia` go. What is left of their purpose moves to who
knows it: `generate_image` checks that it was given images, and `read_attachment` decides by the
bytes of an attachment whether it can read it, a valid UTF-8 being a page at a time, instead of by
its label. No control character is replaced, and no JSON indented, for what is kept.

**What of an MCP result is kept.** Above the bound, measured on what would go to the model:

- its text, if there is one, which is what the server sent;
- its `structuredContent` as well, as JSON, unless the text parses to the same JSON - the case the
  specification recommends, where it would only be a copy. Text alone would lose the data of a
  server whose text is a summary.

They are two ordinary attachments of the one result, `<component id>.txt` and `<component id>.json`,
named after the component and not after a remote tool, whose name need not be a file's. A kind of
attachment with several representations of one thing was weighed and left until there is another
user: `ToolCallResult::files` already holds what belongs to one result. The preview is made of the
text, without one of the JSON. A failure of the tool (`is_error`) is cut to the size of a preview
where a bound is configured, and not kept.

If keeping fails after the server acted - no storage, too large, a label no storage is cleared for -
the call is reported as failed all the same; whoever sets the bound knows that there is a storage.

**The names.** What a tool hands over is content, as in `ContentRef`, which it becomes:
`ToolMedia` is `ToolContent`, with `bytes`; `create_media` is `create_content`, `kept_media`
`kept_content`. The field on `ToolOutput` and `ToolCallResult` is `files`, since `content` is the
text of a result already. `media_type` stays: it is what `Content-Type` carries.

```rust
// caelumora-agents/src/tool
/// From how many bytes on a text is kept whole; refuses less than a preview when it is read.
#[serde(try_from = "usize")]
pub struct KeepWholeAbove(usize);

impl KeepWholeAbove {
    /// Whether `text` is above the bound.
    pub fn keeps(self, text: &str) -> bool;
}

impl ToolOutput {
    /// A preview of `text` as the value, and `text` whole as content named `name`.
    pub fn kept_whole(text: String, name: String, prompts: &PromptTexts) -> Self;
}
```

The text that says how the whole is read becomes `results.kept_whole` of the agents' catalogue and
names no tool of another library. `mcp_server` becomes `writing_for_the_model`, to have it.

What the original of a page fetched is, which this makes possible, is issue
[553](553-a-fetched-page-keeps-no-original.md). How a frontend shows control characters is no part
of this.

## Steps

1. The names above, without a change of behaviour.
2. What is kept is not judged (`is_media` goes, `generate_image` and `read_attachment` decide for
   themselves), and the logic of keeping moves from `web_fetch` to `caelumora-agents`.
3. `mcp_server` keeps what is above its bound.

## History

- **2026-11-08** — Opened at the maintainer's request, split from issue
  [465](465-attachments-in-the-context.md), where the question had been recorded on 2026-11-06, and
  taking over the step left open in issue [547](547-long-page-kept-whole-as-media.md).
- **2026-11-09** — Decided with the maintainer as above; steps 1 and 2 implemented.
