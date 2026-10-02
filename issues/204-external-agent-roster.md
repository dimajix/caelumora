# 204 — An external agent is not told who is in its channel

**Status:** open
**Area:** Integrations

## The problem

An agent of our own learns who is in its channel from `channel_members_prompt_provider`
([176](176-participants-in-system-prompt.md)): its own name, the others with their `@` handles and
introductions, and that a member is addressed by mentioning its handle. An external agent
([179](179-external-agents-via-a2a.md)) learns none of this. A `PromptProvider` contributes to the
context a `Turn` builds for a model, and an external agent has neither: its components are its
`ExternalAgent` (such as `a2a_agent`) and perhaps `PerceiveInProcessor`s. When it is asked,
`ExternalAgentActor` sends the remote agent only what was said since it was last asked, as
`Said { sender, text }` (`caelumora-runtime/src/external.rs`).

So the remote agent knows the names of those who spoke, but not who else is there, under which
handle, nor that a mention exists. Since [45](45-agent-mentions.md) a mention written by an external
agent makes the mentioned agents respond - but it would have to guess the notation and the handles.

## Why it matters

- Delegation between agents, the reason for [45](45-agent-mentions.md), is open to our own agents
  only; an external agent can be addressed but cannot address anyone.
- It cannot tell a person from an agent, nor whom it is talking to when someone joins who has not
  spoken yet.

## Considerations

### What is sent, and how

1. **A leading part of the request.** `OutboundMessage` gains the roster as text of its own (e.g.
   `members: Option<String>`), which `a2a_agent` sends as a first part of the message. It is sent
   with the first request of a conversation and again whenever the roster changed since it was
   last sent; the external agent's channel state remembers what it sent. Every remote agent reads
   it, since it is part of what it is given.
2. **Metadata of the A2A message.** The roster travels as structured metadata. Cleaner, but only a
   remote agent that knows to read it benefits; to most it is invisible.
3. **Every request.** Simplest, but repeats the roster in the remote conversation each time.

Option 1 looks most useful; whether the part is plain text, or text plus metadata (option 2 on
top), is to be decided.

### Where the roster is made

The text of the roster lives in `channel_members_prompt_provider` (`render`, `named`, `one_line`),
built from a `Turn`. An external agent needs the same text from its channel without a turn. The
roster - reading the members and rendering them - should move to one place both use, rather than be
written a second time; whether that is a function over the channel's members, a method of the
channel's view, or a component facet that the external agent may be configured with (so that it can
be switched off, as for our own agents), is the architecture question of this issue.

### What leaves the system

Names, handles and introductions are public (176), so sending them out needs no new check. It is
still something the remote agent was not given before; whether it is sent should be configurable
per external personality, as the prompt part is per personality.

## Done when

- An external agent's remote agent is told its own name, the others with their handles and kinds,
  and how to address one, when a conversation begins and when the roster changes.
- The roster is made in one place for our own and for external agents.
- It can be switched off per personality.
- CHANNELS.md (external agents) and COMPONENTS.md say so.

## History

- **2026-10-03** — Issue created, after asking whether an external agent gets the roster from
  `channel_members_prompt_provider`. It does not: prompt providers act on a `Turn`, which an
  external agent does not have.
