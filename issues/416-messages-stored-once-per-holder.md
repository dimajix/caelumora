# 416 — A message is stored whole in every chain that holds it

**Status:** rejected
**Type:** refactoring
**Priority:** medium
**Area:** Persistence
**Summary:** Every history and transcript stores each message it holds in full, so one message said in a channel lies in the store once for the channel and twice for every agent in it, and every change of it as often again.

## The problem

A message said in a channel is stored as a whole `Message` in every chain that holds it (issues
379, 380, see [STORAGE.md](../docs/STORAGE.md#histories)):

- in the chain and in the items of the channel's history,
- in the chain and in the items of the history of every agent in the channel,
- in the transcript of every agent that took it in.

With three agents that is about ten copies of one message. A `Modified` entry carries the whole
message again, in every one of these chains. The store grows with the number of participants times
the number of messages, and a history of the user's own
([417](417-user-history-from-its-actor.md)) would add two more copies for every user.

The copies are not all alike, so a message cannot simply be stored once by its `MessageId`. One id
is stored in different forms:

| Where | Form |
|---|---|
| the sender's history and transcript | the message as said, with what only its owner sees |
| the channel's history | `Message::published` |
| the history and transcript of every other agent | `Message::as_heard`: role `user`, without tool calls and results |
| a turn given up below its channel | `Message::names_only` |

What is the same is the content of one form: the `published` form is the same in the channel and,
with 417, for every user; the `as_heard` form is the same for every other agent, in its history and
in its transcript.

## Why it matters

- The store grows with every participant, not only with what is said; with 417 also with every
  user.
- Writing a message, or a change of it, costs one write of the whole message per holder.
- A history of the user's own (417) is only affordable if what it holds costs next to nothing.

## Considerations

### Each form once by its digest, the chains name it

The prompt texts and tool sets are kept once by digest already (`Kept`, issue 383). Messages are
kept the same way:

- **A message store** keeps every form of a message once, under its digest, idempotently: a digest
  there already is not written again (`ON CONFLICT DO NOTHING` in Postgres, a file per digest in the
  `JsonFileStore`).
- **The chains hold a reference.** `HistoryEntry::Said` and `Modified`, `TranscriptEntry::Message`,
  `Given` and `Modified` hold the message's id and the digest of its form instead of the message;
  so do the items of a history (`StandingItems`). Folding a change into an item
  (`HistoryEntry::fold_into`) yields a new form, kept by its digest.
- **The digest of an entry** covers the digest of the message instead of its content, as a Merkle
  tree does; it says as much as before.
- **Written first.** The put of a state writes the messages added since it was last stored before
  the chains, as the prompt texts are, so that nothing stored names a message that is not there
  after a crash.
- **Read by reference.** Whatever reads a page, a message or a transcript resolves the references
  of what it read in one request to the store.

So a message lies in the store one to three times, however many take part: the `published` form
for the channel (and with 417 every user), the `as_heard` form for every other agent, the sender's
form only where it holds something only its owner sees - otherwise it is the `published` form.

### Kept as stored, or derived on reading

Instead of every form, only the message as said could be kept, the other forms derived as they are
read (`Said { message, form: Heard }`). That saves one more copy, but then every way of reading a
history holds the message with what only its owner sees, and one that forgets to derive the form
discloses it.

**Recommended:** every form is kept as it is stored. What a history holds is exactly what may stand
there, as today.

### Scope of a kept message

A kept message must go with what it belongs to. Kept globally, deleting a channel would need a count
of who still names each message.

**Recommended:** per channel, under `(channel_id, digest)`: the channel's history, the histories of
the agents and users in it and the transcripts of the agents in it share one store, which is
deleted with the channel as its history directory is today. A message an agent reads from another
channel (the tool that reads another channel) is kept again in the agent's channel; nothing depends
on a channel that is not its own.

### What it costs

- The stored form of histories, items and transcripts changes: everything under `runtime/` has to be
  deleted.
- The digests of transcripts change, and with them every `ContextRef`.
- Postgres ([04](04-postgres-state-store.md)) gets a table `messages(channel, digest, body)`;
  `history_items` and `transcript_entries` hold references.
- The `MemoryStore`, the `JsonFileStore` and the store suite change alike.

## Done when

- Every form of a message is kept once per channel by its digest, and every history, its items and
  every transcript name it by reference.
- The store suite checks that a message said in a channel with several agents is kept once per
  form, and that a change of it keeps one new form per holder's form.
- Deleting a channel deletes the messages kept for it.
- Nothing a client or an agent sees changes.
- STORAGE.md describes the message store beside the prompt texts.

## History

- 2026-10-17: Opened, as the step before [417](417-user-history-from-its-actor.md): a history of the
  user's own is only affordable once messages are kept once. The two recommendations above - forms
  kept as stored, kept per channel - are to be confirmed before it is built.
- 2026-10-17: Rejected the same day, with the redesign of 417: the copies this issue is about are
  the agents' histories, which 417 removes - the channel keeps the one history, with the
  recipients of every entry, and no user gets one. What remains is the copy of what others said in
  every agent's transcript, as it was given to the model, which is the transcript's own and not
  worth a store of its own. If that ever changes, the design above - every form once by digest,
  the chains naming it - still applies to transcripts.
