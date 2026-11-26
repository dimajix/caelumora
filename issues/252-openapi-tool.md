# 252 — Tool servers described by OpenAPI cannot be reached

**Status:** in progress
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

## What exists

As of 2026-11-26, since this issue was opened, the remote tool sources have changed in ways that
settle several of the points below (see [TOOLS.md](../docs/TOOLS.md#mcp-servers)):

- **MCP is a library of its own.** The component kind `mcp_tool` is now `mcp_server`, in the crate
  `caelumora-tool-mcp` with the library `McpToolLibrary`
  ([325](325-mcp-in-a-crate-of-its-own.md)), which `caelumora-app` and `caelumora-tui` list. What
  such a kind may use of the kernel is said in
  [PLUGINS.md](../docs/PLUGINS.md#what-a-component-kind-outside-the-kernel-may-use). 325 names this
  issue as the next one of that shape.
- **Approval is shared.** `ApprovalMode` (`true`, `false`, `"unless_read_only"`) is in
  `caelumora_agents::tool` and read by every component with a `requires_approval`; the step of
  moving `Approval` first is done.
- **The schema conversion is not.** `schema_to_tool_parameters` is still private in
  `caelumora-tool-mcp/src/component/tool.rs`. It resolves no `$ref` and reads no `type` given as a
  list (`["string", "null"]`): either becomes a string parameter described as "Unrecognized
  parameter schema", and the values of an `enum` are dropped - for MCP's input schemas too, which
  may use `$defs`.
- **Discovery is lazy and repeated** ([126](126-mcp-tools-discovered-once.md)): the tools are
  discovered when they are first offered (`ToolProvider::offered`), in the name of whoever acts
  there and with the secrets of their situation
  ([516](516-the-runtime-trusts-the-agents-linked-into-it.md)), never while the component is built.
  What was discovered is kept in an `ExpiringCache` (`caelumora_api::cache`) for the endpoint's
  `refresh_secs`, and discovered again after that, or after a call of a tool the server did not
  know. A server that cannot be asked leaves the tools discovered before, or, without any, its tool
  set unavailable, with why in general terms ([32](32-unavailable-tools-invisible-to-model.md)).
- **What goes out is checked** ([180](180-outbound-flow-unchecked.md)): `destination` says what a
  server is cleared for, public unless configured, and its tools are not offered in a channel that
  may not flow there.
- **Results are bounded** ([552](552-a-tool-result-has-no-bound.md)): with
  `keep_whole_above_bytes`, a result above it is kept whole beside a preview.
- **Messages and interruption** ([188](188-remote-helpers-on-llm-error.md),
  [161](161-tools-cannot-be-interrupted.md)): `RemoteEndpoint::shown` and `redact_in` keep
  credentials out of messages; an interrupted call drops its request.
- **Open Terminal has a component of its own** ([251](251-open-terminal.md), done), in
  `caelumora-open-terminal`, on a small typed client and bearer authentication. It needed neither
  this issue nor another kind of `auth`.
- **Authentication** is still `bearer` alone (`RemoteAuthConfig`).

## Considerations

### Alignment with `mcp_server`

This is a second implementation of "a remote server whose tools are discovered", and it should
differ from `mcp_server` only where the protocols do:

- **Where it lives:** a crate of its own, `caelumora-tool-openapi`, with a library that registers
  the component kind, as `caelumora-tool-mcp` does: a program that reaches no OpenAPI service need
  not build it.
- **Configuration:** `endpoint` (`RemoteEndpoint`: the base URL, `auth`, `timeout_secs`,
  `refresh_secs`), `requires_approval` (`true` by default, `false`, `"unless_read_only"`),
  `classification`, `description`, `destination` and `keep_whole_above_bytes` - the same fields, the
  same defaults, the same `deny_unknown_fields`. New are where the OpenAPI document lives and which
  operations are offered (*Which operations*, *Decisions taken*).
- **Names:** `{component_id}__{operationId}`, as MCP tools are named, so that servers cannot collide
  (issue [37](37-tool-name-collisions.md)). An operation without `operationId` could get a name
  derived from method and path, or be left out (*Decisions taken*).
- **Discovery:** as `mcp_server` discovers (*What exists*): the document is read when the tools are
  first offered, in the name of whoever acts, kept for `refresh_secs`, and read again after that. An
  unreachable server or an unreadable document leaves the tools read before, or makes the tool set
  unavailable with why in general terms.
- **Description:** the parameter `description`, else the document's `info.description`, else a text
  naming the component (issue [194](194-tool-sets.md)).
- **Shared code:** `ApprovalMode` is shared already. The conversion of a JSON Schema into
  `ToolParameters` is needed twice; it moves to a shared place first, as a step of its own without
  behaviour change, and what OpenAPI needs of it - `$ref`, a `type` given as a list, `enum` - then
  serves MCP's schemas as well. Besides, both would do alike what `McpServerComponent` does in
  `offered`, `description` and `tools_of`: keep what was discovered in an `ExpiringCache`, keep it
  where the server cannot be asked, fall back in the description, name the tools. That is a few
  dozen lines. This issue said first that a common "discovered remote tool set" is worth asking once
  both exist, not before; [AGENTS.md](../AGENTS.md#design-principles) asks for repeated logic to be
  moved before its second use. The latter holds (*Decisions taken*).

### What is new with OpenAPI

- **Which operations.** A service may have hundreds, most of them useless or dangerous to an agent.
  A filter is needed: `include` / `exclude` by `operationId`, or by tag. Whether all operations are
  offered when no filter is given, or none: offering all mirrors MCP, offering none is safer for
  large APIs (*Decisions taken*).
- **Read-only.** MCP has `readOnlyHint`, which `"unless_read_only"` trusts (issue
  [246](246-mcp-tool-hints.md)); OpenAPI has the HTTP method. `GET` and `HEAD` are safe by HTTP's
  definition, so `"unless_read_only"` could trust them as read-only - with the same caveat: it is
  the server's word, trusted only where configured, and services that change state on `GET` exist.
  An extension (`x-...`) is not standard and would not be relied on.
- **Parameters.** An operation's path, query and header parameters and its request body have to
  become one object for the model, and be taken apart again for the request. Names can collide
  between them (a query `id` and a body field `id`); a body that is not an object (an array, plain
  text) needs a property of its own. Only JSON bodies at first; `multipart/form-data` (uploads)
  later or never.
- **Schemas.** OpenAPI schemas use `$ref` into `components/schemas`, which have to be resolved
  before conversion; cycles have to be cut. OpenAPI 3.0 and 3.1 differ in their schema dialect
  (3.1 is JSON Schema 2020-12; 3.0 has `nullable`). Swagger 2.0 is not supported. The document
  could be parsed by a crate for 3.0, one for 3.1, or by our own minimal reading of the parts we
  need as `serde_json::Value` (*Decisions taken*).
- **Servers.** A document may name `servers`; the configured `endpoint.url` is the base and wins.
- **Authentication.** `RemoteAuthConfig` knows only `bearer`. API keys in a header of their own
  (`X-API-Key`) or in the query are common with OpenAPI; a variant `header { name, secret }` would
  serve all remote components, and is an extension of `remote`, not of this component. A document
  on another origin than the endpoint would raise the question whether the credential is sent
  there.
- **Whose credential.** A call should be able to reach the service with a secret of the agent's
  owner, so that the service, too, knows in whose name it acts. A secret is resolved today in the
  context the agent acts in, whose principal is its owner (issues
  [255](255-whose-name-agents-act-in.md), 516), but found in the operator's configured chain.
  Issue [310](310-secrets-have-no-owner.md) decided that a reference names its vault (`vault/KEY`)
  and reserved `user` as the principal's own vault; a secret a user brings, which `user/KEY` finds,
  is issue [343](343-users-bring-their-own-secrets.md). Two questions of 343 meet this component
  as every remote one: that the service sees the owner also where another member of the channel
  caused the turn (*Turns caused by others*), and whether an owner without a secret of their own
  falls back to a configured one.
- **Results.** A JSON body is the result; text is returned as text; both are bounded by
  `keep_whole_above_bytes` as for MCP. Anything else (binary, images) is lost for both until issue
  [208](208-generated-artifacts.md). A status of 4xx or 5xx is a failed call with the body's
  message, bounded in length, the endpoint redacted (`RemoteEndpoint::redact_in`).
- **Interruption.** A dropped call drops its request, as for MCP; what the server does with a
  request it already received is up to it.

### The first service: Kimai

The time tracking of the maintainer's instance, `https://kimai.dimajix.net`, is the second,
concrete service *Relation to issue 251* waits for. Its document, as Kimai's Swagger page shows it
(`kimai-api.json`), says:

- **OpenAPI 3.0.0**, 69 paths, 99 operations (35 `GET`, 22 `POST`, 19 `PATCH`, 23 `DELETE`), each
  with an `operationId` of its own (`get_timesheets`, `post_timesheet`, ...). No `servers`; the
  paths are absolute (`/api/timesheets`), so the endpoint is the host.
- **Authentication** by a bearer token (`securitySchemes.bearer`, `scheme: bearer`): `bearer` of
  `RemoteAuthConfig`, nothing of a header of its own. What `security` an operation names besides
  are Kimai's permissions, not schemes.
- **Parameters** in the path (96) and the query (59), none in a header, no name shared with a
  field of a body. Request bodies are JSON: 19 a schema of `components` (`$ref`), 5 an object
  written in place, 2 an array (`patch_app_api_invoice_updatemetafields`,
  `patch_app_api_user_updateuserpreference`) - the case of a body that needs a property of its
  own.
- **Schemas:** 53 in `components`, 141 `$ref`s, no cycles, no `allOf` or `oneOf`, one `nullable`.
- **Results** are JSON but for 21 without a body and 8 that export or render (PDF, DOCX, XLSX,
  ODS, XML, HTML): those are left out by `operations`.
- **What a reading has to bear:** query parameters named with `[]` (`users[]`, `projects[]`,
  `tags[]`), which a model's tool can hardly be given as they are - named without the brackets for
  the model, sent as `users[]=1&users[]=2` -; `"items": []` for those arrays, which OpenAPI does
  not allow and is read as anything; allowed values given as a `pattern` alone (`"ASC|DESC"`,
  `"0|1"`, `"\d+|all"`), with no `enum`, which goes to the model as the `enum`'s values do; and
  `"default": null` nearly everywhere.
- **Kimai serves its document only inside the HTML of its Swagger page**: `/api/doc.json`, where
  its documentation names it, is not found on this instance, logged in or not. Hence `spec_file`
  (*Decisions taken*).

### Confidentiality

As for MCP: what comes back is classified as configured (`classification`; issue
[139](139-mcp-result-classification.md), open, for classifying per result), and what goes out only
where the configured `destination` may receive it ([180](180-outbound-flow-unchecked.md)). 139
should be answered for every remote tool source at once, not for MCP alone.

### What it does not replace

A generic component gives every operation the meaning of "an HTTP call": no approval by what a call
does beyond the method, no `matcher` that understands an argument, no following of a long job. A
service where that matters is better served by a component of its own, as Open Terminal's shell is
(251); the generic one is for services whose operations are self-explanatory.

### Relation to issue 251

251 is done, without this issue: Open Terminal is reached by `caelumora-open-terminal`, and the
bridge and the order weighed here (see *History*) are moot. What remains is that Open Terminal's
`/openapi.json` is a document at hand to test against, with its `/ports/...` proxy as the example of
why the operations have to be filtered. Per [PLAN.md](PLAN.md), this issue waits for a second,
concrete OpenAPI service to be reached.

## Decisions taken

Agreed with the maintainer on 2026-11-26.

- **The schema conversion is a method of `ToolParameters`,** the type it makes, in
  `caelumora-domain`: `ToolParameters::from_json_schema(&Value)`. It moves there first, without a
  change of behaviour. Resolving `$ref` and reading a `type` given as a list follow in a step of
  their own, since they change what MCP's tools are offered as. The values of an `enum` go into the
  parameter's description for now; `ToolParameters` gets no variant for them. Decided with step 2:
  `from_json_schema(schema, document)`, `document` what the references point into - an MCP tool's
  input schema itself, an OpenAPI service's whole document -; a `pattern` goes into the description
  as an `enum`'s values do, and stays there where a description written beside a reference takes
  the place of the one it points to; an array whose `items` is no schema holds strings; and a
  reading follows a bounded number of references, so that definitions that refer to each other do
  not make it without end.
- **The discovered tool set is shared before this component is built**, as AGENTS.md asks of
  repeated logic: what `McpServerComponent` does in `offered`, `description` and `tools_of` becomes
  one type that both use. Its interface is designed in the plan of the implementation. What each
  protocol plugs in is what differs: MCP's `instructions` and its discovering again after an
  unknown tool, OpenAPI's `info.description`.
- **An operation without `operationId` is left out**, with a warning in the log. A name derived from
  method and path would be a rule of its own, and the common generators - FastAPI among them - set
  an `operationId`.
- **The filter is one optional list, `operations`,** of `operationId`s. Without it, every operation
  is offered, as MCP offers every tool; approval by default (`requires_approval: true`) and
  `destination` keep that safe, and a service of hundreds of operations needs the list. No tags and
  no `exclude` until somebody needs them. *Replaced the same day by the map below.*
- **`operations` is a map, and required:** each operation offered is named by its `operationId`,
  with what holds for it alone - `{}` for nothing of its own -, and only what it names becomes a
  tool; a component without it, or with none in it, is not built. An operation the document does
  not have is refused as the component is built. Unlike `mcp_server`, which offers every tool of
  its server: a service described by OpenAPI has hundreds of operations, deleting ones among them,
  and what an agent may do with it is chosen, not filtered. What holds for an operation alone is,
  for now, `requires_approval`, `true` or `false`, over the component's `requires_approval` (`true`
  unless written, `false`, or `"unless_read_only"`, which trusts `GET` and `HEAD`): so a reading
  operation may be asked, and a writing one - stopping the running timesheet - may not. Rules of a
  place still ask or deny on top (APPROVALS.md). Considered for later, not built: the same override
  per tool for `mcp_server`, in the shared discovered tool set, once an MCP server needs it.

  ```yaml
  requires_approval: unless_read_only
  operations:
    get_timesheets: {}
    post_timesheet: {}
    patch_stop_timesheet: { requires_approval: false }
    get_users: { requires_approval: true }
  ```
- **The document is read as `serde_json::Value`**, by our own reading of the parts needed - paths,
  their parameters, a JSON request body, `info.description` -, covering 3.0 and 3.1 without a new
  dependency. Resolving `$ref` is needed by the schema conversion for MCP anyway.
- **The kind is `openapi_server`**, in the crate `caelumora-tool-openapi`, as `mcp_server` is in
  `caelumora-tool-mcp`.
- **The document lies below the endpoint:** `spec_path`, relative to `endpoint.url`, by default
  `openapi.json`, read with the endpoint's credential. It cannot lie on another origin, so whether
  the credential goes there does not arise.
- **Or the document lies in a file:** `spec_file`, a path relative to the working directory, for a
  service that does not serve its document as such - Kimai, which renders it into its Swagger page
  alone (*The first service: Kimai*). `spec_path` and `spec_file` exclude each other. A file is
  read once, when the tools are first offered; `refresh_secs` is `spec_path`'s alone. What fails
  in reading it names the component and that the document cannot be read or is no OpenAPI 3.x,
  never what the file holds. That a file is read with the server's rights, as a URL is reached from
  its network, is decided on the layer of who may write a component (issue
  [608](608-a-component-a-user-writes-acts-with-the-servers-position.md)), not by a guard here: a
  path in the operator's configuration of the library alone was weighed and set aside as a detour
  that solves it for files and not for URLs.
- **Authentication by a header of its own** is an issue of its own, opened once a service needs it;
  such a variant names its secret by a reference, as `bearer` does.
- **The credential is resolved as every remote component resolves it:** through `RemoteEndpoint`
  and the `Secrets` bound to the work, with nothing of its own. A secret of the agent's owner is
  `user/KEY` of 310, found once 343 is done; then the service, too, knows in whose name it is
  called, and this component gets it with `mcp_server` and `a2a_agent` at once. This issue does
  not wait for 310 or 343, but comes sensibly after 310, so that the kind declares
  `ComponentKind::reaches` from the start instead of 310 changing one kind more.

## Done when

- A component kind `openapi_server` in the crate `caelumora-tool-openapi` offers the chosen
  operations of an OpenAPI 3.x service as tools, with the configuration, naming, discovery,
  approval, destination, classification and bound of `mcp_server`, and shares its code with it
  where they do the same.
- It is documented in TOOLS.md (next to MCP servers) and COMPONENTS.md.

## History

- **2026-10-07** — Opened alongside issue [251](251-open-terminal.md), where a generic OpenAPI
  component was weighed as one way to reach Open Terminal and found to be worth having on its own,
  not as that way.
- **2026-10-07** — Added how this issue relates to [251](251-open-terminal.md): the shared step
  (`Approval`), this component as a bridge for Open Terminal with its `/ports` proxy filtered out,
  and the order of the two.
- **2026-11-26** — Brought up to date with the remote tool sources as they stand (*What exists*):
  `mcp_server` in a library of its own (325), `ApprovalMode` shared, discovery lazy and repeated
  (126), `destination` (180), the bound of results (552), redaction (188), interruption (161), and
  251 done with a component of its own. The considerations follow: a crate of its own, the fields
  `destination`, `keep_whole_above_bytes` and `refresh_secs`, what the shared schema conversion
  lacks, and the conflict between "a shared tool set once both exist" and AGENTS.md's rule to move
  repeated logic before its second use. The bridge for Open Terminal and the order of 251 and this
  issue are moot.
- **2026-11-26** — Discussed with the maintainer and decided (*Decisions taken*): the schema
  conversion as `ToolParameters::from_json_schema`, `$ref` and `type` lists in a step after the
  move; the discovered tool set shared before this component is built; operations without
  `operationId` left out; one optional list `operations`, all offered without it; our own reading
  of the document; the kind `openapi_server` in `caelumora-tool-openapi`; `spec_path` below the
  endpoint; authentication by header an issue of its own once needed. Added at the maintainer's
  request: a call should reach the service with a secret of the agent's owner, which needs 310 and
  343 and nothing of this component's own.
- **2026-11-26** — Kimai (`https://kimai.dimajix.net`) is the first service (*The first service:
  Kimai*). It serves its document only inside the HTML of its Swagger page, so a document may also
  lie in a file (`spec_file`), decided with the maintainer; who may name a file, as who may name a
  URL, is issue [608](608-a-component-a-user-writes-acts-with-the-servers-position.md).
- **2026-11-26** — Decided with the maintainer on approvals: `operations` is a map, required - only
  the operations it names are offered -, and an operation may set its own `requires_approval`
  over the component's (*Decisions taken*). The same override per tool for `mcp_server` is noted
  for later.
- **2026-11-26** — Step 1 done: the conversion of a JSON Schema into `ToolParameters` is
  `ToolParameters::from_json_schema` in `caelumora-domain`, moved from `caelumora-tool-mcp`
  (`schema_to_tool_parameters`) with its tests, without a change of behaviour.
- **2026-11-26** — Step 2 done: `ToolParameters::from_json_schema(schema, document)` resolves a
  `$ref` as a JSON pointer into `document` - an MCP tool's input schema itself, an OpenAPI
  service's whole document -, cutting one that points nowhere or back to where it began; reads a
  `type` given as a list beside `"null"` as that type; adds the values of an `enum` and a `pattern`
  to the description; and reads an array whose `items` is no schema - Kimai's `"items": []` - as
  one of strings. MCP's tools whose schemas use these are offered more precisely than before. The
  review added that a description beside a reference keeps the allowed values where it points, a
  reference is cut by what it points to, not by how it is written, and a reading follows at most
  `MAX_REFERENCES` (1000) references - a document of definitions that refer to each other is read
  again on every way to them, which grows without end otherwise.
- **2026-11-26** — Step 3 done: the discovered tool set is `DiscoveredTools` in
  `caelumora_agents::tool`, named beside `FixedTools` - `ToolSet` is the domain's name for what a
  provider is described as -, over a `ToolSource` - what a protocol plugs in: how its tools are
  discovered (`discover`, failing with `Undiscovered` - why in general terms and in full), the
  cache of what was discovered (`discovered`, which a tool may invalidate), what a discovery says
  of the source (`described`), and the tools made of it (`tools`). The set keeps the description
  as configured, else as the source says, else a text the component gives (`unnamed`), and the
  destination; it offers, keeps the tools discovered before on a failure and logs. `McpServer` is
  `mcp_server`'s source, and `McpServerComponent` is gone; nothing changes in what MCP does but
  the wording of the two warnings.
