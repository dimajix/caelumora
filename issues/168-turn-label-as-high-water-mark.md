# 168 — A turn writes as its channel, even when it has seen less

**Status:** done
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
- **2026-10-01** — [167](167-origin-and-releases.md) was rejected. The
  turn's label is what [169](169-changing-classified-content.md) checks a
  write against: it must flow into the classification of what is written.
- **2026-10-01** — Done. Decided with the maintainer:
  - **Two values.** The turn keeps the channel's classification when it
    began as the bound of what it may read (`Turn::classification`), and
    its label (`TurnState::seen`, `Turn::label`) as what it writes:
    messages, the tool message, notes, private notes, memory, the answers
    of built-in tools, the message telling of a given-up turn's calls. The
    label starts at `public` and rises with every message of the
    transcript and tool result in them, every prompt part, every
    knowledge read, every tool result once the model is given it.
  - **No top element.** Everything a turn sees flows into its channel, so
    the channel bounds the join. The join itself
    (`Classification::join`) is the higher level and the audience in
    common, computed without the read relation of the compartments, which
    is the policy's ([138](138-confidentiality-policy-in-ownership.md)).
    Where the join is not known to flow into the channel without it - no
    compartment in common, or only by which compartments read which
    (`Classification::surely_flows_to`) - the label is the channel's
    classification. Sound under every read relation; with one configured,
    sometimes higher than necessary.
  - **Prompt parts** are classified by their provider
    (`Prompt::classification`); a part that does not say counts as the
    channel. Option c, as decided: since a personality's system prompt is
    the `static_system_prompt_provider` component, the optional
    `classification` is that component's parameter, `public` if left out.
    Parts that hold only knowledge - notes, private notes, memory - are
    classified by it.
  - **Recorded.** `PromptRef` and `KnowledgeRef` record the classification
    of what was read, so that the label can be explained later, whatever
    is reclassified since. Showing it in the context panel is
    [170](170-context-panel-shows-the-label.md).
  - **Not done here:** a turn whose label still flows into a channel
    lowered while it runs gives up all the same
    ([141](141-turn-during-reclassification.md)); letting it go on would need
    its bound for reading to fall too.
  - **What it gains in practice.** A user's message is classified as the
    channel unless the user classifies it lower, and a tool that does not
    classify its results gives the channel's classification. A turn that
    answers such a message writes as the channel, as before. The label
    falls below the channel only where everything the turn saw was lower:
    released messages, public tool results, knowledge from lower channels.
