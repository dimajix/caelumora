# 167 — A classification says who may read something, but not where it was written

**Status:** rejected
**Area:** Identity and access management

## The problem

Everything that holds information - a message, a tool result, a note, a memory entry - carries
exactly one `Classification` (issue 133). That one value has to say two things that are not the
same:

- **Where it was written** - the authorship: the channel, or the turn's label, it was produced
  under. Its *origin* is a fact. It never changes, and there is exactly one.
- **Who may read it** - the audience: the origin, widened by every release someone has decided on
  since. Releases are acts. They come one after the other, may come from different people, and could
  be taken back.

Today a release overwrites the one value (`Change::Reclassify`, `ChannelLoop::reclassify_message`,
`Classification::fitted_into` when a channel is lowered). After that:

- **The origin is lost.** After a release nobody can tell what the entry was written as, so a
  release can only be taken back by guessing what was there before.
- **Releases cannot be told apart.** Two releases to `{hr}` and to `{finance}` become one
  classification that covers both. Neither can be withdrawn alone, and nothing says who made which
  ([140](140-audit-log.md)).
- **Writing and reading are mixed up.** The flow rule and the clearances are rules for *reading*.
  Nothing states the rule for *writing*: under which classification something new has to be written,
  given what the writer has seen. In fact it is the label of the turn (`AgentActor::label`), and "a
  message is never above its channel" is a consequence of it. Nowhere is it a concept of its own.
- **Several writers, one value.** Something written to from several places - a memory document
  revised in several channels ([166](166-memory-as-topic-documents.md)), later a shared workspace
  ([19](19-memory-tiers.md), point 10) - would have to bring several origins into one
  classification. There is no rule for that, and every component would invent its own.

Proposed by the maintainer: an entry carries **exactly one classification as its authorship**, and,
as part of a **release process**, any number of further classifications under which it may also be
read. This is a concept of its own beside the present classification and clearance logic, which only
looks at reading.

## Why it matters

- [166](166-memory-as-topic-documents.md) (memory as documents) cannot decide how a document is
  classified without it.
- Releases are the act that most needs an audit trail ([140](140-audit-log.md)). As separate records
  they can be audited, as an overwritten value they cannot.
- Taking back a release becomes possible, and well defined: drop that release. The origin and the
  other releases stay.

## Considerations

- **The shape.** Something like `Label { origin: Classification, releases: Vec<Release> }` with
  `Release { classification, by, at }`. It may be read from a destination if its origin, or one of
  its releases, flows there (`flows_to`, the rule unchanged). Clearances are judged the same way,
  per classification.
- **The origin is the writer's label.** The rule for writing would be: whatever is written carries
  as its origin the label of what wrote it, and that label must be one that everything the writer
  has seen flows into. For an agent that is the label of its turn, today the channel's
  classification when the turn began. For a user it is the channel they write in. A user who
  classifies their own message lower releases it, just as they write it.
- **No classification above the origin.** "Raising" something means withdrawing releases. The origin
  itself never changes, so "never above its channel" holds by construction rather than by a check.
- **Writing to something that already exists.** A revision is a write of its own and has its own
  origin. If the writer may only revise what it can read where it is, the old content already flows
  into the writer's label, so the new version is correctly classified with that one label. A
  document then always has exactly one origin, that of its newest version. What follows from this -
  the document disappears from lower channels once it has been revised in a higher one - is correct,
  but has to be decided for [166](166-memory-as-topic-documents.md). Each channel could also see the
  newest version it may read.
- **A per-agent writing classification?** Raised by the maintainer as one way to decide what an
  agent writes as. Assessment: as the origin of what it writes, no. The agent writes in many
  channels, and what it has seen differs from turn to turn. One fixed value per agent is either
  above most channels (then nothing it remembers is recalled there), or below some (then it writes
  down what it read high up: a leak). It fits as a ceiling - what the agent may hold at most - but
  that already exists as its owner's clearance. The value that fits is the turn's label. Open is
  only whether that label stays "the channel's classification when the turn began", or becomes the
  *high-water mark* of what the turn has actually seen (the join of the classifications of its
  transcript, prompt parts and tool results). The latter is more precise and may lie below the
  channel, which would let memory written in a high channel stay readable lower down. It is more
  work, and the join of compartments needs a top element for unrelated compartments.
