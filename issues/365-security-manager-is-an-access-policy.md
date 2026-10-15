# 365 — `SecurityManager` manages nothing: it is the access policy

**Status:** open
**Area:** Code structure

## The problem

`SecurityManager` (`caelumora-model/src/security.rs`) is a trait with one method,
`check(context, action) -> bool`: whether someone may do something. It holds no state and owns no
lifecycle. Everywhere else a manager is the one owner of the lifecycle of a kind - `AgentManager`,
`SessionManager`, `WorkspaceManager` ([364](364-managers-in-different-places.md)) - and
`SecurityManager` is the one name that means something else.

What it is, the code says already: the configuration describes its field as "The access policy;
`ownership` unless configured otherwise", the implementation is documented as "The ownership
policy". And its neighbour, asked beside it everywhere, is a policy by name: `ConfidentialityPolicy`
decides what may flow where, "apart from who may do what, which is the `SecurityManager`'s".

## Why it matters

A reader takes a manager for something that keeps and changes things, and looks in it for what it
manages. The pair the two policies are - who may do what, what may flow where - is hidden by two
naming schemes.

## Considerations

- **The names:**

  | Now | Then |
  |---|---|
  | `SecurityManager` | `AccessPolicy` |
  | `OwnershipSecurityManager` | `OwnershipPolicy` |
  | `DummySecurityManager` (allows everything; tests and development) | `AllowAllPolicy` |
  | fields, parameters, accessors `security_manager` | `access_policy` |
  | `SecurityConfig::create_security_manager` | `create_access_policy` |

  `AccessQuestions` and `Enforcement` stay as they are; their blanket implementations hold for every
  `AccessPolicy`.
- **Not taken:** `SecurityPolicy`. Confidentiality is security too, and the documentation keeps the
  two questions apart; `SecurityPolicy` would read as if it covered both. `AccessPolicy` says what
  it decides, and pairs with `ConfidentialityPolicy`.
- **Reach:** a rename only, but a broad one - `SecurityManager` occurs about 575 times in 134 files,
  `security_manager` about 120 times, in the model, the runtime, the REST server, the app and the
  plugins `caelumora-a2a`, `caelumora-artifacts`, `caelumora-mcp`, `caelumora-skills` and
  `caelumora-open-terminal`, and in the documents (SECURITY.md, PERMISSION-CHECKS.md, PLUGINS.md
  and others). A commit of its own, so that the diff can be reviewed as a rename.
- The configuration (`security: ownership`) does not change; no stored data and no configuration
  file has to be changed.

## Done when

- No type, field, parameter, method or document names a `SecurityManager` any more.
- No behaviour changes; clippy and the tests pass.

## History

- **2026-10-15** — Opened at the maintainer's request, under epic [349](349-core-review-epic.md):
  next to [354](354-names-follow-several-schemes.md), the other rename across the code, each in a
  commit of its own.
