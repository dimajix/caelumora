# 133 — Confidentiality levels mix sensitivity with topology

**Status:** open
**Area:** Identity and access management

## The problem

The confidentiality levels of [61](61-reasoning-leaves-the-agent.md) -
`public < session < channel < private` - say *how far* a piece of
information may travel, not *how sensitive* it is. Two of them are structure
(`session`, `channel`: where in the tree of sessions and channels the
information stays) and one is ownership (`private`: an actor's own, which
"is an actor's level, not a channel's", as `docs/CONFIDENTIALITY.md` has to
explain). Only `public` is a classification. Consequences:

- Information that belongs to a topic or a group - HR, a customer, a project
  - cannot be told apart from information that merely belongs to a session
  ([64](64-confidentiality-compartments.md)).
- Two channels may only be shielded from each other by their position in
  the tree; that neither of them may see the other's contents cannot be
  said of two channels of the same session except by isolating both
  completely (`channel`).
- A tool's results are labelled by the tool, once and for all
  (`ToolConfidentiality`). A tool that reads a knowledge base, or an MCP
  tool with mixed sources, cannot say that *this* result is more sensitive
  than *that* one.
- Messages written by users carry no label at all; a user cannot say that
  one message is more sensitive than the channel it is written in would
  need it to be.

## Why it matters

- Classification is what deployments in organisations already have and
  understand: a level from a fixed scale and, for what it applies to, the
  area it belongs to. The current reaches are a substitute for it.
- Labels carried by the data, not by their source, are what allow
  knowledge beyond the session ([18](18-knowledge-sharing.md)), retrieval
  ([59](59-retrieval-of-notes.md)) and external sources to respect the
  rules rather than the rules being re-implemented at every source.

## Considerations

Decided with the maintainer on 2026-09-30 (see History); the rest of this
section records the model and the points that were weighed.

### The classification

A classification is a **level** and a set of **compartments** that is the
information's *audience*:

- The levels are `public < internal < confidential < strictly_confidential`.
  They replace the reaches; `session` and `channel` disappear as levels.
- A **compartment** is an area - a topic, a group, a customer. The set of
  compartments is managed by the administrator, not created ad hoc by
  session owners. `public` takes no compartment: public is public, and a
  request that pairs them is refused (like `private` for a channel today).
- The compartments on an entity are its **audience**: who may read it.
  Adding a compartment *widens* the audience. An entity without
  compartments is *general*: readable by everyone of a sufficient level.
  In practice nearly everything carries one compartment, the one of the
  channel it arose in; more than one is the result of a release (below).
- The administrator also configures, per compartment, **which other
  compartments it may read**. The core takes the transitive closure of
  that: if HR-management may read HR and HR may read Finance, then
  HR-management reads Finance too. Transitivity is not optional - HR's
  notes may contain what HR read from Finance (see derivation) - so the
  closure is computed, not left to the administrator.
- **The flow rule** is the one relation the core needs. Information may
  flow from a source S to a destination D (a part into a channel, a
  channel's contents into another, a result into an agent's context) if
  D's level is at least S's, and every compartment of D is covered by one
  of S's - the same, or one that it may read - or S has none. For a
  destination with one compartment this reads "any of the source's
  compartments will do"; for a channel with two, everything in it must be
  released to both, since both audiences see it. Today's two rules -
  `admits` for publishing and `readable_from` for reading - are both this
  question.
- The rules are policy ([24](24-opa-security-manager.md)): the compartment
  set and the read relation are configuration of the `SecurityManager`, and
  the questions stay `ActionType::Publish` and `ActionType::ReadChannel`.

Two other readings of several compartments were weighed and rejected:

- **Conjunctive** (the classic lattice: the compartments are what the
  information *touches*, the reader needs all of them, derived knowledge
  takes the union). Sound, but the rules read the wrong way round - more
  compartments means stricter, an empty set means general - and a user
  classifying a message has to think in sets. Rejected as of no use in
  practice.
- **One compartment, combinations as administrator-defined nodes** of the
  read hierarchy (`HR-Finance` reads HR and Finance). Keeps entities to one
  compartment, but every combination that occurs needs an administrator
  first. Rejected as too complex for what it buys.

The audience reading is the one users think in, and it is what a release
changes. That a set can mean "any" or "all" is not an ambiguity of the
model but of the word: a classification's compartments are always an
audience, and the flow rule above says how an audience of several is
judged.

### Release and reclassification

The labelling of an entity may be changed by an actor authorised to do so:
a natural person, or an agent through a tool that asks for approval. The
usual change is a **release**: widening the audience by a compartment, or
lowering the level, so that a wider circle may read what was written for a
narrower one. A user may also release when writing - a message classified
`{HR, Finance}` in an `{HR}` channel is admitted (the channel's audience is
covered) and readable from Finance from the start. Narrowing is allowed
too, but retracts nothing already read.

- It is its own question to the policy (an `ActionType` with the entity,
  its current and its new classification and the actor), so that a
  deployment decides who may release what.
- A release applies to the entity itself, not to what others derived from
  it before: a note written in HR-management from an `{HR}` message keeps
  its own classification when the message is released to Finance. This is
  the price of derived knowledge carrying the channel's classification
  rather than the sources'.
- Changing a channel's classification ([67](67-change-channel-level.md))
  is the same act on the channel.

### What carries a classification

Every channel, every message, every note and every memory entry:

- A **channel** has a classification, set by its owner (template or
  request), default `internal` without compartment - the closest to
  today's `session`. It admits what may flow into it. Changing it
  afterwards is [67](67-change-channel-level.md), now for a classification.