- **Who may release.** Today: the channel's owner for a channel and its messages, the author for
  their own message, the agent with approval for its notes and memory. With releases as records this
  becomes one question - `ActionType::Release { label, classification }` - and its answer names who,
  which is what the audit needs.
- **Lowering a channel.** Today every message is rewritten (`fitted_into`). With releases, this is
  one release *of the channel* that its messages inherit, rather than one per message, and it can be
  taken back as a whole. Whether a container's releases apply to its entries is to be decided.
- **Names.** "Classification" stays the value (level and compartments). The pair of origin and
  releases needs a name of its own; *label* is the usual word in information-flow models (cf. the
  decentralised label model, where owners declassify). `AgentActor::label` already uses the word for
  the writing side.
- **Store and REST.** Every place that stores a classification - messages, tool results, ledgers -
  changes. The REST API and the web UI show one classification today. Keeping `classification` as
  the origin and adding `releases` keeps the API compatible.

## Decisions

- **The turn's label is the channel's classification when the turn began** (decided 2026-10-01). It
  is what `AgentActor::label` is today, and it is sound because a turn sees nothing that may not
  flow into its channel. The high-water mark of what the turn actually saw is deferred to
  [168](168-turn-label-as-high-water-mark.md).
- **There is no writing classification per agent** (decided 2026-10-01). The agent's ceiling is its
  owner's clearance, as it is now.
- **The pair of origin and releases is called a *label*** (decided 2026-10-01); "classification"
  stays the value of level and compartments.

## Open questions

- **Do a channel's releases apply to its messages?** Lowering a channel today rewrites every message
  in it (`Classification::fitted_into`). With releases there are two ways:
  - The channel's release is one record on the channel, and its messages inherit it: a message may
    be read wherever its own label *or* its channel's releases allow. One act, withdrawn as a whole,
    nothing rewritten. But a message's label alone no longer says where it may be read, and every
    read has to look at the channel too - also for a message read through another channel's history
    or quoted elsewhere.
  - Lowering the channel adds a release to every message, as `fitted_into` does today. Every label
    stays self-contained, but one act becomes many records, and withdrawing it means finding them
    all again. Either way, messages written *after* the lowering have the lower channel as their
    origin and need no release.

## Done when

- Everything that carries a classification carries an origin and its releases. The origin is set
  when it is written and never changes.
- The rule for writing is stated: under which label something is written, and why that is sound.
- Releasing and withdrawing a release are acts with an actor and a time, asked of the policy as one
  question.
- `docs/CONFIDENTIALITY.md` describes origin, releases and the writing rule.

## History

- **2026-10-01** — Opened at the maintainer's request as the basis for
  [166](166-memory-as-topic-documents.md), after the question of how a memory document written from
  several channels is classified came up once again. Whether the turn's label stays the channel's or
  becomes the high-water mark of what the turn saw is to be decided.
- **2026-10-01** — Decided: the turn's label stays the channel's classification when the turn began;
  there is no writing classification per agent. The high-water mark moved to
  [168](168-turn-label-as-high-water-mark.md).
- **2026-10-01** — Decided: the pair is called a *label*. Whether a channel's releases apply to its
  messages is kept as an open question.
- **2026-10-01** — Rejected. Something classified carries exactly one classification - one level,
  possibly several compartments - and a fixed one: a write must fit into it, it does not change it.
  With that there is nothing to join from several writers, and no origin and releases to keep apart.
  What remains goes elsewhere: who may change something, what a write must respect and where a
  change was made to [169](169-changing-classified-content.md); a record of who released what, and
  when, to [140](140-audit-log.md).
