# 584 — A tool has no object of its own

**Status:** in progress
**Type:** refactoring
**Priority:** medium
**Area:** Tools and approvals
**Summary:** A `ToolProvider` answers for all of its tools by their names - it carries a call out, asks its questions, proposes its matcher and describes it, each by a `match` of the name or a search of its list -, so what belongs to one tool is spread over the methods of its provider, as a command's was before issue 583.

## The problem

A component gives an agent tools through the facet `ToolProvider` ([TOOLS.md](../docs/TOOLS.md)):
its tool set's description (`description`), the `Tool` records of its tools (`tools`), those it
offers in a piece of work (`offered`), where their arguments go (`destination`), and, each by the
tool's name, what a call does (`execute`), what it asks a person first (`questions`), which calls
are like it (`matcher`) and how a call is described (`describe`). `ToolProviders` finds a call's
provider by searching the providers' tools for its name, each time, since a provider's tools may
change while the agent runs (issue 100).

A provider of several tools therefore matches the name in `execute` - `agent_memory` four names,
`notes`, `private_notes`, the workspace's file tools many more - and again in `questions` and
`matcher` where a tool has its own; and answers a name it does not have with
`ToolError::UnknownTool`, which no call reaches but through a mistake of the provider. What one
tool is - its record, its execution, its questions - lies in three or four places of its provider.
There are 25 providers outside the tests, in nine crates.

Issue [583](583-a-command-has-no-object-of-its-own.md) gives a command an object of its own, and
CODING-GUIDELINES.md (from 583) names a `match` that serves as a multiplexer a smell; tools are
the other place the rule is written from, and were left to an issue of their own for their size.

## Considerations

- **A tool as an object.** A trait `AgentTool` - its record (`Tool`), `execute(params, answers,
  turn)`, `questions`, `matcher`, `describe` with their defaults -, and a provider giving its
  tools as objects beside what concerns its set: its description, `offered`, `destination`. Built
  as 583 builds commands: one facet giving a list of executable things, one set finding them.
- **Tools that change.** A provider's tools are asked for at every call (issue 100) - an MCP
  server's list changes (issue 433) -, so the set is not made once as a command set is; what
  `ToolProviders` keeps stays the providers, asked for their tools as objects.
- **What stays the provider's.** The tool set is named after the component and described by it
  (issue 194), and what a provider offers in a piece of work is decided for its tools together
  (issue 290); neither moves to a tool.
- After 583, whose traits show the form; in steps by crate, the facet changing first.

## Done when

- Each tool is carried out, asked for its questions and its matcher by an object of its own, and
  no provider matches a tool's name.
- TOOLS.md and COMPONENTS.md describe it.

## History

**2026-11-15** - Opened with the maintainer while deciding
[583](583-a-command-has-no-object-of-its-own.md): tools follow commands in becoming objects of
their own, in an issue of their own for their size.

**2026-11-15** - Decided with the maintainer before the work began, following 583 rather than the
names above: the record of a tool is `ToolDescription` (was `Tool`), and `Tool` is the trait of a
tool in `caelumora_agents::tool` - `description`, `execute`, `questions`, `matcher`, `describe`,
each of one call -, as `Command` and `CommandDescription` are; a `ToolProvider` gives its tools as
objects, `offered` too, and selects them by what it holds each with, not by their names;
`ToolProviders` gives the tool it found, which the `ToolExecutor` asks. A provider that decides
nothing is `FixedTools` - its description, its tools and where their arguments go -, in place of
the components that only held tools. In three steps: the rename; every tool an object, in one step,
since the facet changing breaks every provider at once; what the review of the second comes across.

**2026-11-15** - Step 1 done: `ToolDescription` in `caelumora-domain`, the web UI's type of the
same name, in place of `Tool`; no behaviour changed.
