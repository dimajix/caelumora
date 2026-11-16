# 588 — A person's call is released without its result

**Status:** open
**Type:** bug
**Priority:** low
**Area:** Sessions and channels
**Summary:** A message carrying a person's call (issue 587) is reclassified and fitted into a lowered channel by its own classification alone, while its tool results keep theirs, so whoever is given the message is labelled below what it read, or loses more than the result.

## The problem

With issue [587](587-a-person-cannot-run-a-command-in-the-workspace.md) a person runs a command
in the workspace (`/workspace run`), and the call with its result is sent as the person's
message: the message classified as the channel, its `ToolCallResult` as the workspace. Every
other participant hears both, and `read_history` of another channel returns them, in words
(`calls_in_words`).

Three places take a message's classification for all of it, though its results carry their own:

1.  **Reclassifying a message** (`Message::reclassify`, `Transcript::reclassify`) changes the
    message's classification alone. A person may reclassify their own message, below the
    workspace's classification too, as long as it flows into the channel.
2.  **Lowering a channel** fits the message and its attachments into it (`Message::fit_into`),
    not its results, which stay above the channel.
3.  **What an external agent's remote agent was given** is recorded by
    `TranscriptEntry::classification`, the message's or the reclassification's classification -
    no result's (`AgentChannelState::classifications_given`).

How it arises, and what comes of it:

*   A person runs a command, then lowers their message below the workspace. An external agent in
    the channel catching up is given the output in words, and its answer is labelled below what
    its remote agent read. Needs a deliberate lowering and an external agent in the channel;
    unlikely.
*   The same lowering, or a channel lowered, and an agent of another channel reads it with
    `read_history`. The message is let through by its own classification, the result comes with
    it, and what is read is classified with the result (`Message::classification_with_results`).
    Above the reading channel, the `ToolExecutor` withholds the whole read - the other messages of
    the page as well. Nothing leaks; what may be read is lost. Unlikely as well.

An agent's own turn is not affected: `Work::see_message` labels it with the results
(`classification_with_results`), within its channel.

## Why it matters

A label is to say what was read. A person who releases the words of their call has not decided
on the output of their command, which came from the workspace; and a reader that may read the
words should not lose them, nor the rest of a page, to an output it may not read.

## Considerations

- **A. A result is classified as an attachment is.** An attachment has a classification of its
  own, which a reclassification of its message leaves alone, a lowered channel fits
  (`fit_into`), and which is read by its own label from another channel, also where its message
  is released ([CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md#attachments), issue
  [282](282-message-attachments.md)). The results of a person's call are treated the same:
  `fit_into` fits them as it fits attachments; `read_history` gives a message with each result
  it may copy and withholds the others, as the `ToolExecutor` withholds a result the model may
  not see - told only that it was withheld; `classifications_given` records the results of what
  was given. *Cost:* a rule per place, three places; the rule exists already for attachments.
- **B. A message is never classified below its results.** `Message::reclassify` - or the channel,
  as it admits the reclassification - refuses a classification below
  `classification_with_results`, or raises it to the join. *Cost:* small, one place; but the
  words of a call can then not be released without its output, a lowered channel still leaves
  the results above it (2), and raising silently gives the person something else than asked.

Recommended: A, since it follows the pattern attachments set, and covers the lowered channel as
well.

## Done when

- A person's call reclassified below its result, or in a channel lowered below it, is read from
  another channel with its words, its result withheld where it may not be copied, and the rest
  of the page read as before.
- What an external agent's remote agent was given is recorded with the results in it.
- [CONFIDENTIALITY.md](../docs/CONFIDENTIALITY.md) says how a person's call is reclassified,
  fitted and read; tests hold each of the three places.

## History

2026-11-16: Opened at the maintainer's request, from a finding of the work on issue 587: the
results of a person's call are left out where a message is reclassified, fitted into a lowered
channel, or recorded as given to a remote agent.
