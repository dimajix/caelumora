# 252 — Tool servers described by OpenAPI cannot be reached

**Status:** open
**Type:** feature
**Priority:** low
**Area:** Tools and approvals
**Summary:** A tool server that describes itself by OpenAPI - as most HTTP services and Open WebUI's tool servers do - cannot be reached; only MCP servers can.

## The problem

Remote tools reach an agent only through MCP: an `mcp_tool` component discovers a server's tools and
offers them. Most HTTP services do not speak MCP, but many describe themselves by an OpenAPI
document - internal services, public APIs, and the tool servers of the Open WebUI ecosystem, which
are OpenAPI servers by design (Open Terminal among them, issue [251](251-open-terminal.md); `mcpo`
even turns MCP servers into OpenAPI ones). To reach such a service today, someone has to wrap it in
an MCP server first, or write a component of its own.

## Why it matters

An OpenAPI document already says what an agent needs to call a service: its operations, their
parameters as JSON Schema, a summary and description per operation, and how to authenticate. A
generic component would make every such service a tool source by configuration, as `mcp_tool` does
for MCP servers.

## Considerations

### Alignment with `mcp_tool`

This is a second implementation of "a remote server whose tools are discovered", and it should
differ from `mcp_tool` only where the protocols do:

- **Configuration:** `endpoint` (`RemoteEndpoint`: the base URL, `auth`, `timeout_secs`),
  `requires_approval` (`true` by default, `false`, `"unless_read_only"`), `classification`,
  `description` - the same fields, the same defaults, the same `deny_unknown_fields`. Where the
  OpenAPI document lives (`spec_url`, by default `<base>/openapi.json`) is the one addition.
- **Names:** `{component_id}__{operationId}`, as MCP tools are named, so that servers cannot collide
  (issue [37](37-tool-name-collisions.md)). An operation without `operationId` gets a name derived
  from method and path, or is left out - to be decided.
- **Discovery:** once, when the component is built, as `mcp_tool` does; an unreachable server or an
  unreadable document makes the tool set unavailable with why in general terms (issue
  [32](32-unavailable-tools-invisible-to-model.md)). Discovering again is the same question as issue
  [126](126-mcp-tools-discovered-once.md) and should be answered for both.
- **Description:** the parameter `description`, else the document's `info.description`, else a text
  naming the component (issue [194](194-tool-sets.md)).
- **Shared code:** the `Approval` enum and the conversion of a JSON Schema into `ToolParameters`
  (`schema_to_tool_parameters`) live in `mcp_tool.rs` today and would be needed twice. They move to
  a shared place first, as a step of its own without behaviour change; `Approval` is needed by
  issue [251](251-open-terminal.md) as well, and moves with whichever comes first. Whether more is
  shared - a common "discovered remote tool set" the two protocols plug into - is worth asking once
  both exist, not before.

### What is new with OpenAPI

- **Which operations.** A service may have hundreds, most of them useless or dangerous to an agent.
  A filter is needed: `include` / `exclude` by `operationId`, or by tag. Whether all operations are
  offered when no filter is given, or none, is to be decided; offering all mirrors MCP, offering
  none is safer for large APIs.
- **Read-only.** MCP has `readOnlyHint`; OpenAPI has the HTTP method. `GET` and `HEAD` are safe by
  HTTP's definition, so `"unless_read_only"` could trust them as read-only - with the same caveat as
  for MCP (issue [246](246-mcp-tool-hints.md)): it is the server's word, trusted only where
  configured, and services that change state on `GET` exist. An extension (`x-...`) is not
  standard and would not be relied on.
- **Parameters.** An operation's path, query and header parameters and its request body have to
  become one object for the model, and be taken apart again for the request. Names can collide
  between them (a query `id` and a body field `id`); a body that is not an object (an array, plain
  text) needs a property of its own. Only JSON bodies at first; `multipart/form-data` (uploads)
  later or never.
- **Schemas.** OpenAPI schemas use `$ref` into `components/schemas`, which have to be resolved
  before conversion; cycles have to be cut. OpenAPI 3.0 and 3.1 differ in their schema dialect
  (3.1 is JSON Schema 2020-12; 3.0 has `nullable`). Swagger 2.0 is not supported. Which crate
  parses the document - one for 3.0, one for 3.1, or our own minimal reading of the parts we need as
  `serde_json::Value` - is to be evaluated.
- **Servers.** A document may name `servers`; the configured `endpoint.url` is the base and wins.
- **Authentication.** `RemoteAuthConfig` knows only `bearer`. API keys in a header of their own
  (`X-API-Key`) or in the query are common with OpenAPI; a variant `header { name, secret }` would
  serve all remote components, and is an extension of `remote`, not of this component.
- **Results.** A JSON body is the result; text is returned as text; anything else (binary, images)
  is lost until issue [208](208-generated-artifacts.md). A status of 4xx or 5xx is a failed call
  with the body's message, bounded in length, the endpoint redacted (issue
  [188](188-remote-helpers-on-llm-error.md)).
- **Interruption.** A dropped call drops its request (issue
  [161](161-tools-cannot-be-interrupted.md)); what the server does with a request it already
  received is up to it, as with MCP.

### Confidentiality

As for MCP: what comes back is classified as configured (`classification`, issue
[139](139-mcp-result-classification.md) for classifying per result), what goes out is unchecked
(issue [180](180-outbound-flow-unchecked.md)). Both issues should be answered for every remote tool
source at once, not for MCP alone.

### What it does not replace

A generic component gives every operation the meaning of "an HTTP call": no approval by what a call
does beyond the method, no `matcher` that understands an argument, no following of a long job. A
service where that matters - Open Terminal's shell (251) - is better served by a component of its
own; the generic one is for services whose operations are self-explanatory.

### Relation to issue 251

- **Shared:** the `Approval` enum (see *Shared code*); nothing else. The component of 251 talks to a
  fixed, known API through a small typed client and is not built on this one: the shape of its
  endpoints would otherwise be read from the document at run time instead of from Rust types, and
  it needs none of the hard parts here.
- **A bridge for Open Terminal.** Until 251 is done, this component can reach Open Terminal better
  than MCP does: with `"unless_read_only"` trusting the HTTP method, its `GET` endpoints read
  (`/files/read`, `list`, `grep`, `glob`, `/execute/{id}/status`) and need no approval, while `POST`
  and `DELETE` (`/execute`, `/files/write`, `replace`, `move`, `delete`, the kill) do. Its proxy
  `/ports/...` has to be filtered out, since it passes a `GET` on to any service running in the
  container - an example of why the filter of operations is needed. 251 would replace such a
  configuration, not use it.
- **Order.** If Open Terminal is the goal, 251 comes first, and this issue once a second, concrete
  OpenAPI service is to be reached - that service then decides the open questions here (the
  default of the filter, API keys in a header). If reaching many services quickly matters more,
  this issue comes first, with Open Terminal as its first test case.

## Done when

- An `openapi_tool` component offers the chosen operations of an OpenAPI 3.x service as tools, with
  the configuration, naming, discovery, approval and classification of `mcp_tool`, and shares its
  code with it where they do the same.
- It is documented in TOOLS-AND-COMMANDS.md (next to MCP servers) and COMPONENTS.md.

## History

- **2026-10-07** — Opened alongside issue [251](251-open-terminal.md), where a generic OpenAPI
  component was weighed as one way to reach Open Terminal and found to be worth having on its own,
  not as that way.
- **2026-10-07** — Added how this issue relates to [251](251-open-terminal.md): the shared step
  (`Approval`), this component as a bridge for Open Terminal with its `/ports` proxy filtered out,
  and the order of the two.
