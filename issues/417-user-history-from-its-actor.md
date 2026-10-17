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
which messages it received, in which order, and what it said in between.** Live and on reload
alike, from one record, for an audit as for the chat.

## Decision

Three principles, decided on 2026-10-17:

1. **A moderator decides delivery, never content** ([418](418-moderator-decides-delivery-only.md)).
   What a participant receives is what entered the channel, projected by the channel's one rule
   (`Message::published`), or a notice the channel builds from the moderator's decision.
2. **The channel keeps the one history, and every entry says whom it went to.** Every envelope
   that enters the channel - a message, a change, a notice, a request, an approval, an answer to a
   question, a refusal - is one entry, with its recipients. What an actor received is the channel's
   history filtered by that actor; what it said is the channel's history filtered by its sender.
   There is no history of an agent's any more, and none of a user's is needed.
3. **An actor's own order is a watermark, not a second chain.** Every envelope an actor sends
   carries the last position of the channel's history it had taken in (`seen_up_to`). The
   channel's order is the shared truth - the sequencer's -; an actor's own messages are placed
   after what it had seen when it said them, wherever that view is wanted. The transcript keeps
   what the model was given, in the order it was given, as today.

Weighed and set aside: (A) the channel's history stays the only record and a moderator must
deliver everything to every user - the private notices stay lost; (B) a history per user beside
the agents' - a copy per participant, kept in step by every participant, which
[416](416-messages-stored-once-per-holder.md) would then have had to make affordable; (C) one
history with an audience per item, but the agents keep theirs - two records of one delivery.

## The design

### The flow of an envelope

```
actor ── Answering ── OriginEventSender ──► channel inbox
   (full message: what only its owner sees included, with what it was produced from)

channel, per envelope, in order (`ChannelLoop::enter`):
  1. admission          as today: whose it is, its classifications, what it changes
  2. resolve mentions   as today
  3. project            shown = event.published()   - what everyone but the sender may see
  4. decide             routing = moderator.route(shown)   (418: recipients, asked, notices)
  5. record             entry { event (full), sender, at, recipients, public, seen_up_to }
                        → seq; stored with the channel's state before anything is delivered
  6. deliver            shown, stamped with seq, to every recipient (never the sender)
  7. notices, requests  each recorded as an entry of its own, in the system's name, with
                        its recipients, then delivered: ChainCutOff; ResponseRequested
  8. announce           channel::SAID on the event bus, with the published form (issue 404)
```

A rejection (`admission` refuses) does not enter the channel; what is recorded is the notice
`MessageRejected` to the sender alone, as an entry with that one recipient - so that a reload still
shows why something was not let in. What was rejected is not stored: nothing of it entered.

The channel is the one sequencer, as today: it records and routes in one step, every actor's inbox
is a queue, so the order in which an actor received things is the order of the entries addressed
to it. Nothing new is needed for the *received* order; the watermark is needed for the actor's
*own* entries among them.

### The entry

The chain stays `Chain<HistoryEntry>`; the entry becomes what entered the channel, with its
delivery:

```rust
pub struct HistoryEntry {
    /// What entered, in full - as recorded (`ActorEvent::recorded`): what
    /// the chain holds already is left out, e.g. the messages a
    /// `TurnFinished` carries, which were said before it.
    pub event: ActorEvent,
    pub sender: ActorId,
    pub sender_name: String,
    /// On whose word (`SecurityContext::initiator_id`): the issuer of a
    /// command an agent carried out, the user an agent's turn answers.
    pub initiator: ActorId,
    pub at: DateTime<Utc>,
    /// Whom it was delivered to; never the sender.
    pub recipients: Vec<ActorId>,
    /// Whether every user taking part at the time was among them: what
    /// anyone reading the channel without an actor in it may read.
    pub public: bool,
    /// The last position of this history the sender had taken in when it
    /// sent this; `None` for the channel's own word and the moderator's.
    pub seen_up_to: Option<u64>,
}
```

**Decision point 1 - the record's vocabulary.** Two ways:

