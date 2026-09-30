# 137 — What tools a model was offered is not recorded

**Status:** open
**Area:** Context & knowledge

## The problem

What an agent's model was given for a message can be told again: the
history records a `ContextRef` with every message - transcript, prompt
parts, knowledge read, the model's profile - and the `ContextResolver`
resolves it ([06](06-context-in-message-for-auditability.md),
[136](136-virtual-transcript.md)). The tools are missing from it.
`AgentActor::call_llm` passes them to `LLMClient::chat_completion` next to
the context (`self.tool_executor.tools()`), and they are recorded nowhere.

So it cannot be told afterwards which tools the model could call when it
produced a message, nor how they were described to it. The calls it made
and their results are in the transcript; the choice it had is not.

## Why it matters

- **Audit.** "Why did the agent not use tool X?" or "why did it call the
  tool with these arguments?" often has its answer in the offer: the tool
  was not there, or its description said something else then.
- **The offer changes.** A provider's tools are asked for on every model
  call (issue 100, `ToolExecutor::tools`), so they may differ between the
  messages of one turn - an MCP server replaced
  ([126](126-mcp-tools-discovered-once.md)), a server that failed to load
  ([32](32-unavailable-tools-invisible-to-model.md)), a skill loaded into a
  turn ([22](22-skill-management.md)).
- **Tool descriptions are prompt text**, and often the larger part of what
  a model is given: a context panel without them shows less than the
  model saw.

## Considerations

- **Kept like prompt parts.** The tool set of a model call is stored once
  by digest in the agent's state in the channel (next to
  `AgentChannelState::prompts`), and the `ContextRef` refers to it by that
  digest. Identical sets across calls and turns are stored once. Per tool
  rather than per set would store less when sets differ by one tool, at
  the price of a list of digests per reference.
- **What the digest covers:** what the model is sent - name, description,
  parameters - not `requires_approval` or `confidentiality`, which the model
  never sees. A fixed digest function behind a version tag, like
  `digest::message_chain` and `digest::prompt`.
- **When it is taken.** At every model call, since the offer may change
  within a turn: it belongs to the turn's context
  (`TurnState::context`) and so is copied into every message's reference
  as it stood.
- **REST and the context panel** would answer and show the tool set,
  perhaps collapsed; the web UI's types change with it.
- **The model itself.** Which model answered, with which parameters
  (`model`, `think`), is not recorded either - only the profile. It is the
  same kind of gap and could be recorded the same way, but is a question
  of its own: model and parameters are configuration, not something the
  model is given.
- The stored form of `ContextRef` changes, without serde defaults
  ([108](108-serde-defaults-for-legacy.md)).

## Done when

- For every message an agent produced it can be told which tools, as
  described to the model, it was offered.
- The context panel shows them.

## History

- **2026-09-30** — Issue created from the maintainer's question whether
  the whole context can be reconstructed from what is stored, after
  [136](136-virtual-transcript.md): messages, prompt parts, knowledge and
  profile can; the tools offered cannot.
