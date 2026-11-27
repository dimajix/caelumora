# Tools

Agents act through tools. A tool is contributed by a component and run by the agent's
`ToolExecutor`, which receives the ready providers and has the kernel decide what is the kernel's
(issue 516). This document describes what a tool is, how its calls are run and what they answer, and
the tools that come with Caelumora. Which calls need a person's approval, and how a tool asks a
person, is in [APPROVALS.md](APPROVALS.md); how components are built in
[COMPONENTS.md](COMPONENTS.md); how far a tool call may travel in
[CONFIDENTIALITY.md](CONFIDENTIALITY.md).

Every tool is an object of its own, a `Tool` (issue 584): it holds its `ToolDescription`, carries
out a call (`execute`), and says what a call does in words for a person (`describe`), which calls
are like a given one (`matcher`, for a rule derived from it), and what a call asks a person before
it runs (`questions`). A component gives an agent tools through the facet `ToolProvider`, which
hands out its tools as objects and answers for them as a set: what they are for, which of them it
offers in a turn and why it offers none (`offered`, in general terms, never with the error), and
where their arguments go (`destination`). A provider that decides none of this beyond a description
and a destination is `FixedTools`, the tools its component made as it was built; one whose tools are
discovered of its remote side - an MCP server, an OpenAPI server - is a `DiscoveredTools` over its
`ToolSource`, which plugs in how they are discovered and made, while the set keeps them for a time
and decides what is offered when discovering fails ([MCP servers](#mcp-servers)); one whose offer
depends on its tools holds each with what it is offered by, never choosing among them by their
names. A parameter written in a language - a bash command line, `ToolParameters::shell` - says so;
the model is not told of it, but the default `matcher` derives from it a rule on the words a command
begins with ([APPROVALS.md](APPROVALS.md#rules-on-commands)). A tool that knows more of what makes
two calls alike says better than the default.

What a call does, as its tool describes it, is the call's: the agent stamps it on the call as the
executor admits it (`ToolCall::description`, issue 586), whoever decides on the call is shown it,
and a frontend shows it with the call as its presentation allows (the web UI's, in
[WEB-UI.md](WEB-UI.md)). A tool that says nothing of a call beyond its arguments gives none, and the
call is shown by them; nor has a call answered before it was described. Made of the arguments, it
is withheld with them (`ToolCall::withhold_arguments`).

A `ToolDescription` declares a tool's name, description and parameters, whether it requires
approval, its visibility, optionally the classification of its results, and its presentation: how
frontends show its calls, so that they need not recognise a tool by its name. The presentation is
display only - what a call does, and whether it waits for anyone, does not depend on it. A call's
result carries it too (`ToolCallResult::presentation`, issue 586), stamped from the call as its
visibility is, so that a frontend shows a result it has not joined to its call, as the terminal's
line-wise view does. A call is shown by its own presentation until its result comes, and then by
the result's - so far no tool gives its result another.
Approval and classification are decided per tool by its provider, not by the personality.

What a tool, its parameters and its tool set say to the model are texts of the prompt catalogue
(issue 57, see [AGENT-TURN.md](AGENT-TURN.md#the-prompt-catalogue)), kept apart from the tool's
schema. The code builds the schema - the parameters, their types, which are required
(`ToolParameters::string()`, `object(…)`) -, and `tool::described` gives the tool and each of its
parameters the text of its path among the tools of its component kind, in the namespace of the
agents or of the library that brings it (`<namespace>.tools.<kind>`): `notes.write_note` is the
tool's, `notes.write_note.text` its parameter's, `.item` an entry's of a list, and the entries of a
list of objects are described by their fields under the list's path. A parameter described already
keeps its text, as the parameters every reclassifying tool shares (`tool::reclassify_parameters`).
The texts are rendered once, when the component is made; a text missing keeps it from being built. A
tool of a generic provider, an MCP server's, is described as it describes itself, its parameters
read of the JSON Schema it gives (`ToolParameters::from_json_schema`).

## Tool sets

The tools of one provider are a *tool set* (`ToolSet`), named after its component, with the
provider's description and, if it is unavailable, why. A model's backend is given the tools
natively, without the sets; the sets are kept in the record of what the model was offered and shown
in the web UI. Only a generic provider such as `mcp_server` takes its description from its
configuration.

**What a turn is offered** depends on where it works (issue
[290](../issues/290-tools-offered-by-the-turn.md)). A provider has its tools (`tools`), among which
a call finds its tool by name and whose names no other provider of the agent may have, and offers of
them those that may be called in the turn's situation (`offered`): by default all, a provider whose
tools depend on the place fewer, or none, saying why. It is asked once the turn is begun and before
its context is built, given the turn's `Situation` - the place and whose authority it acts with -,
as a prompt part is; what it offers is fixed in the turn (see
[AGENT-TURN.md](AGENT-TURN.md#tools-offered)). Offering is a convenience for the model, never the
guard: a call of a tool not offered is answered as unknown, and a call of one offered keeps its own
checks, since what may be called can change while the turn runs. What the web UI shows of an agent's
tools is what a turn would be offered now, in the agent's own name.

The model needs no prompt to learn of its tools, but a source that is unavailable leaves no trace in
what it is offered. The optional component `tool_sets` names the unavailable sets of the turn, with
why in general terms, and tells the model to say they are unavailable rather than that it cannot do
what they do. A personality without it tells its model nothing of missing tools.

A set may say where the arguments of its calls go (`ToolSet::destination`): where the channel may
not flow there, the set is offered as unavailable, and a call of a tool the turn was not offered is
answered as unknown (see [CONFIDENTIALITY.md](CONFIDENTIALITY.md#what-leaves-through-tools)).

## Tool visibility

Every tool declares a `ToolVisibility`: whether the call is published to the channel, and whether
its arguments and result are, or only the agent's owner sees them.

*   `SHOWN`, the default: others see the call in full.
*   `WITHOUT_RESULT`: others see that the tool was called and with what, but not what it returned.
*   `CALL_ONLY`: others see that the tool was called, but not with what.
*   `HIDDEN`: the whole call stays with the agent.

How published messages are redacted, and what the model is told about results it must not quote, is
described in [CONFIDENTIALITY.md](CONFIDENTIALITY.md).

## Classified results

Every result is classified: as the tool classified it, else as the tool is configured, else as the
channel. A result that may not flow into the channel (`may_publish`, in the agent's name) never
reaches the model: it is replaced by the status `withheld` and a message that names the
classification and asks the model not to obtain it another way. The value is kept nowhere. Results
that did not come from the tool - refused, denied, timed out - are the agents' fixed texts,
classified as the turn's label; a failed one, whose error may come from the tool, is the channel's.
Built-in tools whose answer is derived from the arguments alone classify it as the turn's label too.
See [CONFIDENTIALITY.md](CONFIDENTIALITY.md#tool-results).

## Running tool calls

An agent's tool calls are run by its `ToolExecutor`, built with the `ToolProvider`s of its
components. The executor is no component: how a call runs is its tool's business, but whether it
runs and what the model is told is never the choice of whoever may write a personality. What of a
call is the kernel's to decide it asks of the kernel, through the agent's runtime (issue 516):
whether the policy permits the call (`CallTool`) and what the rules of its places say, once, as the
call is admitted (`ActorRuntime::admit_tool_call`, see
[Checked before anyone is asked](APPROVALS.md#checked-before-anyone-is-asked)); whether a rule
denies it now, again as it runs (`rule_denying`); and what may flow where (`may_publish`). It finds
a call's tool among the tools its providers have now (`ToolProviders::find`), the one of the
provider configured first, and asks that tool, never a provider by the tool's name: once as the call
is admitted, for what it does, asks and is like, and again as it runs, since a provider's tools may
change in between. Before it hands a call to its tool, it checks besides that the call was offered
to it. Only agents call tools:
the kernel makes an `ActorRuntime` for an agent alone, so that the executor need not ask. Afterwards
the executor times the call, turns an error into a status and a next step, classifies the result,
withholds what may not flow into the channel and marks what only the owner sees.

**Two tools of the same name** are a configuration error that keeps the agent from starting, naming
both components: resolved by order, one of them would be silently unreachable. User commands follow
the same rule. A provider whose tools change while the agent runs - an MCP server's - can bring
such a clash later: then the tool of the component configured first keeps the name, the other is
not offered, and a warning names both (issue 126).

**A call is not waited for longer than its turn lasts.** A cancel or a stop drops the tool's future
wherever it stands; the call is answered as `interrupted`, and the agent starts no further call of
the step. A tool that is done already keeps its result. Dropping a tool does not undo what it did
([20](../issues/20-transactional-effects.md)): the call counts as having run - a turn given up keeps
it with its result - and the model is told to check its effect before calling it again.

A user may also interrupt one call alone and let the turn go on (see
[CHANNELS.md](CHANNELS.md#cancelling)). A call interrupted while it runs is dropped as above and
answered as `interrupted`, saying who interrupted it and that the turn goes on; one interrupted
before it runs is answered as `not_executed`. The other calls of the step run. A shell command
handed back as running is no call any more; a user ends it as a command (see [The
workspace](#the-workspace)).

Dropping a future is all the stopping a tool gets: none of its code runs after the `await` it stood
at, and what the future holds is released. A tool therefore:

*   expects to be dropped at any `await`, and never leaves the turn half changed across one;
*   binds whatever it starts to its own future: a task it spawns is aborted on drop (a plain
    `JoinHandle` only detaches it), a child process is killed on drop, and work on a thread of its
    own cannot be interrupted at all;
*   ends on drop what it started outside the process and that would go on without it, in a task
    of its own, bounded and logged, which the drop does not await
    (`caelumora_api::guard::EndOnDrop`): the commands of `workspace`, which live with the storage,
    and the prompts an image model of the backend `comfyui` runs for `image_generation`, which hold
    its server;
*   does not block: code that never reaches an `await` cannot be interrupted, and holds up the
    agent's whole task, which even the abort after the stop's grace does not end.

What happened outside the process - a request the remote side already received - is not undone,
and whether the remote side stops depends on it, unless the tool ends it as above.

### Results

Every call ends with a result the model can act on: its `content`, which is all the model is given
of the `ToolCallResult` in words, by its tool and its call - besides what the tool gave and showed
it, given as what it is.

*   A successful call's result is the tool's output.
*   Every other outcome is `{"status": ..., "message": ...}`, whose message says what happened and
    what to do next: `denied`, `forbidden` (without the reason, which stays in the log),
    `timed_out`, `declined`, `invalid_arguments`, `failed`, `not_executed` (the tool never ran),
    `interrupted` (it ran, but was broken off), `unknown_tool` (with the tools that do exist) and
    `withheld` (its result may not flow into the channel, see
    [Classified results](#classified-results)).
    The status is stored with the result, so that refusals and failures can be told from results.
    The wording is the prompt catalogue's (`agents.results`, issue 57); what went wrong, the
    error's own text. A message that cannot be had is logged, and the model is given the status
    alone: the call is answered all the same. A result only its agent may see is given only with
    the mark that says so, and withheld without it.
*   No call is left without a result. A turn that ends while calls of its step have none answers
    them as `not_executed`, saying why the turn ended.

A call that ran records when it started and ended, around the tool's own run, so that a client can
show the duration. A call that needed approval records who approved or denied it, with the rule
that answered for them where one did, and when (`ToolCallResult::approval`, see
[APPROVALS.md](APPROVALS.md#what-is-recorded-of-an-approval)); a call that asked questions, how they
were answered (`answers`). Both are for the record and never given to the model.

A tool may give the agent something to read besides its result (`Turn::give`) - a skill it loaded,
say, with the knowledge it is. It is written after the results of the step, since nothing may stand
between the calls and their results (see
[AGENT-TURN.md](AGENT-TURN.md#beginning-and-ending-a-turn)).

What a tool read for its result - a file by its place, a channel's history, notes - it gives with
its output (`ToolOutput::knowledge`), never records itself (issue 538). The executor records it with
the turn and keeps it on the result (`ToolCallResult::knowledge`) where the result goes to the
model, not where it is withheld: the transcript holds what the agent read, as an entry given holds
what it was given, and the model is never given the record.

### Content a tool gives its model

A tool may give its model files besides its value (`ToolOutput::files`, issue 512) - an image it
generated, a text too long for its result: each a name and its bytes. A tool hands over bytes; the
executor has them kept, so that what is checked is checked once for every tool and no tool reaches
the storage. They are kept only for a result that goes to the model, not one withheld, as the
result's (`ToolCallResult::files`, through `ActorRuntime::create_content`): in the agent's name,
classified as the result, in the channel's key space, byte for byte and whatever they are, each
labelled as its bytes say, never as the tool or a name claims, at most as many as a message may
carry attachments, and all of them or none. Keeping is not interpreting: what a tool expects of what
it hands over it checks itself, as `generate_image` that it was given images. They wait for no
message: the result refers to them from the moment they are kept, and its turn keeps the result
(see [CHANNELS.md](CHANNELS.md#what-a-message-carries)). Until the channel has recorded the message
of the result, the kernel finds what it kept for the caller by itself
(`AttachmentManager::kept_content`, `ChannelView::attachment`), so that the next step of the turn
can read it, however fast it follows.

A text above a bound its component is configured with (`keep_whole_above_bytes`, `KeepWholeAbove`)
is kept this way, beside a preview of 4 KiB whose `next` says how the whole is read
(`ToolOutput::kept`).

What cannot be kept fails the call, after the tool acted. So a tool that keeps something nobody
configured it to keep - the original of a page fetched - asks first whether its turn would keep
it with the files it hands over anyway (`Turn::can_keep_content`): what `create_content` checks
before it keeps anything - the caller, the number of files, their names and sizes, the
classification and the storage -, keeping nothing. Where the answer is no, it hands the file not
over, and the call goes on as if it had none. A yes holds but for a storage that fails.

What a tool gave is seen, not said: the model is given an image with the result as it is given an
attachment - anything else is reached with the tools for [attachments](#attachments) -, each
numbered as one and noted after the result's text - its number, name, type and whether it is given
along -, so that a model that cannot see learns of it and any model can name it; the agent shows it
to the others by `attach`. The card of the call shows it where the result is shown, until a message
of the same turn carries it: then the message shows it, and the card names it.

## MCP servers

An `mcp_server` component exposes the tools of a remote MCP server, reached over the Streamable HTTP
transport:

```json
{ "id": "github-mcp", "kind": "mcp_server", "params": { "endpoint": { "url": "https://mcp.example.com/mcp", "auth": { "type": "bearer", "secret": "agents/GITHUB_MCP_TOKEN" } } } }
```

The kind lives in the library `caelumora-tool-mcp` (issue 325), with `rmcp`, so that the kernel
does not depend on it; `caelumora-app` and `caelumora-tui` list the library, and a personality names
the kind as before. What such a component kind may use of the kernel is said in
[PLUGINS.md](PLUGINS.md#what-a-component-kind-outside-the-kernel-may-use).

*   **Several servers.** Each server is an instance of its own; its tools are named
    `{component_id}__{remote_tool_name}`, so that servers cannot collide.
*   **Discovery.** The tools are discovered when they are first offered, in the name of whoever
    acts there, never while the component is built (issue 126) - a component keeps no security
    context for later. They are discovered again once what was discovered is older than the
    endpoint's `refresh_secs`, and after the server did not know a tool that was called, so that an
    agent follows a server that was replaced without being built again. A server that cannot be
    asked does not fail the agent: the tools discovered before stay; without any, its tool set is
    unavailable, with why in general terms, never the error, which may name hosts. Either way a
    warning is logged, and the server is asked again at the next offer. That a server announces a
    change itself (`notifications/tools/list_changed`) is not heard (issue 433).
*   **Description.** The parameter `description`, else what the server says of itself, else a
    generic text.
*   **Credentials.** `auth.secret` names a secret by its vault and key, `vault/KEY`, resolved
    through its vault, which has to grant its use to the agent's owner and let it be sent to the
    endpoint's origin (see [SECURITY.md](SECURITY.md#secrets)); it is never a literal credential.
*   **Parameters.** `endpoint` (required: `url`, `auth` - optional, `bearer` -, `timeout_secs` -
    default 10 -, `refresh_secs` - default 300; see
    [COMPONENTS.md](COMPONENTS.md#remote-endpoints)), `requires_approval` (`true` by default,
    `false` or `"unless_read_only"`; see [What needs approval](APPROVALS.md#what-needs-approval)),
    `classification`, `description`, `destination` and `keep_whole_above_bytes`.
*   **Calls.** Each call opens a connection, and so an MCP session, of its own. An interrupted call
    drops it, which ends the session; whether the remote tool stops is up to the server.
*   **What it keeps.** Without `keep_whole_above_bytes`, a result goes to the model whole, whatever
    its size. With it, at least the 4 KiB of a preview, a result above that size, measured on what
    would go to the model, is kept ([content](#content-a-tool-gives-its-model)): its text as
    `<component id>.txt`, and its structured content as `<component id>.json` unless the text parses
    to the same JSON, in which case it would only be a copy; an answer with neither is kept as the
    JSON of its blocks. The result is then a preview of the text, none where there is no text, since
    a preview would cut JSON in the middle; its `next` says that the whole is kept and is reached
    with the tools for [attachments](#attachments). Whoever sets it gives the agent `attachments`
    too, and a storage of content that is missing fails the call, after the server has acted. What
    a failure of the tool says is not kept: it is cut to the size of a preview where a bound is
    configured.

## OpenAPI servers

An `openapi_server` component (issue 252, library `caelumora-tool-openapi`) offers the operations of
a server described by an OpenAPI 3.x document as tools, chosen one by one:

```yaml
- id: kimai
  kind: openapi_server
  params:
    endpoint: { url: "https://kimai.example.com", auth: { type: bearer, secret: agents/KIMAI_TOKEN } }
    spec_file: local/kimai-api.json
    requires_approval: unless_read_only
    operations:
      get_timesheets: {}
      post_timesheet: {}
      patch_stop_timesheet: { requires_approval: false }
```

It is an `mcp_server` of another protocol, and alike where the protocols do not differ: one server
per component, its tools named `{component_id}__{operationId}`, discovered as an MCP server's are -
a `DiscoveredTools` over the server -, its parameters those of an `mcp_server` with their defaults,
its results kept beside a preview above `keep_whole_above_bytes` as an MCP server's (JSON without
one), and its credential resolved for the URL of each request.

*   **The document.** `spec_path`, below the endpoint (`openapi.json` unless written), read with
    the endpoint's credential and again after `refresh_secs`; or `spec_file`, a file relative to the
    working directory, read once, for a server that does not serve its document - the two exclude
    each other, and `refresh_secs` goes with `spec_path` alone. What fails in reading it names
    neither the file nor what it holds. The paths of the document lie below the endpoint; its
    `servers` are not read. The description of the tool set is the parameter `description`, else
    the document's `info.description`, else a generic text.
*   **Which operations.** `operations` is required: each operation offered is named by its
    `operationId`, with what holds for it alone - `{}` for nothing; no other becomes a tool. A
    server has hundreds, deleting ones among them, and what an agent may do with it is chosen, not
    filtered. An operation the document lacks, or one that needs what the component cannot give - a
    parameter in a header or a cookie, or one without a name or a place, a parameter named `body`,
    two arguments of one name, a body that is required and no JSON -, fails the reading of the
    document as an unreachable server does: the tools read before stay, without any the tool set
    is unavailable, and the log names the operation.
*   **Approval.** An operation's own `requires_approval`, `true` or `false`, else the component's:
    `true` by default, `false`, or `"unless_read_only"`, which trusts `GET` and `HEAD` to only read,
    as HTTP defines them - a server that changes something on a `GET` is wrong, and trusted only
    where configured (see [What needs approval](APPROVALS.md#what-needs-approval)).
*   **Arguments.** An operation's parameters in the path and the query, by their names - a name
    ending in `[]` (`users[]`) without it, an array sent as the name repeated
    (`users[]=1&users[]=2`) -, and its JSON request body, whatever it is, as one argument of its
    own, `body`, so that no field of a body collides with a parameter. Their schemas are read as an
    MCP tool's are (`ToolParameters::from_json_schema`), references into the whole document. A
    value of the path that is empty, `.` or `..` is refused: it would make the call reach another
    path than its operation's.
*   **Results.** A call is one request; an interrupted call drops it. JSON is given as JSON, text as
    text, an answer without a body as its status (`{"status": 204}`); anything else - a PDF, an
    image - fails the call. A status of failure is the call's failure, with what the server said,
    cut to the size of a preview whether a bound is configured or not - unlike an MCP tool's
    failure: an HTTP server's is often a whole page of HTML.

## Images

An `image_generation` component (issue 525, library `caelumora-tool-image`) generates images with
the image models its agent is given: those of its components of the kind `image_model`, each one of
the image models the platform offers (see [CONFIGURATION.md](CONFIGURATION.md#image-models)). The
tool names no backend; where and how a model generates is its backend's - ComfyUI's
([CONFIGURATION.md](CONFIGURATION.md#comfyui)).

```yaml
- id: image-model
  kind: image_model
  params:
    name: z_image_turbo
- id: image-generation
  kind: image_generation
  params:
    destination: { level: internal }
```

*   **One tool.** `generate_image` makes an image from a description: `prompt`, and optionally
    `model` - one of the agent's image models, the first unless given -, `negative_prompt`,
    `width`, `height` and `seed`. The parameters are the tool's, the same for every image model; an
    image model takes those it can and leaves the others out, and the tool's description lists the
    image models with what each says of itself. An image model not offered, or a request the image
    model does not take - a size outside its bounds -, is the agent's model's to correct. An agent
    without an image model has no such tool: the component is not built.
*   **A seed** left out is chosen at random, below 2^53, so that another call gives another
    image; the result names the seed taken, so that the agent's model can repeat or vary one.
*   **A call** asks the image model, in the agent's name, and gives the images it generated to the
    agent's model ([Content a tool gives its model](#content-a-tool-gives-its-model)); its value
    names the image model, the seed and the files. What the image model's server refused or failed
    with is the call's failure, in its words. A call given up - its turn cancelled or stopped -
    drops the request to the image model, and a backend that started something on its server ends
    it (see [Running tool calls](#running-tool-calls)).
*   **Parameters.** `requires_approval` (`false` unless given: a call changes nothing but the
    server's time), `destination` (`public` unless given - what the servers of its image models are
    cleared for, where the arguments go and stay, in a server's history and in what it saves) and
    `description`. The result is classified as the turn's label: it follows from the arguments
    alone.

## Pages of the web

A `web_fetch` component (issue 544, library `caelumora-tool-web`) fetches pages of the web by their
URL, with the tool `fetch_page`:

```yaml
- id: web-fetch
  kind: web_fetch
```

*   **What it gives.** A page of HTML as Markdown, without what only a browser needs - scripts,
    styles, frames, the controls of forms - or what stands around the content - navigation, a
    site's footer, sidebars (`aside`) -; plain text and JSON as they are, decoded by the charset
    their type names, as UTF-8 where it names none. A page of another type - a PDF, an image - or
    of none is refused, naming the type, as is one larger than 5 MiB. The text is given a page at a
    time, as `read_file` gives a file (`start_line`, `start_offset`, `line_count`, 64 KiB at once),
    with the URL the page was found at, after redirects, and its title - as text, where the
    Markdown escapes what it would give a meaning (`\*`, `\<`, `\&`, issue 620). Every call fetches
    the page again; nothing is cached. What failed names the URL without credentials, query and
    fragment.
*   **What it keeps.** With `keep_whole_above_bytes`, the text of a page above that size, asked for
    with none of `start_line`, `start_offset` and `line_count`, is kept whole beside its result, as
    a text the tool gives its model ([content](#content-a-tool-gives-its-model)), named after the
    page's host. Its result is then a preview of 4 KiB, whose `next` says that the whole is kept and
    is reached with the tools for [attachments](#attachments). So a long page is fetched once and
    read as it was. Whoever sets it gives the agent `attachments` too, and a storage of content
    that is missing fails the call. Of a page of HTML asked for whole, whatever its size, the body
    as the server sent it, before it was decoded, is kept beside its result as well, named
    `<host>.html` - but only where the turn can keep it, unasked as it is: what the Markdown left
    out, a table, a link, an attribute, is then found without fetching the page again, which may
    have changed. It is read with `read_attachment` where it is UTF-8, and copied into the workspace
    with `copy_attachment` in any charset; the charset the response named is not kept.
*   **What it reaches.** `http` and `https` alone, at most five redirects. A fetch goes through the
    proxy the environment names (`HTTPS_PROXY`, `NO_PROXY`), as the server reaches the internet.
    It refuses an address inside the network: the host itself; the networks that are private,
    shared (`100.64.0.0/10`), link-local - the metadata service of a cloud, `169.254.169.254` -,
    unique local, reserved, for benchmarks (`198.18.0.0/15`, which VPNs use inside networks), for
    documentation, for the IETF's protocols (`192.0.0.0/24`), or for NAT64 inside a network
    (`64:ff9b:1::/48`); multicast groups; and an IPv6 address that carries an IPv4 one - mapped, by
    NAT64, by 6to4 - by that one. An address written in the URL is refused always, at every
    redirect too; a name where the client resolves it itself, for a host the proxy is not asked
    for, by every address it resolves to, as it is resolved for the connection, so that what was
    judged is what is reached. The connection to a proxy is made wherever it lies, but no page is
    fetched from a proxy's host.
*   **What the proxy has to refuse.** Through the proxy, the proxy resolves a name and connects;
    Caelumora sees only the proxy's address, so the proxy's rules decide what a fetch reaches by a
    name (issue [545](../issues/545-web-fetch-through-the-proxy-reaches-anything.md)). A proxy the
    server goes through has to refuse it the destinations above, by the address it resolves a name
    to: the metadata service by whatever name, `metadata.google.internal` among them. What the
    server itself reaches inside the network - a model's endpoint, an intranet service - is named
    in `NO_PROXY`: it is connected to directly, and so refused to a fetch by Caelumora's own check.
    Where a server's proxy reaches hosts inside the network for it, every agent with `fetch_page`
    reaches them too, by name. On AWS, IMDSv2 is to be required: like the metadata services of
    Google's and Microsoft's clouds, it then answers no request without a header of its own, which
    a fetch never sends.
*   **Confidentiality.** What a URL carries goes to anyone: the tool set's destination is `public`,
    not configurable, so the tool is offered in public channels alone. A page is classified
    `public`, as anything anyone can fetch. What it says is written by anyone - the tool's
    description tells the model to follow no instructions in it; what bounds such text is what the
    agent may do afterwards, not the fetch.
*   **Approval.** `requires_approval` (`false` unless given: a fetch only reads, and its URL may
    carry only what is public). A rule derived from a call is about the pages of its host, by
    `http` or `https` and on any port, not about the one page.
*   **Parameters.** `requires_approval`, `description` and `keep_whole_above_bytes`, at least the
    4 KiB of a preview.

## Searching the web

A `web_search` component (issue 544, library `caelumora-tool-web`) searches the web with a
provider, with the tool `search_web`:

```yaml
- id: web-search
  kind: web_search
  params:
    provider: tavily
    endpoint:
      url: https://api.tavily.com
      auth: { type: bearer, secret: agents/TAVILY_API_KEY }
```

*   **A provider.** There is no protocol for searching the web; every provider has an API of its
    own. `provider` names whose the endpoint speaks - `tavily`, the one Caelumora knows so far -,
    and the endpoint's `auth` its key, which every provider wants: a component without is not
    built. The key is resolved through its vault in the name of whoever asks the agent, to be sent
    to the endpoint's origin (see [SECURITY.md](SECURITY.md#secrets)). A search goes through the
    proxy the environment names.
*   **What it gives.** For a `query`, at most `max_results` pages - 5 unless given, 20 at most -,
    each with its title, its URL and a snippet of at most 1,000 characters; no answer of the
    provider's own, nor the content of the pages, which the agent reads with `fetch_page` where it
    has it. The query is not repeated in the result. An empty query is refused before anything is
    asked, as one the provider refuses as written is the model's to correct; anything else the
    provider refused or failed with is the call's failure, naming the endpoint without credentials
    and what the provider says, bounded.
*   **Confidentiality.** The queries go to the provider, and stay there and with the engines it
    asks in turn: the tool set's destination is `destination`, `public` unless configured - an
    operator who may vouch for a provider clears it higher. What is found is classified `public`,
    as the web it was found on.
*   **Parameters.** `provider` and `endpoint` (both required; timeout 15 s), `requires_approval`
    (`false` unless given: a search only reads, and its query goes only where `destination` lets
    it), `destination` and `description`.

## Built-in tools

The tools that come with Caelumora are listed with the components that offer them, their parameters
and their approval defaults in [COMPONENTS.md](COMPONENTS.md#the-components); what the model is told
of each - its description and arguments - is the tool's own, in the code. How frontends show the
calls of a tool is said by its `ToolPresentation`: a name, defined as a constant next to the tools
shown so (`SKILL_PRESENTATION`, …) with what it shows of their arguments and results - the shell's,
`SHELL_PRESENTATION`, with how a command is called, answered and read back (`command_state`) in
`caelumora_api::terminal`; a frontend shows those it knows, and every other as `default` (issue
323). Two are any tool's, the type's own: `default`, and `quiet`, for a call that ends at once and
whose effect shows elsewhere - not shown while it runs and where it succeeded, as `default` where
it did not. Notes, private notes and agent memory are described in [MEMORY.md](MEMORY.md), skills
in [SKILLS.md](SKILLS.md).

`stop`, `bye` and `silence` end the turn with the current tool step: after `stop` and `bye` the
agent sleeps, after `silence` it stays idle without an answer. `create_plan` submits a multi-step
plan of at most `Plan::MAX_STEPS` steps; a plan in progress is extended, not replaced. `sleep`
waits at most 60 seconds, for testing a cancel or a stop while a tool runs.

### The workspace

`workspace` gives an agent its session's workspace, where its channel may use it
([CHANNELS.md](CHANNELS.md#its-workspace)): its files, and a shell where its storage gives one. What
its access list grants decides which of its tools may be called: the tools that only read files need
reading (`GetWorkspace`), the others and every shell tool changing (`WriteWorkspace`) - a command
may change anything its place reaches. A turn is offered those it may call: none where there is no
workspace it may use in its channel - the set is unavailable, saying so -, none that change where
the list grants reading alone, and no shell tool where its storage gives none; its prompt part says
the same ([AGENT-TURN.md](AGENT-TURN.md#its-workspace)). The workspace is reached through the turn's
views on every call all the same, before anything is sent: a call where there is none, or one the
list does not grant, fails and says so. Which calls need approval is configured as for `mcp_server`:
all - the default -, none, or all but those that only read files.

The kind lives in the library `caelumora-tool-workspace` (issue 428): the workspaces are the
kernel's, what an agent is given to work on them is built on the terminal's public API, the
agents' tools and prompt parts and the compaction's `KnowledgeRestorer`, as the tools of any other
subject are. Its tools and its prompt
part, which names its `read_file`, are one kind (issue 532): the part tells nothing true without
the tools. `caelumora-app` and `caelumora-tui` list the library.

**Files.** The file tools work over the workspace's terminal, whatever its storage, without a shell:
those that only read over the `TerminalReader` the workspace's reader hands out, those that change
over the `Terminal` of its writer (issue 516). A path is relative to the workspace's top; one that
leaves the workspace is the model's to correct, as is a file that is not there or not text. Reading
is bounded, and says where to read on: a text is given a page at a time, in whole lines (`page_of`).
A line too long for a page by itself is cut between characters and read on inside it, by
`start_offset`, so that a page never holds more than its bytes and a line keeps its number, which
`grep_files` names. A replacement must find its text exactly once, unless every occurrence is to be
replaced. Nothing is moved over, which would be a deletion, and the workspace's
top is neither moved nor moved over. What was read is knowledge, by its path, a file's content with
its digest (see [MEMORY.md](MEMORY.md#recording-what-was-read)); so is what `write_file` and
`replace_in_file` wrote, by its path and the digest of what they wrote - what the agent knows of the
file now, not what it found before a replacement -, and a file moved, where it lies now, by its
content as the agent knew it where it did. A file the agent last knew otherwise than it is now - a
person, a command or an agent of another channel changed it since - is neither written nor replaced
in, and the model is told to read it again (`AgentChannelState::last_known`): a workspace is used by
several channels and people at once, and a stale write would destroy their work unnoticed. A file
never known - a listing, a search or an offer tells no content -, or gone since, is written as now.
A refused call wrote nothing; the next is approved as any call. The check and the write are not one
step: what changes the file between them is overwritten. The approver reads a change whole, and a
rule derived from an approved change covers the file, whatever is written into it - for a move, both
places.

After a compaction, the component gives again the files the agent read or wrote whole in the part
replaced and still needs (`KnowledgeRestorer`, see [AGENT-TURN.md](AGENT-TURN.md#compaction)): each
as it is now, under its path (`[File: docs/plan.md]`), and known so, so that a write checks against
it. One that is gone, no text, or larger than a result holds (`max_output_bytes`) is not, nor one
that finds no room in what the compaction gives again, nor any where the channel may no longer use
the workspace or its storage cannot be reached: the model reads it again if it needs it.

`publish_file` offers a file for download at the message that calls it: a link to the file as it is
now, with its digest, downloaded through the channel's workspace by whoever may use it there. It
changes nothing and hands nothing to anyone who could not download the file already, so it reads,
and never needs approval, whatever is configured. What is to be kept as it was offered is published
with `publish_artifact`.

**Commands.** A command runs in the workspace's directory, every one anew - a `cd` does not carry
over - and is seen only from the channel it was started in. A call follows its command for
`wait_secs`; one still running then is handed back with its id, and the model follows it with
`command_output` or ends it with `kill_command`. What a call answers - what the command printed
since the offset asked for, cut from the front to what a result holds, where it stands and its exit
code - is a `CommandState`, which a frontend reads back to show a call presented `shell`. The
result is classified as the workspace. The approver reads the command whole, and a rule derived
from an approved command covers the commands beginning with its first two words, however long a
call waits. Following and ending a command need no approval, since only a command that was approved
is followed or ended.

A call dropped while it waits for its command - the turn cancelled or stopped - kills the command,
in a task of its own, bounded and logged, that the drop does not await, as a tool ends what it
started outside the process (see [Running tool calls](#running-tool-calls)): a command lives with
the storage, so that one handed back as running outlives the call, and one left running by a
dropped call would go on writing into the workspace after the turn that started it.

A command handed back as running is ended by the model, or by a user who may change the workspace's
files in the channel; the agent sees `killed` the next time it follows the command. How long a
command lives and when the kernel ends it is in [WORKSPACE.md](WORKSPACE.md#commands).

### Attachments

`view_attachment`, `read_attachment`, `copy_attachment` and `attach` (component `attachments`,
issue 282) reach the attachments the agent heard, by the number its transcript gave each (`a3`, see
[AGENT-TURN.md](AGENT-TURN.md#attachments)). A number resolves to the message that carries the
attachment (`AttachmentIndex::named`), read through the view of the agent's channel as it stands
(`ChannelView::attachment`): one a change removed is not found, and its label is the current one.
They are offered from the first turn on, since a tool may give one in the turn under way, whose
tools are fixed as it begins; `view_attachment` only to a model that sees, `copy_attachment` only
where a workspace may be written to.

*   `view_attachment` gives the model an image again: its result shows it (`ToolOutput::shown`,
    `ToolCallResult::shown`), and the model is given it along with the result as it is given an
    attachment. A result shows only attachments of its channel, which the channel checks as it
    admits the result.
*   `read_attachment` gives a text a page at a time, as `read_file` gives a file (`page_of`). All
    of its bytes decide what is a text, not its label: valid UTF-8, whatever control characters it
    holds; anything else is refused.
*   `copy_attachment` writes an attachment into the workspace the turn may use, to be worked on
    there: a document converted, data processed. A workspace used in the channel takes what the
    channel holds. It writes, so it is always asked.
*   `attach` puts a copy aside for the turn's next answer (`Turn::put_aside`), under an id and a key
    of its own (`AttachmentManager::copy`), so that what an agent shows of what it heard is its
    message's own. What a turn put aside is not stored: a turn resumed after a restart has lost it,
    not its answer. Its calls are presented `quiet`: the answer shows what was attached.

Each result is classified as the attachment, `copy_attachment`'s as the workspace; nothing is
recorded as knowledge besides - the result in the transcript is what was read.

The kind lives in the library `caelumora-tool-attachment` (issue 533): the attachments are the
kernel's and their numbers the transcript's, which names no tool; what reaches them is built on the
agents' tools and turn, as the tools of any other subject are.

### Artifacts

`publish_artifact` (component `artifacts`, of the library of `caelumora-artifacts`, issue 328)
publishes a file of the session's workspace as a version of an artifact: a fixed copy, kept in a
storage of content beyond the workspace, downloaded at the message that published it. `publish_file`
offers a link to a file as it is now; a model is told the difference in their descriptions - a link
for a look now, an artifact for a result to be kept.

It is offered where a file can be published: where the turn may use a workspace in its channel and a
storage of content is configured. The file is read from the workspace as the file tools read it and
handed to the `ArtifactPublisher` the plugin binds to the views of the turn's work (issue 516). It
publishes in the name of whoever the agent acts for: its owner, who makes the artifact
(`CreateArtifact`) or adds a version where the artifact's list lets them (`WriteArtifact`), asked
before anything is kept - and who reads the project it goes into (`GetProject`), which the session's
participants do. The participants of the channel it is published in read the version, and the
artifact, which reading a version needs as well (issue 316); the members of the project read the
artifact alone. An artifact is published into the scope of the project the session's results go to
(issue 467) under its name, made with its first version and given one more by every later
publication under that name. It is classified as the workspace, fixed with its first version; a
later version from a workspace classified otherwise is refused. A storage takes nothing above its
`destination`. Publishing changes what is kept, so it is approved where changes are; the approver
reads which file is published as which artifact.

The participants of the channel download the version published there, wherever they act; whom the
owner shares the artifact with read the versions there are as they share it; its owner reads every
version and shares it. An artifact lives as long as its project: with the session where the project
is the session's own, beyond it where it is shared - kept by lifting the session's own project,
never by moving an artifact (see [PROJECTS.md](PROJECTS.md), [STORAGE.md](STORAGE.md) and
[SECURITY.md](SECURITY.md#ownership)).

### Sub-agents

`start_subagent`, `ask_subagent` and `end_subagent` (component `subagents`, of the library of
`caelumora-subagents`, issue 558) hand a task to a sub-agent: a new agent of one of the
personalities the component is configured with (`personalities`), which works on the brief apart, in
a thread of its own opened at the call, and gives back only what came of it. Starting does not
block: the call returns the thread at once, and what the sub-agent answers is given to the agent to
read where the thread was opened, once it is there - as the kernel's delegate gives it (see
[CHANNELS.md](CHANNELS.md#delegating-into-a-thread)): the answer of its turn alone - its last
message, calling no tool -, bounded in length (issue 619), never the remarks it made on the way,
so that the agent's context holds the result and not the steps of the work. The sub-agent is told
so with each brief (see [AGENT-TURN.md](AGENT-TURN.md#what-is-given-to-read)); an answer it writes
in a message that calls a tool ending its turn (`turn_end`: `stop`, `bye`) is no answer and is not
given back, so the terminal's helper has none of them. `ask_subagent` gives it more to read - in
the turn that read its answer: one left without a word then is let go (issue 598), asked again
`NoSubAgent`, to be ended with its thread -, `end_subagent` ends it: its thread, which owns it,
closed with it (`procedure::thread::Close`, issue 598) - let go from its channel and its session,
the agent deleted, never purged, the thread closed last. A brief still open as it is ended is
dropped - if its leave came first, the agent is given back that it left before it answered.

They are offered where the work's session runs, and act through the `SubAgentWriter` the plugin
binds to the turn's work (issue 516), in its context: the agent gives, its owner owns the sub-agent,
made in the channel's scope, of the personality of that name the owner finds from the channel - the
innermost they may read, as `/agent create` finds one -, an internal one alone (issue 591), cleared
no wider than the agent that starts it (issue 597: where that agent is limited, every role the
personality names is named by its limit too; refused as a parameter, since another personality may
do), named by an adjective and the personality (`curious-researcher`), never by a name the channel
has had. Only the agent that started a sub-agent asks or ends it. Starting one asks for approval
only where the component is configured so (`requires_approval`), and is bounded by the limits the
operator sets in the plugin's section of the configuration (`plugins.subagents`, issue 597): no more
sub-agents open at once in a tree of a channel - an agent and all that descend from it there, those
let go not counted - than `max_open`, refused as a parameter, since ending one makes room; none
deeper than `max_depth`, refused as not permitted - by default, a sub-agent starts none of its own.
The writer checks them, on the kernel's side: a component's parameters are the agent side's word.
What the access policy refuses is answered as a refusal, saying what was refused and not by which
rule; anything else as users are told it - a work that may not reach its own channel among them,
which is not found, as a channel one may not reach is.

An agent's owner has it start one by a command of the agent, `/subagent @agent <personality>
<brief>` (issue 596), offered by the same component with the same personalities - the one the
arguments begin with, compared as names are; the rest is the brief - and for the agent's owners
alone: whoever issues it would approve the tool's call, so it asks for no approval. The agent
carries it out in a command turn and delegates as with the tool, and is given to read in the
issuer's name what it started, as `/skill` gives a skill: the personality, the brief and the
thread, since a command turn leaves nothing else in its transcript. The thread hangs at that
message, in the thread the command was issued in, where what the sub-agent answers is given back and
the agent responds; a message given goes to nobody, so the frontends mark the thread at no message
they show, while its opening is announced to everyone. A start that fails gives nothing; one whose
text cannot be given is ended again.

