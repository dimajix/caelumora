# 417 — On reload a user is shown the channel's history, not what their actor showed them

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Sessions and channels
**Summary:** Live, a user sees what the moderator routed to their actor; on reload they see the channel's history, filtered by what they may read, so the two can differ and whatever went to the user alone is gone.

## The problem

Since [220](220-user-stream-from-its-actor.md) a user's stream is what their actor shows: what the
moderator routes to them and what they say themselves, in that order. The reason is in 220: what a
user sees should be what is routed to them, one owner, the moderator, and no second path with a
filter of its own.

What the user is shown on reload, and as the last page of a snapshot, is that second path: the
channel's history (`ChannelView::history_page`), read from the `HistoryStore` under
`HistoryKey::Channel` and filtered by where the user's view begins and by what they may read. The
user's state in the channel (`UserChannelState`) holds only their read mark. So the two differ:

- **What went to the user alone is lost on reload**: a private answer of a command, a chain cut off
  (`ChainCutOff`), a message the channel refused, a retry rejected. WEB-UI.md accepts this as
  "shown live and gone on reload".
- **A moderator that withholds something from a user is undone on reload.** `Moderator::route` may
  deliver a message to some users and not to others; the history shows it to everyone who may read
  the channel. No moderator does this today, since `policy_moderator` delivers every message to
  every user, but nothing rules it out.
- **The order can differ.** A user's own message is shown as they send it, before the channel takes
  it in, so live it may stand before a message the history places earlier.

An agent has a history of its own in every channel (`AgentChannelState::history`): everything
offered to it and what it said, with what each of its own messages was produced from. A user has
none. So the same message is kept once by the channel and once more by every agent, in a different
form each (`Message::published`, `Message::as_heard`, the sender's own), and an agent's record is
built by the agent - what it took in, and what it caught up with on joining - rather than by the
channel that delivered it.

## Why it matters

The requirement is one sentence: **for every actor - agent or user - it can be read back exactly
which messages it received, in which order.** Live and on reload alike, from one record.

## Decision

Decided on 2026-10-17:

1. **A moderator decides delivery, never content** ([418](418-moderator-decides-delivery-only.md)).
   What a participant receives is what entered the channel, projected by the channel's one rule
   (`Message::published`), or a notice the channel builds from the moderator's decision. The rule
   by which "everybody" is delivered is the channel's.
2. **The channel keeps the one history, and every entry says whom it went to.** Every envelope
   that enters the channel - a message, a change, a notice, a request, a refusal - is one entry,
   with its audience: everybody, by the channel's rule, or some alone. What an actor received is
   the channel's history filtered by that actor; what it said is the channel's history filtered by
   its sender. There is no history of an agent's any more, and none of a user's is needed.
3. **The history records the conversation, not the audit.** An audit of acts - who released,
   approved, removed, deleted what and when - is one log of its own, without content, for an
   administrator ([140](140-audit-log.md)); the history is one of its sources. So approvals and
   questions are not entries: they are the state of a call, restored from the agent's turn on a
   reload, and who approved is kept at the call ([419](419-approval-recorded-with-the-call.md)).
   The envelope stream of [05](05-postgres-event-recorder.md) is, for a channel, this history.
4. **What only an owner sees stays with the owner.** The channel never holds it: the sender
   publishes as today, and the full form of an agent's own messages lives in its transcript, where
   it lives today.

Weighed and set aside: (A) the channel's history stays the only record and a moderator must
deliver everything to every user - the private notices stay lost; (B) a history per user beside
the agents' - a copy per participant, kept in step by every participant, which
[416](416-messages-stored-once-per-holder.md) would then have had to make affordable; (C) one
history with an audience per item, but the agents keep theirs - two records of one delivery; (D)
the full form of every message in the channel's history, projected at delivery and at reading - a
second holder of what only an owner sees, and two places to withhold it instead of one; (E) every
kind of event as an entry, approvals and questions included - sixteen kinds for what the UI shows
and loses as six, and the approver's round trips as plumbing in the record.

Deferred to an issue of its own: an actor's own order among what it received. Every envelope would
carry the last position its sender had taken in (`seen_up_to`), so that an actor's own entries can
be placed after what it had seen - for the chat after a reload, and to tell a model that something
was said while it answered. It is additive: nothing here depends on it.

## The design

### The flow of an envelope

```
actor ── Answering ── OriginEventSender ──► channel inbox
   (published, as today: without what only its owner sees;
    a message of which nothing is left to show as an empty shell)

channel, per envelope, in order (`ChannelLoop::enter`):
  1. admission      as today: whose it is, its classifications, what it changes
  2. mentions       as today
  3. decide         routing = moderator.route(event)      (418: audience, ask, notices)
  4. record         entry { event, audience }  → seq; stored with the channel's state
  5. deliver        the event, stamped with seq, to the audience - never the sender
  6. notices        each recorded as an entry of its own, in the system's name, with its
     and requests   audience, then delivered: ChainCutOff; ResponseRequested
  7. announce       channel::SAID on the event bus, as today
```

A rejection (`admission` refuses) does not enter the channel; what is recorded is the notice
`MessageRejected` to the sender alone, as an entry with that one recipient - so that a reload still
shows why something was not let in. What was rejected is not stored: nothing of it entered.

The channel is the one sequencer, as today: it records and routes in one step, every actor's inbox
is a queue, so the order in which an actor received things is the order of the entries addressed
to it. That is the requirement; nothing more is needed for it.

### The entry

`HistoryEntry` stays the history's own vocabulary, an enum of what happened (today's twelve
variants), and gains what the record needs:

