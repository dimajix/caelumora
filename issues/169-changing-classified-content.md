# 169 — Nothing says who may change something classified, or what a change must respect

**Status:** open
**Type:** feature
**Priority:** high
**Area:** Identity and access management
**Summary:** Nothing says who may change something classified once it is written, or that what the writer has seen must fit into its classification.

## The problem

Classifications and clearances ([133](133-classification-model.md)) decide who may *read* something.
Nothing decides, as one rule, who may *change* it and what a change has to respect:

- **Who may change it.** Channels and sessions carry `SecurityAttributes` (owner and contributors),
  and `UpdateChannel` is asked of the policy. What lives inside them - notes, memory entries, and
  soon memory documents ([166](166-memory-as-topic-documents.md)) and a shared workspace
  ([19](19-memory-tiers.md), point 10) - has no such attributes. Who may change it is decided ad
  hoc: the agent that wrote it, through its tools.
- **What a write must respect.** Something that already exists is changed from a place with a label
  of its own: an agent's turn in some channel, or a user in some channel. Nothing states that what
  the writer has seen must fit into what is changed. Today it holds for messages because the channel
  checks every message that enters it (`Publish`), and for ledger entries because each is added once
  with its channel's classification. For something that is revised from several places it holds by
  nothing.
- **Where a change was made.** A ledger records for every change where it was made
  (`LedgerEntry.origin`). A message records its sender. Something revised in place needs the same,
  or it cannot be told afterwards which channel or turn wrote which version.

Issue [167](167-origin-and-releases.md) tried to answer this with a label of origin and releases per
entry. It was rejected: something classified carries exactly one classification, and the question it
really raised is this one.

## Why it matters

- [166](166-memory-as-topic-documents.md) needs it. A memory document is revised from several
  channels, and how it stays classified correctly is decided here, not there.
- Without a rule, every component that holds revisable content invents its own, and some will let an
  agent write what it read in a high channel into something readable in a low one.

## Considerations

- **Two questions, two axes.** *Who* may change something is ownership: `SecurityAttributes`, as for
  channels and sessions. *What* a change must respect is information flow: the classification.
  Neither answers the other.
- **A fixed container rather than a floating label.** The thing that is changed keeps the
  classification its owner gave it. A write does not change it. Changing it is an act of its own, by
  its owner, as reclassifying a channel is today. The alternative - the classification rises to the
  join of everything written - lets one sentence make a whole document vanish from lower channels,
  and classifications creep upwards over time.
- **The rule for writing**, as proposed: someone may change something if
  - their `SecurityAttributes` allow it - they own it or contribute to it,
  - they may read it - clearance and flow, as now; a blind overwrite makes no sense, and
  - their label flows into its classification - for an agent the label of its turn
    ([168](168-turn-label-as-high-water-mark.md)), checked as `Publish` checks a message entering a
    channel. For a user the last point can be judged only by the channel they act in. What a person
    knows is not tracked. As with messages, they are trusted to classify what they write, bounded by
    their clearance.
- **A write that does not fit is refused.** The writer then creates something new, classified high
  enough, or the owner raises the classification first. Raising and lowering stay owner acts,
  recorded for [140](140-audit-log.md).
- **Who owns what an agent writes.** The agent, its owner, or the channel's owner. For notes and
  memory the agent writes them, but its owner may want to correct or delete them. To be decided
  here, and per kind of content.
- **Where a change was made.** Recorded per change, as `LedgerEntry.origin` does: channel, turn or
  actor, time. This is a record, not a classification. It does not affect who may read.
- **One question to the policy.** Changing something classified is asked as one `ActionType`, with
  the thing's attributes, its classification and the writer's label, so that a policy other than the
  ownership policy decides all of it ([138](138-confidentiality-policy-in-ownership.md)).

## Done when

- Everything that can be changed after it was written carries `SecurityAttributes`, and changing it
  is asked of the policy as one question.
- The rule for writing is stated in `docs/CONFIDENTIALITY.md`: who may change something, that the
  writer's label must flow into its classification, and that its classification changes only by an
  act of its own.
- Every change records where it was made.

## History

- **2026-10-01** — Opened at the maintainer's request after [167](167-origin-and-releases.md) was
  rejected: something classified has exactly one classification, but who may change it and what a
  change must respect deserve an issue of their own, as the basis for
  [166](166-memory-as-topic-documents.md).
