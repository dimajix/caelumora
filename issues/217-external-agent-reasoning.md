# 217 — What an external agent says while it works is dropped

**Status:** rejected
**Area:** Integrations
**Summary:** What an external agent reports while it works is dropped, so its messages carry no reasoning and the chat shows nothing until it answers.

## The problem

An agent run here streams its model's reasoning with its answer, and since
[216](216-show-reasoning.md) the web UI shows it, folded into the message. An external agent
([179](179-external-agents-via-a2a.md)) shows nothing of the kind: its messages never carry
`reasoning`, and while its remote agent works, the chat shows only that it is generating.

A2A has no reasoning of its own. A message consists of parts - text, file, data - and what a remote
agent says about its work reaches the caller in three ways, none of them standardized as reasoning:

- **Status updates while working.** A `TaskStatusUpdateEvent` in the state `working` (or
  `submitted`) may carry a message - progress such as "searching ...", or whatever the remote agent
  chooses to report. The translation drops these on purpose
  (`caelumora-runtime/src/external_impl/a2a_agent/translation.rs`, `status_events`): "what it says
  meanwhile is not the answer".
- **`metadata` on parts and messages.** Free-form; some implementations mark a text part as thought
  this way, by a key of their own.
- **Extensions.** An agent card can declare extensions (A2A 0.3 and 1.0). A "thinking" extension
  would be the clean way, but none is established that we know of.

## Why it matters

- A remote agent may work for minutes; without anything shown meanwhile, the chat looks stuck.
- What it reports while working explains its answer the way reasoning does for an agent run here.

## Considerations

- **Status texts as reasoning.** The texts of `working` updates become the reasoning of the message
  under way: streamed as partials with `reasoning` set, and kept on the message once the task
  completes. Works with every A2A agent that reports progress, and needs no convention; but what an
  agent reports is not necessarily reasoning, and some repeat the whole status each time instead of
  adding to it - to be seen with real agents, Hermes first.
- **A convention per agent.** Read a `metadata` key or an extension the remote agent declares,
  configured on `a2a_agent`. Exact where an agent offers it, but nothing works without knowing the
  agent; and a declared extension would better be read from the agent card than configured.
- Both can be combined: status texts by default, a declared marker where there is one.
- **Who sees it.** The remote agent is only given what is published to it, so its reasoning holds
  nothing the channel may not see; the personality's `reasoning_visibility` still applies, as for an
  agent run here (see [CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md#reasoning)).
- **Before deciding:** record what Hermes actually sends while it works - states, status texts,
  metadata - since the choice depends on it.

## Done when

- What an external agent reports while it works reaches the chat as the reasoning of the message
  under way, and stays with the message once it is complete.

## History

- **2026-10-03** — Issue created at the maintainer's request, after asking whether A2A carries
  reasoning: it does not as such; status updates, metadata and extensions are the ways an agent can
  report it.
- **2026-10-04** — Recorded what Hermes sends, with one streamed call (`SendStreamingMessage`,
  about 16 s) and the finished task read with `GetTask`. Its agent card declares no extensions. The
  stream held four events: the task `TASK_STATE_SUBMITTED`, a status update `TASK_STATE_WORKING`
  without a message, one artifact update with the whole answer at once, and `TASK_STATE_COMPLETED`.
  The finished task held the answer twice, as `status.message` and as an artifact. Nowhere was
  there `metadata`, reasoning or token usage. So with Hermes neither option yields anything - its
  working status says nothing, and it has no convention to read - and it reports no usage either,
  so issue 209's gauge stays unknown for it.
- **2026-10-04** — Rejected by the maintainer: the one external agent in use reports nothing while
  it works, so there is nothing to show. Should an A2A agent that reports its reasoning or progress
  be connected, the options above are where to start.