```rust
pub struct Recorded {           // the chain's entry type: History = Chain<Recorded>
    pub entry: HistoryEntry,
    pub audience: Audience,     // Everybody | Only(Vec<ActorId>) - 418
}
```

or, if a struct around the enum reads worse than a field, `audience` on every variant that is
delivered; to be settled as it is built - the predicate below must read it without a match.

Seven kinds are new; what each holds is what its event holds:

| New entry | Audience | Today |
|---|---|---|
| `ResponseRequested { of: Vec<ActorId>, answering: Answered, by: ActorId }` - built by the channel from the moderator's `ask`, `by` the user whose message or nudge it answers | those asked | never recorded; the moderator's one decision |
| `ChainCutOff { message, max_agent_hops, not_asked }` | users and the agents involved | gone on reload |
| `CommandExecuted` gains `output` - an agent's and the session's (`/wakeup`) alike | the issuer, or every user | recorded without the output; the session's not at all |
| `CommandFailed { command, reason, issuer }` | the issuer | gone on reload |
| `RetryAccepted { target, actor }`, `RetryRejected { target, reason, actor }` | the users | gone on reload |
| `MessageRejected { rejected, message, reason, sender }` | the sender | gone on reload |
| `HistoryShared { participant, from, by }` | the newcomer | the `HistoryLimit` in the document |

`CommandExecuted` keeps its output now: the entry says whom it went to, so "a private answer is the
issuer's, the history everybody's" (issue 269) no longer needs the output left out.

Two things need a form they do not have today, since the agent's history recorded them itself:

- **What a message was produced from** (`ContextRef`, today `HistoryEntry::Said::context`,
  recorded by the agent) travels with the message: `Message::produced_from: Option<ContextRef>`,
  set by `AgentChannelState::said`, stripped at delivery as what only the owner sees. References
  only, a few digests - no content.