- (A) keep `HistoryEntry` as an enum of its own, extended by a variant per kind of entry that is
  new - `ResponseRequested`, `ChainCutOff`, `CommandRequested`, `CommandAnswered`, `CommandFailed`,
  `RetryRequested`, `RetryAnswered`, `MessageRejected`, `ApprovalRequested`, `ApprovalDecided`,
  `ApprovalWithdrawn`, `QuestionRequested`, `QuestionAnswered`, `QuestionDeclined`,
  `QuestionClosed`, `HistoryShared` - and a struct around it for the delivery. Every entry is
  worded by the record; the five exhaustive matches over the enum (`is_item`, `said`, `fold_into`,
  `changed`, `context`) grow by sixteen arms each, and `Envelope::announced` grows into the mapping
  of every event to its entry.
- (B) the entry holds the `ActorEvent` as it entered (above). One rule says what is recorded and
  what the chain holds already (`ActorEvent::recorded`); the folding keys on the event's variants
  (`MessageModified`, `MessageRevoked`, `MessageReclassified`, and a channel announcement for a
  reclassified channel, see below); what the REST API hands out as an item is the event with its
  delivery, so that a client renders a stored item and a live event through one path - which is
  the point of the feature. The record's form follows the wire form, so a change of an
  `ActorEvent` changes what new entries look like; stored data need not stay readable, so nothing
  is lost by that.

**Recommended: (B).** It is the smaller type and removes a second enum that mirrors the first
(AGENTS.md: no repeated logic); the cost is that `HistoryEntry` is no longer a vocabulary of its
own. What the chain holds already is left out by `recorded`: a `TurnFinished` is recorded without
its messages, a `CommandExecuted` with its output (below).

Three things need a form they do not have today:

- **What a message was produced from** (`ContextRef`, today `HistoryEntry::Said::context`,
  recorded by the agent) travels with the message: `Message::produced_from: Option<ContextRef>`,
  set by `AgentChannelState::said`, stripped by `Message::published` as what only the owner sees.
  References only, a few digests.
