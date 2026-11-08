# 552 — What an MCP tool gives back goes into the context whole, however long

**Status:** open
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

## History

- **2026-11-08** — Opened at the maintainer's request, split from issue
  [465](465-attachments-in-the-context.md), where the question had been recorded on 2026-11-06, and
  taking over the step left open in issue [547](547-long-page-kept-whole-as-media.md).