- **A compaction** (`HistoryEntry::Compacted { from, to }`, today the agent's history alone) is an
  event the agent sends: `ActorEvent::ContextCompacted { turn_id, from, to }`, delivered to nobody
  (`ActivityFinished` is what the users are told), recorded by the channel.

And **a message of which nothing is left to show** - a step of only `HIDDEN` calls, reasoning for
the owner alone - is sent as an empty shell: id, turn, sender, time, classification, no content, no
calls; delivered to nobody, recorded. So it has a position, and the agent's history panel shows it
where it happened; its content is read from the transcript (below).

Not recorded, as today: what streams (`PartialMessageGenerated`, `IntermediatePartialGenerated`),
a beginning whose end is recorded (`ResponseStarted`, `PlanStepFinished`, `ActivityStarted`,
`CommandStarted`), where an actor stands (`StateChanged`), `WakeupRequested`, and - by decision 3
- approvals and questions, whose state the agent's turn keeps and whose record is 419 and 140.
`NudgeRequested` is recorded as the `ResponseRequested` it becomes.

### Whose view: reading through the views

Every read of a history goes through the views, as today, and every view is a predicate over the
entry's own fields - so that a database answers it with a `WHERE` (issue 332):

- **A participant `V`** - a user, an agent, now or formerly - sees the entries delivered to them,
  sent by them, and shared with them when they joined:

  ```
  (audience = Everybody ∧ delivered_to(kind, V)) ∨ V ∈ Only ∨ sender = V
    ∨ (audience = Everybody ∧ seq in a range shared with V)
  ```

  where `delivered_to(kind, V)` is the channel's rule of 418 read off the entry - a user every
  kind, an agent a message heard (`Message::is_heard`), a change, who joined or left -, from
  where their view begins (`visible_from`, as today), and of those only what they may read by
  classification (`may_read_entry`, as today).
- **A reader without an actor** - the session's owner who takes no part - sees the entries
  delivered to everybody: what any user was shown.
- **Read from another channel** (`read_history`, `ChannelReading::Released`): the entries
  delivered to everybody and released to where the reader reads from, as today.
- **What an agent saw** (`AgentContextView`, for whom may read its context): the agent's view, as a
  participant's; `context_of` reads the `produced_from` of the agent's own entry; `in_transcript`
  stays with the `TranscriptStore`; the full form of an own message - a shell, or one published
  without its owner-only parts - is read from the transcript by id (`TranscriptStore`, as
  `ActorRuntime::transcript_message` does), as `context_of` reads two stores today.

`ChannelView::history_page`, `message`, `unread`, `participant` (the former agent known by what it
said) and `UserController::mark_read` keep their signatures; the predicate they pass to the store
changes. In Postgres ([04](04-postgres-state-store.md)) `history_entries` and `history_items` carry
`sender` and `everybody`, and the recipients of an `Only` entry are a table
`history_recipients(key, seq, actor)`; the predicate above is a join and three comparisons.

### Pages

A page is still the last `limit` items before a position, in the channel's order, of those the
reader's predicate keeps, with `earlier` as the position of its oldest item (issue 218). Nothing
changes for a client: it passes `earlier` back. Each item carries its position (`seq`) now, and the
live envelope the position of the entry it stems from (`EventContext::position`, stamped by the
channel at delivery), so that a client tells a stored item from the live event that announced it
by one number instead of by a message id - which a notice has none of.

The snapshot's last page and `GET …/history` answer the same page.

### Joining: `HistoryShared`

Whoever adds a participant decides how much of the history they see (`HistoryAccess`, issue 46).
Today that is fixed as a `HistoryLimit` in the channel's document, and a joining agent reads what
it may see from the store itself (`catch_up`), into its history and transcript. With the delivery
recorded, catching up is a delivery too: the newcomer *received* what was shared with it.

The channel records `HistoryShared { participant, from: u64, by }` as it takes the participant in,
right before `ParticipantJoined`, delivered to the newcomer alone: "you may read this history from
position `from` on". A joining agent takes it in like every other entry: from it, it reads the
entries delivered to everybody from `from` on from the store, as `catch_up` does today, into its
transcript, and nothing else fixes where its view begins. The views read the newcomer's view by
the same entry: the shared range is `[from, seq of the HistoryShared)`, and the predicate above
holds. A participant added twice has two such entries; a range is never widened backwards past an
entry the participant sent, as today (CHANNELS.md, a rejoining actor).

The channel's document keeps the index `visible_from` needs without a scan of the chain, as the
position `from` instead of a message id (`HistoryLimit { participant, from: u64 }`) - what the
entry records, kept where it is read on every page.

### The agent's history goes; the transcript stays

`AgentChannelState::history` and `history_from`, `HistoryKey::Agent`, the agent's history files
and the `JsonFileStore`'s and `MemoryStore`'s keeping of them go. `HistoryKey` has one variant
left and becomes the `ChannelId`.

What read the agent's history in memory reads the transcript or the channel instead:

- `Ending::finish` (`caelumora-agents/src/turn/ending.rs`): the turn's messages
  (`History::said_in_turn`) - from the transcript's own messages with the turn's id, across its
  branches (`all_entries`, in the order added): a compaction in the middle of a turn moves on to a
  new branch, and the turn's earlier messages lie on the old one. The "last message id" the memory
  is applied with becomes the position the agent last took in.
- `AgentActor` on a retry (`agent.rs:328`): whether a message is one of its own - the transcript.
- `take_in`: a change of another's message is applied to the transcript alone; the channel's
  history already holds it.
- `catch_up`: from the `HistoryShared` entry, as above; where to go on from is the position it
  last took in, no longer found by searching its own history.
- `ExternalAgentActor` (`external_agent.rs:594`): records what it said in the transcript alone.
- `ChannelLoop::given_up_last` reads the channel's chain and asks whether a turn's end is "the last
  thing": with notices in the chain, the last thing *said or done in the conversation* - a
  `ResponseRequested` or `RetryRejected` after it does not count. One rule,
  `HistoryEntry::is_conversation`.

The transcript is what the model was given: what passed the perceive processors, what was given to
read, its own messages in full, compacted as it was. It is not a record of what happened in the
channel and is not touched here, except that it is now the one place an agent's own messages are
held in full - which it is today, too. The copies it holds of what others said are what remains of
416, and are not worth a store of their own.

## Where it carries, where it may not

Carries:

- One record for live and reload, for users and agents alike; what went to one actor alone is
  there after a reload, for that actor.
- The copies of every message in every agent's history go, and with them the ways an agent's record
  could differ from the channel's (`catch_up`, a lost state), the agent's window of its history
  (issue 387) and `Envelope::announced`.
- The `Recorder` (`record_send`, `record_receive`, only ever a `BlackholeRecorder`, issue 05) is
  what this record is; it goes.
- What only an owner sees is held and withheld where it is today.

Where it may not, and what is done about it:

1. **The agent's own turn is read from the transcript across branches.** Low risk: `all_entries`
   iterates in the order added, over every branch; a test with a compaction in the middle of a
   turn.
2. **The chain grows with notices and requests.** A `ResponseRequested` per answer, a notice per
   refusal; small entries. Pages are read by predicate, so a user's page costs what it costs today
   plus what is skipped; the chain's size is not a concern (issue 379 made it so).
3. **`Everybody` is the channel's judgement at record time**, by the rule of 418; the reader's
   predicate reads the same rule off the entry's kind. A moderator that delivers to a subset
   records `Only`, and a reader without an actor does not see it - which is right.
4. **`given_up_last` must skip the notices** (`is_conversation`), or "answer again" fails after a
   `RetryRejected`.
5. **The REST items and the web UI** are the real work: six new item kinds rendered from the
   history, and the chat's snapshot path and live reducer agreeing on them. That is the feature,
   not overhead; the two render paths stay, as today.
6. **`HistoryPage` items carry no position today**; they will, which the client needs anyway.
7. **The agent's history panel reads two stores** for an own message's full form - as `context_of`
   does today; one read per page (`TranscriptStore::messages(ids)`), as `in_transcript` is.

## What changes

`caelumora-model`

- `history`: the entry with its `Audience`; `fold`, `StandingItems`, `page_of` over it; the seven
  new variants in `is_item`, `said`, `fold_into`, `changed`, `context`; `is_conversation`;
  `said_in_turn` and `turn_began` move to the transcript (`Transcript::said_in_turn`).
- `actor`: `ActorEvent::ContextCompacted`, `HistoryShared`; `EventContext::position`;
  `Envelope::announced` grows into `Recorded::of(&Envelope, Audience)`, the one place an entry is
  built from an envelope, or goes if the channel builds entries by kind.
- `message`: `Message::produced_from`, stripped by `published`; `Message::shell` for one of which
  nothing is left to show.
- `channel`: `HistoryLimit { participant, from: u64 }`; `history_start_of` answers a position.
- `agent`: `AgentChannelState` without `history`, `history_from`; `said`, `perceive`,
  `note_announced`, `given` write the transcript alone; `keep_window` without the history.
- `store`: `HistoryKey` → `ChannelId`; `StateStore::put_agent_channel_state` without a history;
  `HistoryStore` as it is, over the new entry; `MemoryStore`, `JsonFileStore` (`histories.rs`)
  without the agents' histories; `TranscriptStore::messages(ids)`; the store suite: a page by a
  participant's predicate, `Everybody`, `Only`, the items equal the chain folded.

`caelumora-runtime`

- `moderator`: 418 first.
- `channel/run.rs`: `enter` as the flow says - decide, record, deliver, notices and requests as
  entries; `reject` records `MessageRejected`; `append_history` becomes the one place that builds
  an entry from an envelope and its routing; `route_event_impl` stamps `position`; `delivers_to`
  (418).
- `channel/participants.rs`, `mod.rs`: `take_in` records `HistoryShared`; `given_up_last` by
  `is_conversation`; `catch_up` is no longer called by the channel - the actor acts on the entry.
- `channel/history.rs`: `visible_from` from the position in the document.
- `actor/mod.rs`: `ActorSender::send` sends a shell where nothing is left to show instead of
  nothing.
- `view/channel.rs`, `view/context.rs`, `view/participant.rs`: the predicates above;
  `AgentContextView` without `HistoryKey::Agent`, own messages' full form from the transcript.
- `controller/user.rs`: `mark_read` with the position from the document.
- `recorder.rs` and `new_recorder`: removed.
- `runtime/actor.rs`: `channel_entries` stays (what a joining agent reads); `channel_position_of`
  goes.
- `tests/security_suite.rs`: S4 (pages and messages as far as the view goes) over the new
  predicate; new: a participant does not read what went to another alone; a reader without an
  actor reads what went to everybody only; a shell's content reaches whom may read the agent's
  context and nobody else.

`caelumora-agents`

- `actor/intake.rs`, `body.rs`, `agent.rs`, `external_agent.rs`, `turn/ending.rs`: as the section
  on the agent's history says; the position last taken in kept by the body; `catch_up` from
  `HistoryShared`.
- `components/channel_history.rs`: entries delivered to everybody.
- `tests/security_suite.rs`: the agent's view through `AgentContextView` as above; `read_history`.

`caelumora-rest-api`, `caelumora-rest-server`, `caelumora-web-ui`

- The seven item kinds in `HistoryPageResponse`, with `seq`; `get_channel_history`,
  `agent_history`, `channel_snapshot` as they are over the views; the chat and the history panel
  render the six notices from the history and no longer drop anything on reload.

`docs`: CHANNELS.md (what is recorded, whom to, joining), STORAGE.md (one history, the entry, the
predicate, Postgres), AGENT-TURN.md (no history of the agent's; the transcript's part),
REST-SERVER.md (items with `seq`, `position` on the stream), WEB-UI.md (nothing gone on reload),
CONFIDENTIALITY.md (where the owner-only parts are held: the transcript), SECURITY.md and
PERMISSION-CHECKS.md (the view predicate).

Stored data to delete: everything under `runtime/` - the channel states with their histories, the
agents' channel states with their histories and transcripts (a message's form changes).