- A **message** carries one. A user's message is classified by the user,
  default the channel's; the channel refuses one that may not flow into
  it. A message classified for a *wider* audience or a lower level than
  its channel is deliberately readable from further away than the rest of
  the channel - this is what makes labels on messages worth more than the
  channel's classification alone.
- **Tool results** are classified by the tool, per result, when it returns
  - from its input, from what it read, from the source it drew on. This
  replaces the static `content` label of `ToolConfidentiality`; the `call`
  label (whether the call itself is shown) is ownership (below), not
  classification.
- **Derived knowledge** - private notes, agent memory, reasoning - carries
  the classification of the channel it arose in, as it does today. This is
  sound because of the enforcement rule: the agent never sees anything
  that may not flow into its channel, so everything it wrote may be read
  by the channel's audience and no wider.

### Enforcement

- **Before the model, not after it.** A tool result that may not flow into
  the channel is withheld from the agent by the `GuardedToolExecutor` (the
  guard in front of the executor, as for permissions - issue 127) and
  replaced by an error that names the reason: classified above this
  channel. The model never sees it, so the answer text - which cannot be
  labelled - cannot quote it. This is the enforceable form of "the channel
  rejects an answer"; rejecting after the model has read the result would
  leave the answer text as the leak of [66](66-channel-level-in-prompt.md).
- **Per part, at the one place.** Publishing is decided per labelled part
  in `Turn::send_event`, since a result's label is only known when it
  returns; the once-per-turn `Publication` goes. Reading remains per read.
- **Joining** a channel can now be checked (`ActionType::JoinChannel`,
  [46](46-channel-participants.md)): a user or agent has a clearance - a
  classification - and may join a channel whose contents may flow to it.
  Not part of the first step, but the model expresses it.

### Ownership is a separate axis

`private` today means "the agent sees it, no channel ever does": private
notes, `HIDDEN` and `CALL_ONLY` calls. That is not sensitivity but
ownership, and it stays as its own flag beside the classification - every
entity has an owner, and owner-only visibility is checked against that.
The classification says how far something may travel among those who are
not its owner; the flag says whether anyone but the owner sees it at all.

### What is given up

- **Sessions are no longer a confidentiality boundary.** Whether one
  channel may read another depends on their classifications, not on
  whether they share a session. Two `internal` channels of different
  sessions may read each other. Whether a session may read another at all
  is a question of *privacy* - the participants' consent - not of
  confidentiality, and is [134](134-session-privacy.md).
- **"Only this channel"** (`channel`) has no direct counterpart. A channel
  that must not be read by any other gets a compartment of its own, or
  waits for [134](134-session-privacy.md).

### Steps

Each leaves the system working:

1. The classification type with levels, an audience of compartments and
   the flow rule; the policy holds the compartments and the read relation.
   Channels carry a classification; `readable_from` and `admits` become
   the flow rule. Today's behaviour is the special case of no compartments,
   except for the session boundary, which [134](134-session-privacy.md)
   restores in its own way.
2. Messages, private notes and agent memory carry the classification; the
   REST API and the web UI show and set it.
3. Tool results are classified by the tool per result, checked by the
   `GuardedToolExecutor` before the model sees them, and published per
   part.
4. Release: an authorised actor widens an entity's audience or lowers its
   level, through the REST API and the web UI, and for agents through a
   tool that asks for approval.
5. Clearances of users and agents; joining is checked against them.

Touched: `caelumora_model::confidentiality`, `Channel`, `ChannelTemplate`,
`Message`, `ToolCall`, `ToolCallResult`, the `Tool` trait, private notes,
`AgentMemory`, `Publication` and `Turn::send_event`, `GuardedToolExecutor`,
the REST channel and message resources, the web UI's channel dialog and
message rendering, and the prompt notes of
[66](66-channel-level-in-prompt.md). `docs/CONFIDENTIALITY.md` is rewritten.

## Done when

- A classification is a level from `public < internal < confidential <
  strictly_confidential` and an audience of compartments; `session`,
  `channel` and `private` are no longer levels, and owner-only visibility
  is a flag of its own.
- Compartments and what each may read are configured by the administrator
  in the policy, and the core decides publishing and reading by the flow
  rule, with the read relation closed transitively.
- Channels, messages, notes, memory entries and tool results carry a
  classification; a result that may not flow into the channel never
  reaches the model.
- An authorised actor can release an entity to a wider audience or a
  lower level, and the policy decides who is authorised.
- `docs/CONFIDENTIALITY.md` describes the model.

## History

- **2026-09-30** — Opened from a discussion with the maintainer, who
  proposed the model: four sensitivity levels instead of reaches, one
  administrator-managed set of compartments with a configurable read
  relation between them, a classification on every channel, message, note
  and memory entry, and tool results classified by the tool per result and
  checked by the channel. Weighed and decided: no compartment means
  general, not stricter; derived knowledge carries the channel's
  classification, which requires the read relation to be transitive;
  ownership (`private`) stays as its own axis; the check on tool results
  happens in the `GuardedToolExecutor` before the model sees the result;
  the session ceases to be a confidentiality boundary, and reading across
  sessions becomes a privacy question ([134](134-session-privacy.md)).
  Supersedes [64](64-confidentiality-compartments.md).
- **2026-09-30** — The compartments were first decided as exactly one per
  entity, for the readability of the rules. Reopened the same day on the
  question of several compartments: of the three readings - conjunctive
  (all required), audience (any suffices, judged per compartment of the
  destination) and one compartment with combinations as administrator
  nodes - the maintainer chose the audience, since it is what a release
  to a wider circle changes; an entity usually starts with the one
  compartment of its channel, and authorised actors widen it. The flow
  rule and the section on release record this.
