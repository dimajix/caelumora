# 51 — An agent has no private knowledge across its channels

**Status:** done
**Area:** Context & knowledge

## The problem

An agent that takes part in several channels of a session knows in each of them only what was said
there. What it learnt in one channel is not available to it in another, unless it was written into
the channel's shared notes ([17](17-information-sharing-within-session.md)), which every channel of
the session can read.

A natural person keeps what they learnt in one conversation and can use it in another, at their
discretion. `TODO.md` names this tier "agent session memory"; the terms in [19](19-memory-tiers.md)
keep it.

## Why it matters

- It is what makes an agent that works in several channels one participant rather than several
  unrelated ones.
- Without it, an agent that wants to remember something across channels has to write it into shared
  notes, where everyone can read it, although it was meant for the agent alone.

## Considerations

Decided on 2026-09-25 (principle 12 in [19](19-memory-tiers.md)):

- **Private notes are a chain per agent and channel**, kept in the agent's `AgentChannelState` next
  to its history and context, on the chain type of [48](48-shared-digest-chain.md). They come from
  the conversation in that channel and branch with it, like the agent's history and context. Notes
  and revocations, provenance, and the state of knowledge as a digest work as for the shared notes
  of a channel.
- **Agent session memory is a view**: the union of the agent's active private chains across all its
  channels of the session. No other agent can read it. `AgentState` is not needed for it.
- **Reading across channels is recorded**: a private note from channel A used in channel B is
  recorded as a `KnowledgeRef` with source "private notes".
- **Every private note carries the confidentiality of its origin.** Private knowledge makes the
  agent a bridge between channels on purpose, and a model has no reliable discretion: what it knows
  from A it may tell anyone in B, including someone who may not read A. So a note carries the
  security attributes of the channel it was written in, and appears in channel B only if B may read
  A under the rule of point 11 in [19](19-memory-tiers.md) (strict until
  [12](12-user-and-agent-permissions.md) decides). The agent *knows* everything, but *recalls* it
  only where that tells no one anything new. When [12](12-user-and-agent-permissions.md) relaxes the
  rule, private knowledge follows.
- Rejected: private notes visible in all of the agent's channels, relying on the model's discretion.
  Closest to the analogy, but a real leak between channels with different participants.
- Possible later: a note marked as usable everywhere when written (e.g. the owner's preferences),
  only with approval.
- Comes after phase 2 of the order of work in [19](19-memory-tiers.md), whose reading mechanism,
  `KnowledgeRef` and access rule it uses.

## Done when

- An agent can write, revoke and read private notes in each of its channels.
- In a channel, the agent is given its private notes from every channel of the session whose
  confidentiality allows it, recorded as `KnowledgeRef`s.
- Private notes branch with the channel they were written in.

## History

- **2026-09-25** — Issue created: the tier "agent session memory" is kept, as private notes of an
  agent per channel, with the confidentiality of their origin (option 1 of the discussion; principle
  12 in [19](19-memory-tiers.md)).
- **2026-09-25** — Notes turned out to belong to the agent as well ([19](19-memory-tiers.md),
  principle 2): private notes are the second chain of the same mechanism, next to the agent's public
  notes per channel, and differ only in who may read them.
- **2026-09-25** — Implemented, as decided with the maintainer before:
  - A component of its own, `private_notes`, so that private notes can be enabled per personality
    apart from public ones; tools `note_private` and `revoke_private_note` (separate tools rather
    than a `private` flag on `note`, harder for a model to mix up) and a `BeforeTurnInput` prompt
    part. Parameter parsing is shared with `notes` (`note_text`, `note_id`).
  - `AgentChannelState::private_notes` and `turn_private_notes`; the `TurnContext` note operations
    work on either chain through shared helpers (`add_note`, `revoke`). Private notes are numbered
    apart from public ones, so gaps in the public numbers reveal nothing. `record_turn` takes both
    over; only public changes are announced (`NotesChanged`).
  - Reading: `SessionReader::private_notes(agent)` reads the agent's private chains of the other
    channels of the session that the current channel may read (`ChannelLoop::may_read`, the rule of
    point 11 in [19](19-memory-tiers.md)); the current channel's come from the turn itself. The
    confidentiality is checked against the channel's *current* attributes when read, not frozen per
    note, so a user later removed from a channel ([46](46-channel-participants.md)) does not keep
    access through old notes.
  - `KnowledgeSource::PrivateNotes { agent, digest }`; REST reports it as `private_notes`, the
    context panel as "Private notes of X in #c".
  - Not in the "Notes" tab; the owner sees them in the context panel, which `ReadAgentContext`
    reserves to the owner. Deleting a channel deletes the private notes written there.
  - Found on the way: `StateContextProvider` collected its prompt providers from a `HashMap`, so the
    order of parts at one position - system prompt and planning prompt, now notes and private
    notes - could change between starts and with it the prompt and its cache. It now follows the
    order of the declared dependencies.
  - Verified in the browser with a `mentions-session` and a second channel `research` (Jonathan and
    the user): Jonathan noted a code word with `note_private` in `research`; asked in `main`, whose
    conversation never contained it, he answered it correctly. His context there shows the prompt
    part "From channel research" and the knowledge `private_notes Jonathan #research`; the "Notes"
    tab of `research` stays empty.
  - Observed, and consistent with point 11 of [19](19-memory-tiers.md): once Jonathan answered in
    `main`, the other agent there (Sassy) received the answer like every message. The rule guards
    users, not agents; an agent that recalls a private note where it may still passes it on to the
    channel it speaks in.