- **A compaction** (`HistoryEntry::Compacted { from, to }`, today the agent's history alone) is an
  event the agent sends: `ActorEvent::ContextCompacted { turn_id, from, to }`, delivered to nobody
  (`ActivityFinished` is what the users are told), recorded by the channel.
- **The channel reclassified** (`HistoryEntry::Fitted`, today recorded by `ChannelLoop::reclassify`
  straight into the chain) becomes the channel's own announcement, `ActorEvent::ChannelReclassified
  { classification }`, told to everybody and recorded - as who joined or left is (issue 408).

### What is recorded, what is not

Everything an actor is shown that is complete, and everything that asks an actor for something.
Not what streams, and not where an actor stands.

| Recorded | with whom as recipients |
|---|---|
| `CompleteMessageGenerated`, `IntermediateMessageGenerated` | whom the moderator delivers to |
| `MessageModified`, `MessageRevoked`, `MessageReclassified`, `ChannelReclassified` | everybody but the sender; a change folds into its message whoever it went to |
| `TurnFinished` (without its messages), `TurnGivenUp`, `PlanStepStarted`, `ActivityFinished`, `ContextCompacted` | the users; a compaction nobody |
| `ParticipantJoined`, `ParticipantLeft`, `HistoryShared` | everybody; the newcomer |
| `ResponseRequested { answering }` - built by the channel from the moderator's `ask`, also for a nudge | whom it asks; `initiator` the user whose message or nudge it answers |
| `ChainCutOff` | the users and the agents involved |
| `CommandRequested` | the agent named |
| `CommandExecuted` - with its `output` now, an agent's and the session's (`/wakeup`) alike | the issuer alone, or every user |
| `CommandFailed` | the issuer |
| `RetryRequested`, `RetryAccepted`, `RetryRejected` | the agent; the users |
| `MessageRejected` | the sender of what was refused |
| `ApprovalRequested`, `ApprovalGranted`, `ApprovalDenied`, `ApprovalTimedOut`, `ApprovalRequestWithdrawn` | the approver; the users who may approve; the agent that asked - each envelope an entry: the agent's request to the approver, the approver's to the users |
| `QuestionRequested`, `QuestionAnswered`, `QuestionDeclined`, `QuestionClosed` | the users asked; the agent asking |

| Not recorded | why |
|---|---|
| `PartialMessageGenerated`, `IntermediatePartialGenerated` | streaming; the complete message follows |
| `ResponseStarted`, `PlanStepFinished`, `ActivityStarted`, `CommandStarted` | a beginning whose end is recorded; a turn is known by its messages and its end |
| `StateChanged` | where an actor stands, not what happened; sent at every step |
| `WakeupRequested` | said to nobody; what it wakes is recorded as what it says |
| `NudgeRequested` | recorded as the `ResponseRequested` it becomes, with the user as `initiator` |

Approvals and questions are recorded because the maintainer wants the audit: who asked for what,
who decided, when. An approval request carries the call's arguments, which the approvers see
whatever the call's visibility (`ActorEvent::published` leaves it as it is); the recipients say who
may read it back, which is who was shown it.

`CommandExecuted` keeps its output now: the entry says whom it went to, so "a private answer is the
issuer's, the history everybody's" (issue 269) no longer needs the output left out.

### Whose view: reading through the views

Every read of a history goes through the views, as today, and every view is a predicate over the
entry's own fields - so that a database answers it with a `WHERE` (issue 332):

- **A participant `V`** - a user, an agent, now or formerly - sees the entries delivered to them
  or sent by them, and what was shared with them when they joined:
  `V ∈ recipients ∨ sender = V ∨ (public ∧ seq within a range shared with V)`,
  from where their view begins (`visible_from`, as today), and of those only what they may read by
  classification (`may_read_entry`, as today).
- **A reader without an actor** - the session's owner who takes no part - sees the public entries:
  what any user would have been shown.
- **Read from another channel** (`read_history`, `ChannelReading::Released`): the public entries
  released to where the reader reads from, as today.
- **What an agent saw** (`AgentContextView`, for whom may read its context): the agent's view, as a
  participant's; `context_of` reads the `produced_from` of the agent's own entry;
  `classification_of` likewise; `in_transcript` stays with the `TranscriptStore`.

What a reader is given of a message is projected as it is read: the entry holds the sender's full
message; a reader who is not the sender and may not read the sender's context is given
`Message::published`. So the owner-only parts are kept by one holder and withheld at two places
by one rule - at delivery and at reading - and the security suite checks both.

`ChannelView::history_page`, `message`, `unread`, `participant` (the former agent known by what it
said) and `UserController::mark_read` keep their signatures; the predicate they pass to the store
changes. In Postgres ([04](04-postgres-state-store.md)) the recipients are a table
`history_recipients(key, seq, actor)` beside `history_entries` and `history_items`, which carry
`sender` and `public`; the predicate above is a join and two comparisons.

### Pages

A page is still the last `limit` items before a position, in the channel's order, of those the
reader's predicate keeps, with `earlier` as the position of its oldest item (issue 218). Nothing
changes for a client: it passes `earlier` back. The items of a page are the entries as they stand,
each with its position (`seq`) and its `seen_up_to`, so that a client can place the reader's own
entries where they were said from their point of view (below), and can tell a stored item from the
live event that announced it by the same position - the live envelope carries it (`EventContext::
position`, stamped by the channel at delivery).

The snapshot's last page and `GET …/history` answer the same page.

### Order: the watermark

Every actor keeps the last position it took in - an agent as it takes an envelope in (`take_in`),
a user's actor as it shows one - and sends it with every envelope of its own (`EventContext::
seen_up_to`). The channel records it with the entry.

The channel's order is what every page is read in; it is the sequencer's order and the same for
everybody. An actor's own view - its own entries after what it had seen - is derived where it is
wanted, from the entries as read: an own entry with `seen_up_to = n` stands after the entry at `n`
and before what came after `n`; own entries with the same watermark keep their order. The web UI
does this for the user's own messages in the chat and the agent's in its history panel; a page's
edge can leave an own entry whose watermark lies on a page not yet loaded at the top of the loaded
pages until that page comes.

**Decision point 2.** The alternative is to page in the reader's own order - the store sorting by
`(anchor, seq)` with `anchor = seen_up_to` for own entries and `seq` for received ones. A database
does that with an expression; the `JsonFileStore` would read backwards holding back own entries
until it passes their anchor. It is exact at page edges, at the cost of a cursor that is a pair and
a store that sorts per reader. **Recommended: the channel's order, with the placement derived by the
reader** - the same for the chat as for an audit, and the agent's cause-and-effect order is kept
by its transcript anyway, which is what its model is given.

What the watermark also gives: an own entry with `seen_up_to < seq - 1` was said while others
spoke. The chat can mark it; an agent's `ContextResolver` can one day tell the model that
something was said while it answered - today the agents' turns interleave without anyone noticing
that they were concurrent. Not part of this issue.

An actor that joins carries no position until it took something in; its first envelopes carry
`None`.

### Joining: `HistoryShared`

Whoever adds a participant decides how much of the history they see (`HistoryAccess`, issue 46).
Today that is fixed as a `HistoryLimit` in the channel's document, and a joining agent reads what
it may see from the store itself (`catch_up`), into its history and transcript. With the delivery
recorded, catching up is a delivery too: the newcomer *received* what was shared with it.

The channel records `HistoryShared { participant, from: u64, by }` as it takes the participant in,
right before `ParticipantJoined`, delivered to the newcomer alone: "you may read this history from
position `from` on". A joining agent takes it in like every other entry: from it, it reads the
public entries from `from` on from the store, as `catch_up` does today, into its transcript, and
nothing else fixes where its view begins. The views read the newcomer's view by the same entry:
the shared range is `[from, seq of the HistoryShared)`, and the predicate above holds. A
participant added twice has two such entries; a range is never widened backwards past an entry the
participant sent, as today (CHANNELS.md, a rejoining actor).

The channel's document keeps the index it needs for `visible_from` without a scan of the chain,
as the position `from` instead of a message id (`HistoryLimit { participant, from: u64 }`) - what
the entry records, kept where it is read on every page.

### The agent's history goes; the transcript stays

`AgentChannelState::history` and `history_from`, `HistoryKey::Agent`, the agent's history files
and the `JsonFileStore`'s and `MemoryStore`'s keeping of them go. `HistoryKey` has one variant
left and becomes the `ChannelId`.

What read the agent's history in memory reads the transcript or the channel instead:

- `Ending::finish` (`caelumora-agents/src/turn/ending.rs`): the turn's messages
  (`History::said_in_turn`) and the last message id for the memory - from the transcript's own
  messages with the turn's id, across its branches (`all_entries`): a compaction in the middle of
  a turn moves on to a new branch, and the turn's earlier messages lie on the old one.
- `AgentActor` on a retry (`agent.rs:328`): whether a message is one of its own - the transcript.
- `take_in`: a change of another's message is applied to the transcript alone; the channel's
  history already holds it.
- `catch_up`: from the `HistoryShared` entry, as above; the position of the last message it heard
  is its watermark, no longer found by searching its own history.
- `ExternalAgentActor` (`external_agent.rs:594`): records what it said in the transcript alone.
- `ChannelLoop::given_up_last` reads the channel's chain and asks whether a turn's end is "the last
  thing": with notices in the chain, the last thing *said or done in the conversation* - a
  `ResponseRequested` or `RetryRejected` after it does not count. One rule, `HistoryEntry::
  is_conversation`.

The transcript is what the model was given: what passed the perceive processors, what was given to
read, its own messages in full, compacted as it was. It is not a record of what happened in the
channel and is not touched here. The copies it holds of what others said are what remains of 416,
and are not worth a store of their own.

### What only the owner sees

`ActorSender::send` no longer applies `ActorEvent::published`: the channel does, at step 3, for
the moderator and for every delivery. The `OriginEventSender` still passes on only what speaks as
its actor. The channel holds the full message in its history and projects it as it is read
(above). What may not flow into the channel at all is withheld before the model as today
(CONFIDENTIALITY.md) and is never in the channel.

## Where it carries, where it may not

Carries:

- One record for live and reload, for users and agents alike; what went to one actor alone is
  there after a reload, for that actor.
- An audit answers who received what, who was asked to respond, who approved what, from the chain.
- The copies of every message in every agent's history go, and with them the ways an agent's record
  could differ from the channel's (`catch_up`, a lost state).
- The `Recorder` (`record_send`, `record_receive`, only ever a `BlackholeRecorder`) is what this
  record is; it goes.

Where it may not, and what is done about it:

1. **The agent's own turn is read from the transcript across branches.** `said_in_turn` today reads
   the history from where the turn began; the transcript equivalent must read every branch and
   keep the turn's order. Moderate risk; a test with a compaction in the middle of a turn.
2. **The chain grows with notices and requests.** Every `ResponseRequested`, every approval round
   trip is an entry. Pages are read by predicate, so a user's page costs what it costs today plus
   what is skipped; the chain's size is not a concern (issue 379 made it so).
3. **The moderator must never see the full form.** Guaranteed by the channel giving it the
   published projection (418); a security test asserts it with a message of owner-only parts and
   a policy that records what it was given.
4. **`public` is the channel's judgement at record time**: every user taking part was among the
   recipients. A moderator that delivers to a subset makes the entry non-public, and a reader
   without an actor does not see it - which is right. If `recipients` were to be compared with who
   took part later, the join/leave entries would have to be replayed; the flag avoids that.
5. **A page's edge and the watermark** (decision point 2).
6. **The history panel of an agent shows what the agent received**: with `HistoryShared`, also what
   it caught up with; what it *perceived* (passed its processors) stays `in_transcript`.
7. **The `channel::SAID` announcement and the `AgentContextView`'s classification** read the entry's
   message; both must take the published form where the reader is not the owner.
8. **The REST items change form** (decision point 1, B): `caelumora-rest-api` and the web UI change
   together; REST-SERVER.md says that an item is the event with its delivery.

## What changes

`caelumora-model`

- `history`: `HistoryEntry` as above; `fold`, `StandingItems` and `page_of` over it; `is_item`,
  `changed`, `fold_into`, `context`, `said` keyed on the event; `is_conversation`; `said_in_turn`
  and `turn_began` move to the transcript (`Transcript::said_in_turn`); `History::push`/`extend`
  take the delivery.
- `actor`: `ActorEvent::recorded`, `ContextCompacted`, `ChannelReclassified`, `HistoryShared`;
  `EventContext::position`, `seen_up_to`; `Envelope::announced` goes (the entry is built in one
  place, `HistoryEntry::of(&Envelope, recipients, public)`).
- `message`: `Message::produced_from`, stripped by `published`.
- `channel`: `HistoryLimit { participant, from: u64 }`; `history_start_of` answers a position.
- `agent`: `AgentChannelState` without `history`, `history_from`; `said`, `perceive`,
  `note_announced`, `given` write the transcript alone; `keep_window` without the history.
- `store`: `HistoryKey` → `ChannelId`; `StateStore::put_agent_channel_state` without a history;
  `HistoryStore` as it is, over the new entry; `MemoryStore`, `JsonFileStore` (`histories.rs`)
  without the agents' histories; the store suite: a page by a participant's predicate, `public`,
  `recipients`, the items equal the chain folded.

`caelumora-runtime`

- `moderator`: 418 first.
- `channel/run.rs`: `enter` as the flow says - project, decide, record, deliver, notices and
  requests as entries; `reject` records `MessageRejected`; `append_history` becomes the one place
  that builds an entry from an envelope and its routing; `route_event_impl` stamps `position`.
- `channel/settings.rs`: `reclassify` announces `ChannelReclassified` instead of `fit_into`.
- `channel/participants.rs`, `mod.rs`: `take_in` records `HistoryShared`; `given_up_last` by
  `is_conversation`; `catch_up` is no longer called by the channel - the actor acts on the entry.
- `channel/history.rs`: `visible_from` from the position in the document.
- `actor/mod.rs`: `ActorSender::send` without the projection; the watermark kept by `Answering` or
  the actor (`seen_up_to`).
- `actor/user.rs`: keeps the last position shown; sends it.
- `actor/approval.rs`: unchanged in substance (its envelopes are recorded by the channel).
- `view/channel.rs`, `view/context.rs`, `view/participant.rs`: the predicates above; the
  projection as read; `AgentContextView` without `HistoryKey::Agent`.
- `controller/user.rs`: `mark_read` with the position from the document.
- `recorder.rs` and `new_recorder`: removed.
- `runtime/actor.rs`: `channel_entries` stays (what a joining agent reads); `channel_position_of`
  goes.
- `tests/security_suite.rs`: S4 (pages and messages as far as the view goes) over the new
  predicate; new: a participant does not read what went to another alone; a reader without an
  actor reads public entries only; the full form reaches neither another participant nor the
  moderator nor a reader of the channel, and reaches whom may read the agent's context.

`caelumora-agents`

- `actor/intake.rs`, `body.rs`, `agent.rs`, `external_agent.rs`, `turn/ending.rs`: as the section
  on the agent's history says; the watermark in `take_in`; `catch_up` from `HistoryShared`.
- `components/channel_history.rs`: public entries.
- `tests/security_suite.rs`: the agent's view through `AgentContextView` as above; `read_history`.

`caelumora-rest-api`, `caelumora-rest-server`, `caelumora-web-ui`

- The item form (decision point 1); `get_channel_history`, `agent_history`, `channel_snapshot` as
  they are over the views; the chat and the history panel render stored items and live events
  through one path, place own entries by `seen_up_to`, and no longer drop anything on reload.

`docs`: CHANNELS.md (what is recorded, whom to, the watermark, joining), STORAGE.md (one history,
the entry, the predicate, Postgres), AGENT-TURN.md (no history of the agent's; the transcript's
part), REST-SERVER.md (items, `position`, `seen_up_to`), WEB-UI.md (nothing gone on reload),
CONFIDENTIALITY.md (where the owner-only parts are held and withheld), SECURITY.md and
PERMISSION-CHECKS.md (the view predicate), EVENTS.md (`channel::SAID` carries the published form).

Stored data to delete: everything under `runtime/` - the channel states with their histories, the
agents' channel states with their histories and transcripts (a message's form changes).

## Plan

Each step is reviewable on its own; 1-3 change no behaviour.

1. **418**: the moderator returns a `Routing`; the channel builds every envelope.
2. **The entry with its delivery.** `HistoryEntry` as decided (point 1), `recipients`, `public`,
   `seen_up_to`, `position` stamped at delivery, the watermark sent by every actor; `enter` in the
   new order. The agents' histories still kept, in the new form. Store suite and S4 pass as
   before; a test that every entry of a channel with two users and an agent names the recipients
   the moderator chose.
3. **The views read by predicate.** A participant's page by the predicate; a reader without an
   actor by `public`; `HistoryShared` recorded at joining and read for the shared range;
   `HistoryLimit` as a position. No new entries yet, so every page reads as before. Security
   tests: one participant does not read another's.
4. **Everything shown is recorded.** The notices, requests, approvals, questions, rejections and
   command answers as entries; `MessageRejected` from `reject`; the REST items and the web UI
   render them from the history. Feature: nothing is gone on reload.
5. **The full form enters the channel.** `ActorSender::send` without the projection; the channel
   projects at delivery and the views at reading; `produced_from`, `ContextCompacted`,
   `ChannelReclassified`; `AgentContextView` reads the channel's history. Security tests for the
   three places the full form must not reach.
6. **The agents' histories go.** `AgentChannelState` without a history; readers moved to the
   transcript; `catch_up` from `HistoryShared`; `HistoryKey` to `ChannelId`; the stores without the
   agents' histories; the `Recorder` removed.
7. **Documents**, and 416 and 404's read mark as they stand after this.

## Done when

- Every envelope that enters the channel, every notice and request the channel sends and every
  refusal is one entry of the channel's history, with its recipients, whether it was public, its
  sender's watermark, and its position, which the delivered envelope carries.
- A participant's page of the history, the last page of their snapshot and the agent's history
  panel are read from the channel's history by the participant's predicate; a reader without an
  actor reads the public entries; the security suite checks all three and that the full form
  reaches only whom may read the sender's context.
- What went to a user alone is shown after a reload, and only to them; a message a moderator did
  not deliver to a user is not on their page.
- An agent's actor keeps no history; what it said in a turn and what it caught up with are read
  from the transcript and the channel.
- The store suite checks the predicate, `public` and the items against the chain folded.
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
  longer needed. The design and the plan above; two decision points - the entry's vocabulary and
  the order pages are read in - are recommended, not decided.
