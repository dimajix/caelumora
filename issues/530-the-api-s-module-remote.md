# 530 — The API's module `remote` holds a cache and the description of errors

**Status:** done
**Type:** refactoring
**Priority:** low
**Area:** Code structure
**Summary:** `caelumora_api::remote` holds a cache of values kept for a time (`ExpiringCache`) and how an error's chain of causes is described (`describe_chain`, `describe_request_error`) - neither of them about a remote system -, under a name that `caelumora_domain::remote`, where an endpoint is, also bears.

## The problem

`caelumora-api/src/remote.rs` says it holds "what every component that talks to a remote system over
HTTP shares besides its endpoint". What it holds:

- **`describe_chain`**: the chain of an error's causes as one line - of any error.
- **`describe_request_error`**: the same for a failed request of `reqwest`, without the URL it
  writes into its messages.
- **`ExpiringCache`**: a value read and kept for a time, read again once it is outdated.

Who uses them talks to a remote system - MCP, A2A, Ollama, ComfyUI, the decision models, the open
terminal -, but what they are is a cache and the description of errors. And `remote` is also the
name of `caelumora_domain::remote`, where a remote system is reached (`RemoteEndpoint`,
`redact_endpoint`): two modules of one name that hold unrelated things.

Raised by the maintainer on 2026-11-02.

## Considerations

- **The cache** becomes a module of its own, `caelumora_api::cache`.
- **The description of errors** goes to `caelumora_api::error`, which already says how the API's
  errors are told to a person; a chain of causes told as one line is part of that.
- **Not into the domain**: `describe_request_error` needs `reqwest`, which the domain does not link,
  and the two descriptions belong together.

Decided: `cache.rs` and `error.rs`, and `remote.rs` is gone. A move without behaviour change; the
imports of its users (`caelumora-mcp`, `caelumora-a2a`, `caelumora-open-terminal`,
`caelumora-decision`, `caelumora-comfyui`, `caelumora-model-ollama`) and the documents that name
the module (CODING-GUIDELINES.md, PLUGINS.md) follow.

## Done when

- `ExpiringCache` is `caelumora_api::cache::ExpiringCache`, `describe_chain` and
  `describe_request_error` are `caelumora_api::error`'s, and no module `caelumora_api::remote` is
  left.

## History

- **2026-11-02** — Opened at the maintainer's request, with the cut agreed.
- **2026-11-02** — Done: `caelumora-api/src/remote.rs` is `cache.rs` (`ExpiringCache` and its
  tests); `describe_chain` and `describe_request_error` are in `error.rs`, whose module docs name
  them; the users' imports, CODING-GUIDELINES.md and PLUGINS.md follow.