## Plan

Each step is reviewable on its own; 1-3 change no behaviour.

1. **418**: the moderator returns a `Routing` with an `Audience`; the channel owns the rule of
   "everybody" and builds every envelope.
2. **The entry with its audience.** The chain's entry with `Audience`, `position` stamped at
   delivery, shells for a message of which nothing is left to show, `produced_from`,
   `ContextCompacted`; `enter` in the new order. The agents' histories still kept, in the new form.
   Store suite and S4 pass as before; a test that every entry of a channel with two users and an
   agent carries the audience the moderator chose.
3. **The views read by predicate.** A participant's page by the predicate; a reader without an
   actor by `Everybody`; `HistoryShared` recorded at joining and read for the shared range;
   `HistoryLimit` as a position; items with `seq`. No new notices yet, so every page reads as
   before. Security tests: one participant does not read another's.
4. **Everything shown is recorded.** The six notices and `ResponseRequested` as entries;
   `MessageRejected` from `reject`; `CommandExecuted` with its output; the REST items and the web
   UI render them from the history. Feature: nothing is gone on reload.
5. **The agents' histories go.** `AgentChannelState` without a history; readers moved to the
   transcript; `catch_up` from `HistoryShared`; `AgentContextView` over the channel's history and
   the transcript; `HistoryKey` to `ChannelId`; the stores without the agents' histories; the
   `Recorder` removed.
