# 206 — Agents run here and external agents keep their channel state in two types

**Status:** open
**Area:** Code structure

## The problem

An agent's state in a channel is one of two types, stored and read apart:

| | `AgentChannelState` (agent run here) | `ExternalAgentChannelState` (issue 179) |
|---|---|---|
| shared | `history: MessageHistory`, `transcript: Transcript` | `history: MessageHistory`, `transcript: Transcript` |
| own | `processing_state`, `notes`, `private_notes`, `prompts`, `tool_sets`, `turn` | `offered_up_to`, `conversation`, `open_tasks` |

The parts they share are the same types with the same meaning - what the agent heard and said, and
what it was given - but everything built on them exists twice or for one side only:

- the `StateStore` has `get_/put_agent_channel_state` and `get_/put_external_agent_channel_state`,
  each with a change type of its own (`AgentChannelStateChange`, `ExternalAgentChannelStateChange`,
  issue 193);
- the views read them through two `AgentMember` implementations and two `AgentContextView`s
  (`InternalContext`, `ExternalContext`), of which the external one answers most questions with
  nothing;
- `ContextResolver::of` and `TranscriptView::of` take an `AgentChannelState`, so a context
  recorded for an external agent ([205](205-external-agent-message-context.md)) cannot be resolved
  without changing them;
- catching up on what was missed (`catch_up`, issue 190) is written once per actor kind.

## Why it matters

- Every capability built on the history or the transcript - showing contexts (205), given-up
  turns, branches (issue 08), notes an external agent might one day keep - has to be built twice
  or is missing for external agents.
- The difference between the two kinds is how an agent produces its answers, not what it keeps of
  the channel; two state types make it look like the latter.

## Considerations

- **One state with a part per kind.** `AgentChannelState` keeps what every agent keeps - history,
  transcript, perhaps notes, prompts and tool sets (empty for an external agent) - and an enum or
  an optional part for what only one kind has: the turn of an agent run here, the conversation,
  open tasks and `offered_up_to` of an external one. One pair of store methods, one change type,
  one context view.
- **A shared core, two states.** A type for the shared part that both states contain, and
  resolvers, views and store helpers written against it. Less of a change, but the store and the
  change types stay doubled.
- **Stepwise.** First make what reads the shared parts (resolver, transcript view, context view)
  take them instead of a whole `AgentChannelState` - which 205 needs anyway - then merge the
  states.

Stored states of external agents may be deleted rather than migrated, as decided for
[201](201-component-nodes-wrapper.md), unless decided otherwise here.

## Done when

- Agents run here and external agents keep their channel state in one type, stored through one
  pair of `StateStore` methods with one change type.
- What reads history and transcript - context, given-up turns, catching up - is written once.
- STORAGE.md and CHANNELS.md describe the one state.

## History

- **2026-10-03** — Issue created as the target named in
  [205](205-external-agent-message-context.md): reuse what exists for the context of an external
  agent's message, and perhaps merge the two states.
