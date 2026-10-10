# 285 — The context and the knowledge it read are set apart

**Status:** open
**Area:** Code structure

## The problem

The `ContextProvider` answers the prompt parts of a turn and the knowledge it read for them as one
value, `(Vec<PromptPart>, Vec<KnowledgeRef>)`. The turn takes them in two calls: whoever asked the
provider records every reference with `Turn::record_knowledge` and then sets the parts with
`Turn::set_context`. That is done in `AgentActor::request_response`
(`actor/agent/mod.rs`) and again in a test of `turn_context_provider.rs`; the external agent does
the same with its work (`actor/external_agent.rs`).

What belongs together by subject - the parts the model is given and what was read to write them -
is split at every caller, and a caller that forgets the first call sets a context whose knowledge
is not recorded: what the turn writes afterwards is labelled below what it saw (issue 168), and
the context panel does not show what was read.

## Considerations

- The same was just done for a message given to an agent ([16](16-context-compression.md), step
  1): `Turn::give(message, knowledge)` takes the knowledge with the message, where `/skill` made
  two calls before.
- `Turn::set_context(parts, knowledge)` records the knowledge, then keeps the parts - one call, the
  order fixed in one place. `record_knowledge` stays for the tools, which read during a turn.
- The external agent's work (`Work`, not a `Turn`) takes the provider's answer the same way; it
  should be given the same shape, so that both kinds of agent take a context alike (issue 204).

## Done when

- A context is set with the knowledge it read in one call, for agents run here and external agents
  alike, and no caller records the provider's knowledge itself.

## History

- **2026-10-10** — Issue created, noted by the maintainer while working on 16.
