# 194 — An agent's tools are a flat list, not grouped by the provider they come from

**Status:** in progress
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

## Decision and plan

Decided with the maintainer on 2026-10-05:

- **The prompt part is an optional `PromptProvider`.** The backends are given the tools natively
  (Ollama's `tools`), so the model needs no prompt to learn of them. The prompt part names at most
  what the native definitions cannot say: the tool sets that are unavailable (issue 32). A
  personality that does not configure the component is told nothing of them - accepted, since
  the component is the one place this is decided.
- **The record keeps the grouping:** what a message was offered is a set of tool sets by digest,
  so that the per-message view shows the same grouping as the view at the participant.
- **The view is at the participant**, not at the agent: an agent's components - and so its tools -
  are built per channel, for its actor there, with that channel's security context
  (`AgentActor::new`). What an agent can do exists at run time only as "this agent in this
  channel".

### Step 0 - free the name (no change of behaviour)

`AgentChannelState::tool_sets: Kept<Vec<ToolDefinition>>` and `AgentChannelStateChange::ToolSet`
(stored as `"tool_set"`) already mean "the tools a message was offered". They are renamed, e.g.
to `offered_tools` / `OfferedTools`; the stored tag `"tool_set"` stays readable
(`#[serde(alias)]`). A commit of its own.

### Step 1 - the type, in `caelumora-model/src/tool.rs`

One generic type rather than two:

```rust
pub struct ToolSet<T = Tool> {
    pub name: String,
    pub description: String,
    pub tools: Vec<T>,
    /// Why the source gives no tools now, in general terms - never the error, which may name
    /// hosts (issue 32). `None`: available.
    pub unavailable: Option<String>,
}

impl ToolSet<Tool> {
    /// The set as its model is offered it (issue 137).
    pub fn definition(&self) -> ToolSet<ToolDefinition>;
}
```

### Step 2 - `ToolProvider`

- `fn description(&self) -> String` - what its tools are for, written for a model.
- `fn unavailable(&self) -> Option<String>`, default `None`.
- The **name** is not the provider's: it is the id of its component, which the executor already
  holds (`ComponentImpl::id`), unique within the agent.
- The description is the provider's: built-in components write their own, worded for a tool set.
  Only a generic provider, whose tools it does not know itself, takes one from its configuration -
  for now `mcp_tool` (step 5). There is no common override for every component.

### Step 3 - executor and turn

- `ToolExecutor::tools()` becomes `tool_sets() -> Vec<ToolSet>`; `GuardedToolExecutor` likewise.
  `DefaultToolExecutor` builds each set from a provider and its component id.
- A turn begins with tool sets. `Turn::tools()` stays the flat view that approval, classification
  and `unknown_tool` need.
- The collision rule of issue 37 is unchanged: tool names stay unique across all sets.
- The model's backend is still given the flat list of `ToolDefinition`s.
- The record keeps `ToolSet<ToolDefinition>` by digest (`crate::digest`). A record written
  before - a flat list - is read as one set without name or description.

### Step 4 - the optional prompt part

A component, e.g. `tool_sets_prompt_provider`, implementing `PromptProvider`. It reads the tool
sets the work began with from `AgentWork` and names the unavailable ones ("The tools of
'github-mcp' are currently unavailable"); no text if all are available. Placed and recorded like
every prompt part. Whether it can optionally also list the sets' descriptions is left open until
a model is seen to need it.

### Step 5 - MCP

- `McpToolProvider` takes the description from the server's `instructions` in its `initialize`
  result, else a generic text naming the component; a parameter `description` of its
  configuration overrides both.
- A failed discovery sets `unavailable` instead of only logging. Retrying discovery is issue 126:
  since the executor asks its providers each time, a refreshed provider is followed without
  further change.

### Step 6 - view, REST and web UI

- The channel's views read the current tool sets of a participant from its running actor - the
  tools it would offer at the start of its next turn - each tool with its labels (visibility,
  classification, whether a call needs approval). A participant whose actor is not running shows
  as such rather than as one without tools.
- Who may see them is a new access question in `AccessQuestions`, with its test in the security
  suite.
- REST: `GET /sessions/{session}/channels/{channel}/members/{member}/tools`.
- Web UI: the tool sets at the member, where it is shown as a participant; the context panel of
  a message shows the offered tools grouped by set.

Order: step 0 alone, then 1-3 together, then 4, 5 and 6 each on their own.

## Done when

- Every `ToolProvider` gives its tools as a tool set with a name and a short description; MCP
  servers use their `instructions` where they send them, and a configuration can override it.
- The model is told the agent's tool sets with their descriptions and tools, recorded with the
  context like every other prompt part.
- The web UI shows an agent's current tool sets and their tools on the level of the agent, read
  through the REST API.

## History

- **2026-10-02** — Opened at the maintainer's request.
- **2026-10-05** — Plan decided with the maintainer: an optional prompt provider that names only
  unavailable tool sets, the record grouped by set, the view at the participant. Issue 32 is
  solved in steps 4 and 5.
- **2026-10-05** — Step 0 done: the record of what was offered is `offered_tools`. Steps 1 to 3
  done: `ToolSet<T>` in the model, `ToolProvider::description` and `unavailable`,
  `ToolExecutor::tool_sets`, the record keeps the sets (`digest::tool_sets`), and the context panel
  of the web UI groups the tools offered. The turn keeps its tools flat (`TurnState::tools`); the
  sets are in the record its context names. Decided with the maintainer: the description is the
  provider's, and only a generic provider such as `mcp_tool` takes one from its configuration
  (step 5) - no common override on the component descriptor or for every tool provider.
- **2026-10-05** — Step 4 done: the optional component `tool_sets_prompt_provider` names the
  unavailable tool sets the work began with (`AgentWork::tool_sets`). Reading the parameter
  `position` moved into `PromptPosition::of_params`, which its two other readers share.
- **2026-10-05** — Step 5 done: `mcp_tool` describes its tool set by its parameter `description`,
  else by the server's `instructions`, else by a text naming the component; a failed discovery
  makes the set unavailable, with why in general terms (`McpError::unavailable`). Issue 32 is done
  with it.
