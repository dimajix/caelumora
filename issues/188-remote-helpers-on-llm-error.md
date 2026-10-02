# 188 — What every remote integration shares is not in `remote`

**Status:** open
**Area:** Integrations

## The problem

`crate::remote` was created in issue [179](179-external-agents-via-a2a.md) for "what every component
that talks to a remote system shares", but only the authentication moved there:

- `redact_endpoint` and `describe_chain` are methods of `LLMError`, and `a2a_agent` calls them
  there - remote HTTP helpers on the error type of the language model.
- `mcp_tool` puts the configured URL unredacted into `McpError::Connect`, `ListTools` and `Timeout`,
  against the rule that error messages contain no secret values: a URL may carry credentials.
- `A2aAgentConfig` and `McpToolConfig` are both `{ url, auth, timeout_secs }`, but only the first
  has `deny_unknown_fields`, its own `shown()`, and a different default timeout.
- `resolve_auth_header` in `mcp_tool` is a one-line wrapper around
  `RemoteAuthConfig::bearer_token`; `pub mod remote` and `pub enum RemoteAuthConfig` are used only
  inside the crate.

## Why it matters

The third remote integration will copy one of two conventions, and MCP errors can show credentials.

## Considerations

- Move `redact_endpoint` and `describe_chain` into `remote` (with `LLMError` delegating, or its
  callers changed) and use them in `mcp_tool` and `ollama_client` too - a mechanical step, except
  that the MCP error texts change.
- **A `RemoteEndpoint { url, auth, timeout_secs }`** with `shown()` and the bearer token, embedded
  by both configurations. `deny_unknown_fields` does not combine with `#[serde(flatten)]`, and
  adding it to MCP may reject existing seeds: a decision about the configuration contract.

## Done when

- Every remote integration redacts its endpoint and describes its errors through `remote`, and no
  error message carries a configured URL unredacted.

## History

- **2026-10-02** — Opened after a cleanup review of the issue-179 series.
