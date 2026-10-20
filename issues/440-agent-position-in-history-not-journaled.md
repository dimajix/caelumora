# 440 — Where an agent stands in its channel's history is lost with the journal

**Status:** open
**Type:** bug
**Priority:** high
**Area:** Persistence
**Summary:** The journal of an agent's channel state records neither what of the channel's history the agent took in (`heard`) nor its last message (`last_message`), so a state loaded again stands where its last snapshot left it, and its channel delivers everything since once more - old requests to respond among it, which every agent then answers again.

## The problem

`AgentChannelState::changes` records the notes, the private notes, `processing_state`, `turn`,
`used`, `remote` and `transcript_from`, but not `heard` ([223](223-full-inbox-drops-events.md)) and
not `last_message` ([417](417-user-history-from-its-actor.md)). Both are written only with a
snapshot, which the `JsonFileStore` writes once the journal is at least as large as the snapshot
and 64 KiB ([193](193-external-state-rewritten-whole.md)). Loaded, a state holds `heard` as that
snapshot had it, and the channel's inbox delivers from there on again: every request to respond
since, of which the agent answers the last.

Seen in the `team-session` of the seed after a restart: the four agents asked to answer a
message of alice's had finished their turns before the shutdown and were `Idle`; after the restart
all of them began a turn answering that message again - their snapshots, written the evening
before, stood at the first few positions of the history, the message at 93. Jonathan answered a
question of alice's he had answered before. The answers started the next round, until
`max_agent_hops` cut it off.

It holds for every load of an agent's channel state, not only after a restart. Since
[388](388-resume-interrupted-work-after-a-restart.md) a restart starts the sessions with work under
way by itself, and with them the agents that had none.

The test of the journal, `what_changed_in_an_agents_channel_state_replays_onto_what_was_stored`,
compares the whole state replayed with the state, but changes neither field.

## Considerations

- **Journaled whole**, as `used` and `remote` are: `heard` is a position and the positions taken
  ahead of it, `last_message` an id - small, and changed in place at almost every put.
- **Stored data.** Variants added to `AgentChannelStateChange` leave the stored states readable. A
  state stored before holds its old position until its next put, so its channel delivers once more
  what it had heard; deleting the agents' channel states of a session avoids that.

## Done when

- `heard` and `last_message` survive a put to the journal and a load, in the replay test and in
  the state store suite over a reopened `JsonFileStore`.
- An agent that finished its turn before a restart does not answer again after it.

## History

- **2026-10-20** — Opened after a restart of the `team-session`, in which every agent began a turn
  answering what it had answered before the shutdown.
