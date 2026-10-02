# 194 — An agent's tools are a flat list, not grouped by the provider they come from

**Status:** open
**Area:** Tools and approvals

## The problem

Every tool of an agent comes from a `ToolProvider` - a built-in component, an MCP server, an
external agent. The provider already groups its tools by subject: the tools of one MCP server
belong together, `remember` and `forget` belong together. But the grouping goes no further than the
provider:

- the model is offered one flat list of `ToolDefinition`s. It is not told which tools belong
  together or what a group is for, only what each single tool does;
- a provider has no description of its own, only its tools have one, so there is nothing to tell
  the model or the user even if the grouping were kept;
- the web UI shows tools only per message: the context panel lists the tools offered for that one
  message (issue 137). There is no place that shows what an agent can do *now* - its tool sets
  and their tools as they are currently available.

Wanted, from the maintainer: every `ToolProvider` has a short description of its tools as a *tool
set*; the grouping is reflected in what the model is told of its tools; and the web UI shows an
agent's current tool sets with their tools on the level of the agent, not of a message.

## Why it matters

- A model chooses better among many tools when it knows the purpose of a group ("tools of the
  issue tracker of project X") than from tool descriptions alone, which are written for the tool
  and rarely say in which context it applies. This grows with MCP servers, which bring many tools
  each.
- A user who wants to know what an agent can do today has to find a message and open its context.
  What an agent can do is a property of the agent, and changes over time (MCP tools, issue 126).

## Considerations

- **Where the description comes from.** A method of `ToolProvider`, e.g.
  `fn tool_set(&self) -> ToolSet` with a name and a description, so that every provider gives one
  the same way. Defaults and sources:
  - the name defaults to the component's name, which is unique within the agent;
  - built-in components write their own description;
  - an MCP server may send `instructions` in its `initialize` result - what the server says about
    itself, written for a model - which is the natural description of its tool set;
  - the component's configuration may override the description, as for any configured text.
  Whether `ToolSet` is a type of its own or the `ToolProvider` answers name and description
  separately is to be decided; a type is needed anyway once the set travels to the model, the
  record and the UI, and belongs into `caelumora-model`, beside `ToolDefinition`.
- **There is no tool prompt yet.** The backends are given the tools natively (Ollama's `tools`),
  which has no notion of groups. How the grouping reaches the model:
  - *A prompt part* listing every tool set with its description and the names of its tools. It is
    built along the existing pattern (`PromptProvider`, placed and recorded like every prompt
    part, so the record of what the model was given stays complete). Who provides it is a question
    of ownership: it needs all providers, which the agent's tool executor knows
    (`GuardedToolExecutor`, issue 127) - a prompt part of the tool machinery itself rather than of
    one component.
  - *A prefix on the tool's name or description* (`issues.create`). Groups without a prompt part,
    but changes the names the model calls, interferes with the collision rule (issue 37) and with
    configured tool names, and repeats the set's description on every tool.
  Recommended: the prompt part. Only the set's description and the tool names go into it - the
  tools themselves stay native definitions, nothing is described twice.
- **What is recorded.** What a message was offered is recorded as a set of `ToolDefinition`s by
  digest (issue 137). Whether the record keeps the grouping (`ToolSet` with its definitions) or
  stays flat with the grouping visible only through the prompt part is to be decided; keeping it
  lets the per-message view show the same grouping as the agent-level view.
- **The agent-level view.** A REST endpoint for an actor's current tool sets, each with its tools
  and their labels (visibility, classification, whether a call needs approval), as the agent would
  offer them at the start of its next turn. Who may see it is an access question
  (`AccessQuestions`), like the agent's other properties. The web UI shows it where an agent is
  shown as a participant, not at a message.
- **Current, not offered.** "Currently available" depends on issue 126 (MCP tools discovered
  once) and issue 32 (a provider that failed to load leaves no trace): a tool set is the natural
  place to show that a source is unavailable, and why - to the user in this view, to the model in
  the prompt part. Whether the view is refreshed by an event when the tools change, or read on
  demand, follows from 126.
- **Presentation.** Issue 165 adds a presentation to a tool for the UI; a tool set may later carry
  one as well (e.g. an icon), but that is not part of this issue.

## Done when

- Every `ToolProvider` gives its tools as a tool set with a name and a short description; MCP
  servers use their `instructions` where they send them, and a configuration can override it.
- The model is told the agent's tool sets with their descriptions and tools, recorded with the
  context like every other prompt part.
- The web UI shows an agent's current tool sets and their tools on the level of the agent, read
  through the REST API.

## History

- **2026-10-02** — Opened at the maintainer's request.