6. **Documents**, and 05, 140 and 404's read mark as they stand after this.

## Done when

- Every envelope that enters the channel, every notice and request the channel sends and every
  refusal is one entry of the channel's history, with its audience and its position, which the
  delivered envelope carries.
- A participant's page of the history, the last page of their snapshot and the agent's history
  panel are read from the channel's history by the participant's predicate; a reader without an
  actor reads what went to everybody; the security suite checks all three and that a shell's
  content reaches only whom may read the sender's context.
- What went to a user alone is shown after a reload, and only to them; a message a moderator did
  not deliver to a user is not on their page.
- An agent's actor keeps no history; what it said in a turn and what it caught up with are read
  from the transcript and the channel.
- The store suite checks the predicate, the audience and the items against the chain folded.
- The documents named above describe the one history and the view predicate.

## History

- 2026-10-17: Opened after looking at where a reload reads from. Depends on
  [416](416-messages-stored-once-per-holder.md). B is decided; how a user who joins with a view of
  the past is shown it, and whether what only a user's history holds becomes new `HistoryEntry`
  variants, are to be decided before it is built.
- 2026-10-17: Redesigned the same day. B - a history per user - is set aside: the maintainer's
  requirement is one record per actor of what it received in which order, and that record is the
  channel's history with the recipients of every entry, once a moderator can only decide delivery
  ([418](418-moderator-decides-delivery-only.md)). The agents' histories go with it, and 416 is no
  longer needed.
- 2026-10-17: Cut down the same day, after weighing what the design costs. The history records the
  conversation, not the audit ([140](140-audit-log.md), decision 3): approvals and questions are
  not entries, who approved goes to the call ([419](419-approval-recorded-with-the-call.md)). What
  only an owner sees stays with the owner (decision 4): the channel records a shell where nothing
  is left to show, and the transcript holds the full form, as today. `Audience::Everybody | Only`
  instead of a list of recipients. The entry keeps the history's own vocabulary, seven variants
  more. The watermark of an actor's own order is deferred to an issue of its own.
