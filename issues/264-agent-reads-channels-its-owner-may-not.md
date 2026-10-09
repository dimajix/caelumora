# 264 — An agent reads channels of its session that its owner may not read

**Status:** open
**Area:** Identity and access management

## The problem

A user reads a channel of their session if they may read it (`GetChannel`, by ownership: the
channel's owner and its participants) and are cleared for its classification. An agent reads
another channel of its session by a different rule: by whether the source's classification flows
into the classification of the channel it reads from (`ReadChannel`, issues
[17](17-information-sharing-within-session.md), [133](133-classification-model.md)) - not by who
takes part in the source, and not by whether its owner may read it.
`SessionView::readable` (`caelumora-runtime/src/view/session.rs`) makes the two rules explicit:
a viewer with a place - an agent - is asked `may_read_from`, a viewer without one `may_read_channel`
and its clearance.

Since issue [255](255-whose-name-agents-act-in.md) an agent acts with its owner's rights in all it
does - and the policy is asked in a context naming the owner as principal. But `ReadChannel` does
not look at the principal under the ownership policy, so for another channel of the same session
the decision does not use the owner's rights at all. The security suite shows it
(`an_agent_reads_another_channel_with_its_owners_rights_whoever_asked`): alice's channel "side",
which bob may not read, is read by alice's agent when bob asks - which 255 accepts, since it reads
with alice's rights. But bob's agent in the same channel would read "side" as well, though bob,
its owner, may not read it.

## Why it matters

- **Membership is no boundary for agents.** A channel of a session that only some of its people
  take part in keeps its conversation from the others - but not from their agents, if its
  classification flows into a channel they share. Whoever asks such an agent learns it.
- **It contradicts 255's sentence** that an agent reads with its owner's rights: within a session,
  it reads with more.
- **The rule was chosen on purpose.** Issue 133 decided that `ReadChannel` depends on the
  channels, not on the reader, "so that the answer does not change when someone joins or leaves
  one" - what an agent remembered, and what was released to a channel, should stay readable
  however membership changes. History 12 (2026-09-25) records the stricter rule before it: every
  user of the reading channel had to be allowed to read the source.

## Considerations

- **Keep the rule, and say so.** Within a session the boundary is classification, not membership:
  who wants a channel kept from others in the session gives it a compartment the others' channels
  do not read. Cheapest; documents what is. Needs SECURITY.md and CONFIDENTIALITY.md to say that
  membership does not hide a channel from the session's agents, and perhaps a way to make a
  channel's compartment easy to set.
- **Ask the owner's rights as well.** An agent reads a channel as a whole only if its principal
  may read it (`GetChannel` of the source, in the turn's context) besides the flow rule;
  entries released to its channel stay readable regardless. Consistent with 255; the answer then
  changes when the owner joins or leaves the source, which is what 133 avoided - and it would have
  to be decided whether that also holds for what the agent remembered there (issue 131) and for
  its private notes (issue 51).
- **Ask the readers of the reading channel** - every user of it may read the source, the rule of
  12's history. Protects the asker's side rather than the owner's; changes with membership on
  both sides.
- **A policy decides.** `ReadChannel` already carries the context with principal and initiator;
  a policy other than the ownership one may look at them. The ownership policy would still need
  one of the answers above.

Related: [255](255-whose-name-agents-act-in.md) (an agent acts with its owner's rights),
[133](133-classification-model.md) (the flow rule), [17](17-information-sharing-within-session.md)
(reading within a session), [131](131-agent-memory-bypasses-policy.md) (memory recall asks
`ReadChannel`), [51](51-agent-private-notes.md) (private notes across channels),
[19](19-memory-tiers.md) (point 11, reading across channels).

## Decision

An agent reads another channel - its history, its notes, what was released from it - only if
the flow rule lets it **and** its owner may read that channel or the agent itself takes part in it
now. What the agent itself knows - its memory, its private notes - stays under the flow rule
alone: what it heard or wrote, nobody takes from it, which is what 133 wanted to keep. So the
sentence of 255 holds within a session too: an agent reads what its owner may read, and besides
what it took part in itself.

1.  **A release crosses no membership.** A release lowers the level of an entry; it adds no
    readers. An agent reads the released entries of a channel only where it may read that channel
    at all. One rule for a whole channel and for its entries.
2.  **Only current membership counts.** An agent that left a channel reads it as its owner may;
    what it heard there stays with it through its memory and its private notes.
3.  **The policy decides.** `ActionType::ReadChannel` carries the attributes of the source
    channel for what is read of the channel, none for what the agent itself knows; the ownership
    policy asks the flow rule and, given attributes, whether the principal or the caller may read
    them. A different policy may weigh the same data otherwise.

## Plan

Written against the code as of `541f822`. One step with a change in behaviour - what an agent
reads of another channel -, the documents on the subject, then the close.

### A. What the code shows

- **Where `ReadChannel` is asked.** Through `AccessQuestions::may_channel_read(context, reader,
  source)`, from three places:
  - `Views::may_read_from` (`view/mod.rs`), for a viewer with a place - an agent: by
    `SessionView::readable` (`view/session.rs`) for a channel as a whole, which otherwise answers
    `Readable::Released`, and by `ChannelView::may_read_entry` (`view/channel.rs`) for each entry
    of a channel read entry by entry - messages, public notes, private notes;
  - `AgentMemory::may_recall` (`agent/memory.rs`), three times, for what the agent remembered;
  - a test in `session.rs`.
- **The agent's private notes of other channels** are read through the same views:
  `SessionView::private_notes_of` goes over `all_channels()`, reaches the agent there as a member
  (`ChannelView::member`, also a former one), asks `ReadAgentContext` and filters the notes with
  `may_read_entry`. Gated by membership, it would lose its own notes of a channel it left.
- **Agents are contributors of their channels** (`channel::participant_attributes`), so "the
  agent takes part now" is "the caller is among the channel's contributors".
- **The ownership policy** decides `ReadChannel` by `ConfidentialityRules::may_read_from` alone
  (`security.rs`); `may_read(attributes, me)` is its rule for reading an entity.

### B. The model (`caelumora-model`)

1.  `ActionType::ReadChannel { reader, source, channel: Option<&SecurityAttributes> }`: the
    attributes of the source channel when what is read is the channel's - its history, its notes,
    what was released from it -, `None` for what the reading agent itself knows of it. Its doc
    comment says both, and `describe` is unchanged.
2.  `AccessQuestions::may_channel_read(context, reader, source, channel)` passes it on. A channel
    still reads itself without asking.
3.  The ownership policy: `self.rules.may_read_from(reader, source) && channel.is_none_or(|a|
    Self::may_read(a, me) || Self::may_read(a, context.caller_id))` - the principal's rights, or
    the caller's own membership. Its doc comment says so.

### C. The runtime (`caelumora-runtime`)

4.  **The views** pass the channel's attributes: `Views::may_read_from(source, channel)`;
    `SessionView::readable` - a channel the agent may not read at all is no longer `Released` but
    unreachable (`None`), as for a user who may not read it - and `ChannelView::may_read_entry`
    with its channel's.
5.  **The agent's own knowledge** passes none: `AgentMemory::may_recall`, and the private notes.
    For those, `SessionView::private_notes_of` no longer goes over `all_channels()` but over the
    channels the agent takes or took part in, reached as `Readable::Own` - a third way of reading
    a channel: of it, only what the viewer's agent itself wrote there, entry by entry by the flow
    rule alone. Its history, its public notes and its members are not readable then, as of a
    channel of which nothing was released. `ChannelView::may_read_entry` asks with `None` for it.
6.  The test in `session.rs` passes `None`.

### D. The frontends

- Nothing: a user reads no channel through a place.

### E. Tests

7.  **Model:** the ownership policy - flow rule and attributes; the principal may read them, or
    the caller is a contributor; `None` by the flow rule alone; a channel always reads itself.
8.  **Security suite**, beside
    `an_agent_reads_another_channel_with_its_owners_rights_whoever_asked`:
    - bob's agent in the shared channel does not read alice's channel "side", neither as a whole
      nor what was released from it to the shared channel's level, and its tool is told there is
      no such channel;
    - an agent taking part in a channel its owner may not read, reads it from another;
    - what an agent remembered in a channel its owner may not read is recalled as before.
9.  **Unit tests** that build `ReadChannel` or call `may_channel_read` pass the attributes or
    `None`; `view/mod.rs` gets a test of private notes of a channel the agent left and its owner
    may not read, which stay readable to it.

### F. Documents

- CONFIDENTIALITY.md, *Reading another channel*: the condition of membership, that a release adds
  no readers, and that memory and private notes stay under the flow rule; *Classification* where
  it calls a release one "to a wider audience".
- SECURITY.md, *In whose name an agent acts*: within a session too, what its owner may read and
  what it takes part in itself; the ownership policy's paragraph on the confidentiality actions -
  `ReadChannel` now adds ownership.
- MEMORY.md, *Reading other channels*: the same, and that private notes are reached as the
  agent's own.
- Issues: this one (history, done), [133](133-classification-model.md) and
  [17](17-information-sharing-within-session.md) (history: what an agent reads of another channel
  now depends on membership as well).

### G. Steps

1.  **The change** (B, C, E, F): one commit. No stored data changes.
2.  **Close**: this file's history, status `done`.

### H. What to watch

- **A channel's documents are read in the runtime's name** (`SessionView::document`); its
  attributes come with it, current also for a running channel (`ChannelLoop::channel`).
- **The system** reads everything: `SecurityContext::system()` is no agent with a place.
- **External agents** read through the same views (`AgentWork::session`) and are covered alike.
- **The context panel** resolves what a turn read (`Views::channel_of`, `KnowledgeRef`) in the
  viewer's name - a user's, without a place: unchanged.
- **A probe.** A channel the agent may not read answers as absent, also by name
  (`NoChannelNamed`), so that its existence is not told.

## Done when

- It is decided whether membership of a channel limits what agents of its session read, and the
  decision is in SECURITY.md and CONFIDENTIALITY.md.
- If it does, the policy decides it, and the security suite pins an agent whose owner may not read
  a channel.

## History

- **2026-10-08** — Opened at the maintainer's request, found while closing
  [255](255-whose-name-agents-act-in.md) (step 3): reading another channel of the same session
  asks only `ReadChannel`, so whose rights an agent reads with does not decide it.
- **2026-10-09** — Decided with the maintainer (*Decision*): an agent reads another channel only
  if its owner may read it or it takes part in it now, besides the flow rule; what it knows itself
  stays under the flow rule alone. Also decided: a release adds no readers, and only current
  membership counts. Against keeping the rule and documenting it, and against asking the users of
  the reading channel, which would forbid what 255 accepts. A plan is written (*Plan*). Found on
  the way: the agent's private notes of other channels are read through the channels' views and
  need a way of their own, `Readable::Own`, so as not to depend on membership.
- **2026-10-09** — Step 1 done: `ActionType::ReadChannel` carries `channel`, the attributes of the
  source channel or `None`; `may_channel_read` and `Views::may_read_from` pass it on, the ownership
  policy asks the flow rule and, given attributes, whether the principal or the caller may read
  them. The views pass the channel's attributes for its history, its notes and the whole channel,
  none for the agent's private notes (`ChannelView::may_read_entry` with `own`); the memory passes
  none. New tests: in the model
  `what_is_a_channels_is_read_by_an_agent_whose_owner_may_read_it_or_that_takes_part`; in the
  views `an_agent_reads_of_a_channel_its_owner_may_not_read_only_what_it_wrote_there_itself`,
  which fails with the old rule; in the suite
  `an_agent_does_not_read_a_channel_its_owner_may_not_read` and
  `an_agent_reads_a_channel_it_takes_part_in_whose_owner_may_not`, beside the reading test of 255,
  all three sharing `with_a_reader_of`; the memory's test policy admits only what the agent knows
  itself. CONFIDENTIALITY.md, MEMORY.md and SECURITY.md say it; 133 and 17 have an entry.
  Deviating from the plan (C5): no `Readable::Own`. A channel the agent may not read at all is
  reached entry by entry as before - `Readable::Released` - and nothing of what is the channel's
  is released then, so it answers as absent; the agent's private notes there are read as its own
  (`own`), whoever may read the channel. `SessionView::readable` and `private_notes_of` are
  unchanged, and `all_channels` has no other caller. No stored data changes.
