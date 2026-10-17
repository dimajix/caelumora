# 290 — A model is offered tools it may not call where it is

**Status:** done
**Type:** feature
**Priority:** high
**Area:** Tools and approvals
**Summary:** A model is offered every tool of its components in every turn, also those it may not call where it is - a workspace's tools where there is none, or where its list grants reading alone.

## The problem

What a model is offered is fixed by its components alone: `ToolProvider::tools()` takes no
argument, and `unavailable()` neither - a provider answers the same for every turn, channel and
caller. Whether a tool can be called *here* is learnt only when it is called:

- `terminal_tool` offers its file tools and its shell in every channel. Where the channel does not
  use the session's workspace, or there is none, every call fails with "there is no workspace you
  can use in this channel".
- Since issue [23](23-acls-in-security-attributes.md), step 2, a workspace's access list may grant
  an agent reading alone: `write_file`, `run_command` and the other tools that change are still
  offered, and fail at the call with `ReadOnly`. The prompt part of the workspace says so
  beforehand, which a model may or may not heed.
- The same holds for any tool whose use depends on the place: a command that is for the agent's
  owners alone is hidden from who may not run it (issue 269) - a tool is not.

## Why it matters

- A model tries what it is offered. Every refused call costs a step of the turn and tokens, and a
  model that is told "you may only read" in the prompt but offered `write_file` gets two answers.
- "Who may not run it does not see it" is the rule commands keep (`ShowAgentCommand`); tools are
  the exception without a reason of their own.
- What is offered is recorded with the turn (issue 137): a record of tools that could never be
  called there says less than it seems to.

## Considerations

- **A turn-aware offering**: `ToolProvider::tools(&self, offered: &OfferContext)` - or a second
  method beside `tools()`, `available(&self, turn) -> Vec<&str>` - asked when a turn's tool sets
  are built, with what the provider needs: the turn's views, its channel, its context. Async, since
  deciding may read (a workspace's list, a channel's switch).
- **Where it is asked**: the runtime's `ToolExecutor` builds the tool sets a turn begins with
  (issue 137) - the one place to ask, as it already withholds a set whose remote side the channel
  may not flow to (issue 180, `destination`).
- **The call is checked anyway**: offering is a convenience for the model, never the guard - the
  call keeps its own checks (`WorkspaceView::terminal(Grant)`), since what was offered at the start
  of a turn may change while it runs.
- **What it costs**: every provider gains the method; most answer "all of them" by default. The
  tool sets recorded in a turn's context become per channel and caller, which they are in fact.
- **Alternatives**: two components for a workspace - one that reads, one that writes - chosen by
  the personality, which cannot follow a list that differs per workspace and changes; or a tool
  set marked unavailable as a whole (`unavailable`), too coarse for a set of which half may be
  called.

## Done when

- A provider can say which of its tools may be called in a turn, asked when the turn's tool sets
  are built; `terminal_tool` offers no tool where there is no workspace to use, and none that
  changes where the list grants reading.
- The call keeps its own check; a test shows both.
- TOOLS-AND-COMMANDS.md describes it.

## History

- **2026-10-10** — Opened at the maintainer's request with step 2 of issue 23, which agreed to
  offer an agent only the workspace tools its list grants and could not: `ToolProvider::tools`
  knows no turn.
- **2026-10-17** — Done. Decided with the maintainer: a turn is begun without its tools
  (`AgentChannelState::begin_turn`) and offered them right after, before its context is built
  (`offer_tools`), so that a provider is asked as a prompt part is - given the turn's work
  (`ToolProvider::offered(&AgentWork)`), not the `Turn`, since offering is a question. `offered`
  replaces `unavailable()`: `Err` with why is an unavailable set; `tools()` stays for finding a
  call's provider and the check of names. `ToolExecutor::tool_sets` takes the work; what an agent
  would be offered (`Actor::tool_sets`, the web UI) is asked of work in its own name with its
  stored state. `terminal_tool` offers no tool where there is no workspace to use, none that
  changes where its list grants reading and no shell tool where its storage gives none
  (`needs`, shared with the call's own check); `artifact_tool` is offered where a workspace is
  used and an artifact storage is configured. `AgentWork::workspace` serves them and the
  workspace prompt part. The `ToolExecutor` lives in `caelumora-agents` meanwhile, and
  TOOLS-AND-COMMANDS.md is `docs/TOOLS.md`, which describes it.
