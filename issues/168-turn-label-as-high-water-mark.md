# 168 — A turn writes as its channel, even when it has seen less

**Status:** open
**Area:** Identity and access management

## The problem

What an agent writes in a turn - its messages, notes, memory - carries as
its origin the label of the turn ([167](167-origin-and-releases.md)). That
label is the channel's classification when the turn began
(`AgentActor::label`). This is sound, because a turn sees nothing that may
not flow into its channel. But it is often higher than necessary. A turn in
a `confidential` channel that has seen only `internal` messages and a
`public` tool result writes `confidential` all the same. What it remembers
is then recalled only in channels that may read `confidential`, although
none of it is.

## Why it matters

- Memory and notes written in high channels are lost to every lower
  channel, even where their content would be allowed there
  ([166](166-memory-as-topic-documents.md)).
- The only remedy today is a release, which needs an approval each time
  for something that never needed one.

## Considerations

- **High-water mark.** The label of a turn would be the join of the
  classifications of everything it has seen: the transcript, the prompt
  parts, the tool results, the knowledge it read. It starts at the lowest
  label and rises with every read, and it never lies above the channel.
- **The join needs a top element.** The join of two unrelated compartments
  (`{hr}` and `{finance}`, with no compartment reading both) is one no
  destination can read. `Classification` has no such value yet.
- **What the transcript contributes.** Every message the turn is given
  counts, including those of earlier turns. A long conversation will soon
  reach the channel's classification. The gain is largest for short
  conversations and for knowledge written early in them.
- **Prompt parts and the system prompt.** A personality's system prompt or
  a skill has a classification of its own, or the gain is lost at once.
- **Recording.** The label a turn wrote under has to be traceable from what
  it saw (`ContextRef`), so that the context panel can say why something was
  written as it was.

## Done when

- A turn's label is the high-water mark of what it has seen, never above its
  channel, and what it writes carries that as its origin.

## History

- **2026-10-01** — Opened when [167](167-origin-and-releases.md) decided that,
  for now, the turn's label stays the channel's classification when the turn
  began; the high-water mark was deferred to this issue.
