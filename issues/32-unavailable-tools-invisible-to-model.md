# 32 — Tools that failed to load are invisible to the model

**Status:** open
**Area:** Tools & approvals

## The problem

An `mcp_tool` component whose server cannot be reached when the component is built starts with no
tools (`caelumora-runtime/src/component_impl/mcp.rs`, "starting with no tools available"). That is
deliberately not fatal — one unreachable server must not take down the whole agent — but the only
trace of it is a `warn` line in the log. The model is never told. From its point of view the tools
simply do not exist: it answers as if it had no way to, say, look at the repository, or it makes
something up.

[15](15-disallowed-tool-errors-to-llm.md) made every refused or failed tool *call* reach the model
as a structured result. This case is different: there is no call, because the model does not know
there is anything to call.

## Why it matters

"I can't do that" and "I could normally do that, but the GitHub tools are unavailable right now"
lead the user to very different conclusions. The first looks like a limitation of the agent, the
second like a transient problem someone can fix. Without it, a configuration or network problem
presents itself as the agent being less capable than it is.

## Considerations

- **Through the system prompt.** The model can only learn about missing tools from its context. The
  established way to contribute to it is a `SystemPromptProvider`, the pattern `create_plan` uses:
  `mcp_tool` would additionally implement it and contribute a line such as "The tools of
  'github-mcp' are currently unavailable" when discovery failed, and nothing otherwise. The catch is
  wiring: the component has to be listed in the `deps` of the context node, or
  `StateContextProvider` never asks it. A component that forgets this silently loses the note, which
  is the problem this issue is about in the first place.
- **Through the tool executor instead.** `DefaultToolExecutor` knows every tool provider. A provider
  could report its availability (`ToolProvider::unavailable() -> Option<String>`), and the agent
  could add the collected notes to the context at the start of a turn, without any configuration.
  That is a second way of contributing to the context next to `SystemPromptProvider`, which the
  architecture otherwise avoids.
- **Staleness.** Discovery runs once, when the component is built. A server that comes back later
  stays unavailable until the agent is rebuilt; one that goes away later is still listed. Whether
  discovery should be retried (on a timer, or on the first call) decides whether the note can ever
  be wrong, and belongs here too.
- **What it says.** The model needs to know *that* tools are missing and roughly which kind ("the
  tools of 'github-mcp'"), not the error. The error, which may name hosts, is in the log and — since
  [02](02-better-error-messages.md) — in `ComponentError`/`LLMError` style messages for humans.
- Users would benefit from the same information: a web UI that shows the agent's unavailable tool
  sources explains odd answers before anyone asks.

## Done when

- A model whose tool source failed to load is told so in its context, naming the source, without the
  error details.
- The mechanism works without per-component configuration, or the need for it is enforced rather
  than silently skipped.
- It is decided whether discovery is retried, and the note reflects the current state accordingly.

## History

- **2026-09-22** — Split off from [15](15-disallowed-tool-errors-to-llm.md), which covers tool calls
  that are refused or fail; this one covers tools the model never learns about.
