# 68 — Notes are bound to one channel of the agent

**Status:** done
**Area:** Context & knowledge

## The problem

An agent's notes and private notes ([17](17-information-sharing-within-session.md),
[51](51-agent-private-notes.md)) are chains in its `AgentChannelState`, one
per channel. What an agent knows is thereby split by channel: its "session
memory" is the union of those chains, read across channels by the rules of
[61](61-reasoning-leaves-the-agent.md), revoking works only where a note was
written ([60](60-note-ids-in-prompts.md)), and nothing reaches another
session. Once `AgentState` - the state of the agent itself, across its
channels and, for agents of user or global scope, across sessions - is kept
consistently ([62](62-json-store-concurrent-writes.md), step 3), notes could
live there instead.

## Why it matters

- A note that is about the agent's knowledge, not about a conversation, has
  no natural home in one channel.
- Revoking across channels ([60](60-note-ids-in-prompts.md)) would become
  simple: one chain, one writer (the agent state's owner).
- Agents of user or global scope take part in several sessions; knowledge
  that outlives a session ([18](18-knowledge-sharing.md)) is what they would
  need.

## Considerations

- **Branching** ([08](08-retry-and-edit-past-answers.md), principle 1 in
  [19](19-memory-tiers.md)): notes in `AgentChannelState` branch with the
  channel's history - a regenerated answer drops the notes its turn made.
  A chain in `AgentState` does not belong to one channel's history; its
  entries would have to record the channel and message they came from, and
  a branch switch would have to revoke or mark the entries of the abandoned
  branch (principle 6: nothing is deleted, reads based on them are marked).
- **Confidentiality of origin** (principle 12, rules of
  [61](61-reasoning-leaves-the-agent.md)): every entry must carry the level
  and place (session, channel) it was written in, and be recalled only
  where that channel's contents may be read - within a session by rule 2,
  across sessions only if its level is `public`. Today this falls out of
  the chain belonging to a channel; in `AgentState` it has to be recorded
  per entry.
- **Public notes** are published to the channel's other agents and users
  (`NotesChanged`, the notes panel): they are about the conversation and
  probably stay per channel; the question is mainly private notes.
- Replace, or add: a third kind beside the channel-bound ones - agent-wide
  memory, committed deliberately (principle 7 in [19](19-memory-tiers.md):
  knowledge that outlives the session is committed, not written) - may fit
  better than moving the existing ones.
- Builds on [62](62-json-store-concurrent-writes.md) step 3 (one owner of
  `AgentState`, changes as deltas) and on [65](65-generic-component-state.md)
  (component state without the core knowing its kind).

## Done when

- It is decided whether private notes (or a new kind of agent-wide notes)
  live in `AgentState`, and how branching and confidentiality of origin are
  kept - or recorded as rejected, with the reason.

## History

- **2026-09-28** — Issue created at the maintainer's request while starting
  step 3 of [62](62-json-store-concurrent-writes.md): notes an agent creates
  across channels and sessions are a candidate first user of `AgentState`,
  and the current notes might move there.
- **2026-09-28** — Decided with the maintainer. Since step 3 of
  [62](62-json-store-concurrent-writes.md), agent memory (`agent_memory`,
  in `AgentState`) is the "third kind" considered above; private notes and
  memory overlap in who sees them and in the recall rule, and differ in
  scope (channel vs. agent, session vs. beyond), in revoking across channels
  and in branching. Proposed was to retire private notes in favour of
  memory. Decided instead: **both stay, as alternatives a personality
  chooses from** - the flexibility the component architecture is for. The
  seeded personalities show both: one with `memory`, one with
  `private_notes`. Private notes do not move into `AgentState`.
  - **Memory is written directly** (`remember`), not committed in a separate
    step. The deviation from principle 7 in [19](19-memory-tiers.md) is
    deliberate: what matters is that memory from public channels is not
    bound to a session - deleting the session keeps the knowledge (the
    `AgentState` is not deleted with a session). Entries from `session` or
    `channel` channels stay recallable only there; once that session is
    gone, nothing can recall them any more.
  - For memory to outlive a session in practice, the agent has to outlive
    it too: agents of `SessionScope` are created anew for every session. The
    seeded memory agent is therefore of `UserScope`, reused in every session
    of the same user.
- **2026-09-28** — Implemented in the seeded data: `create_personality`
  takes `PrivateKnowledge::Memory` or `PrivateKnowledge::PrivateNotes`;
  `jonathan-assistant` has `agent_memory`, `sassy-assistant`
  `private_notes` (and the context provider depends on the one it has).
  Jonathan is `UserScope` in the templates that call him "Jonathan".
  Verified with fresh stores: Jonathan remembered an entry in a `public`
  channel; the session was deleted; in a new session he is the same agent
  and recalls it ("Yes, #1."). No code outside the seed changed - both
  components existed already.
