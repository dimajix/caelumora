# 417 — On reload a user is shown the channel's history, not what their actor showed them

**Status:** in progress
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

1. **A moderator decides delivery, never content** ([418](418-moderator-decides-delivery-only.md),
   done). What a participant receives is what entered the channel, projected by the channel's one
   rule, or a notice the channel builds from the moderator's decision. The rule by which
   "everybody" is delivered is the channel's (`Envelope::concerns`).
2. **The channel keeps the one history, and every entry says whom it went to.** Every envelope
   that enters the channel - a message, a change, a notice, a request, a refusal, what an agent
   was given - is one entry, with its sender and its audience: everybody, by the channel's rule,
   or some alone, or nobody. What an actor received is the channel's history filtered by that
   actor; what it said is the channel's history filtered by its sender. There is no history of an
   agent's any more, and none of a user's is needed.
3. **The history records the conversation, not the audit.** An audit of acts - who released,
   approved, removed, deleted what and when - is one log of its own, without content, for an
   administrator ([140](140-audit-log.md)); the history is one of its sources. So approvals and
   questions are not entries: they are the state of a call, restored from the agent's turn on a
   reload, and who approved is kept at the call ([419](419-approval-recorded-with-the-call.md)).
   The envelope stream of [05](05-postgres-event-recorder.md) is, for a channel, this history.
4. **The channel holds everything, also what only an owner sees; what a reader is given of it is
   decided in one place.** An entry's audience says whether a reader gets it at all - what was
   given to an agent, a compaction, a message of which nothing is for others went to nobody. The
   visibility a message carries for its parts (`ToolVisibility`, `reasoning_visibility`) says
   which parts: the full form to its sender and to whom may read the sender's context, the
   published form (`Message::published`) to everyone else. One function applies both, to every
   delivery and to every read, for a reader that names itself; there is no read of the history
   without one.

Weighed and set aside: (A) the channel's history stays the only record and a moderator must
deliver everything to every user - the private notices stay lost; (B) a history per user beside
the agents' - a copy per participant, kept in step by every participant, which
[416](416-messages-stored-once-per-holder.md) would then have had to make affordable; (C) one
history with an audience per item, but the agents keep theirs - two records of one delivery; (E)
every kind of event as an entry, approvals and questions included - sixteen kinds for what the UI
shows and loses as six, and the approver's round trips as plumbing in the record; (F) what only an
owner sees stays with the owner - the sender publishes as today, the channel records an empty
shell where nothing is left to show, and the full form, what an agent was given and what its
messages were produced from are read from the agent's transcript. F was decided first and turned
round the same day: it needs shells, a second store behind every page of an agent's history, and
an entry for what was given that cannot carry what it gave; and its one argument - that what is not
in the channel cannot leak from it - holds just as well for a read that cannot bypass the rule of
decision 4.

Deferred to an issue of its own: an actor's own order among what it received. Every envelope would
carry the last position its sender had taken in (`seen_up_to`), so that an actor's own entries can
be placed after what it had seen - for the chat after a reload, and to tell a model that something
was said while it answered. It is additive: nothing here depends on it.

## The design

### The flow of an envelope

```
actor ── Answering ── OriginEventSender ──► channel inbox
   (as it is: what only its owner sees included; with what an agent's message was
    produced from - `Message::produced_from`)

channel, per envelope, in order (`ChannelLoop::enter`):
  1. admission      as today: whose it is, its classifications, what it changes
  2. mentions       as today
  3. decide         routing = moderator.route(published)      (418)
                    the moderator is given what everyone may see of the event, never more
  4. audience       nobody, if nothing of it is for others (published is None, or it is what
                    an agent was given or did to its context); else whom the routing names,
                    or everybody
  5. record         Recorded { entry, sender, audience } → seq; stored with the channel's state
  6. deliver        to every recipient what it may see of the entry (`Recorded::as_seen_by`),
                    stamped with seq - never back to the sender
  7. notices,       each recorded as an entry of its own, in the system's name, with its
     requests       audience, then delivered: ChainCutOff; ResponseRequested
  8. announce       channel::SAID on the event bus, with the published form (issue 404)
```

A rejection (`admission` refuses) does not enter the channel; what is recorded is the notice
`MessageRejected` to the sender alone, as an entry with that one recipient - so that a reload still
shows why something was not let in. What was rejected is not stored: nothing of it entered.

The channel is the one sequencer, as today: it records and routes in one step, every actor's inbox
is a queue, so the order in which an actor received things is the order of the entries addressed
to it. That is the requirement; nothing more is needed for it.

`ActorSender::send` no longer applies `ActorEvent::published`; a message of which nothing is left
to show is sent all the same. The `OriginEventSender` still passes on only what speaks as its
actor.

### The entry

```rust
/// What entered a channel, and whom it went to (issue 417): the chain's
/// entry, `History = Chain<Recorded>`.
pub struct Recorded {
    pub entry: HistoryEntry,
    /// The actor whose envelope it was - the agent for what it was given,
    /// the system for a notice.
    pub sender: ActorId,
    pub audience: Audience,
}

pub enum Audience {
    /// Everyone it concerns by the channel's rule (`Envelope::concerns`,
    /// read off the entry's kind - `HistoryEntry::concerns`).
    Everybody,
    /// These alone; nobody if empty.
    Only(Vec<ActorId>),
}
```

`HistoryEntry` stays the history's own vocabulary, an enum of what happened (today's twelve
variants); `Said` loses its `context`, which travels with the message (`Message::produced_from`).
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

What the agent's history recorded by itself becomes what the agent sends, recorded with an
audience of nobody:

- **What an agent was given** - a skill loaded for it, the summary and the skills a compaction gives
  again - is `ActorEvent::MessageGiven { message, knowledge }`, recorded as `Said` with the agent as
  `sender` and the message as it was given, its own sender the issuer or the agent. The message is
  not the envelope sender's, so it is no message the event carries (`ActorEvent::messages`), and
  admission checks its classification on its own, as it checks every message's.
- **A compaction** is `ActorEvent::ContextCompacted { turn_id, from, to }`, recorded as
  `Compacted`; the replacement follows as what it was given.
- **What an agent's message was produced from** (`ContextRef`) is `Message::produced_from`, set
  by the turn on what it sends - not on what it keeps in its transcript -, by an external agent
  from the call its answer came of; a part only the owner sees, withheld by `Message::published`.

### What is recorded, what is not

Recorded: everything an actor is shown that is complete, everything that asks an actor for
something, and what an agent was given or did to its context. Not recorded, as today: what streams
(`PartialMessageGenerated`, `IntermediatePartialGenerated`), a beginning whose end is recorded
(`ResponseStarted`, `PlanStepFinished`, `ActivityStarted`, `CommandStarted`), where an actor stands
(`StateChanged`), `WakeupRequested`, and - by decision 3 - approvals and questions, whose state the
agent's turn keeps and whose record is 419 and 140. `NudgeRequested` is recorded as the
`ResponseRequested` it becomes.

### Whose view: one rule for every read

```rust
/// Who reads a channel's history, and so what of an entry they are given.
pub enum Reader {
    /// A participant, now or formerly: what was delivered to them, what they
    /// sent, what was shared with them when they joined.
    Participant { actor: ActorId, shared: Vec<Range<u64>> },
    /// Whom may read `agent`'s context (`ReadAgentContext`): the agent's
    /// view, in full.
    ContextOf { agent: ActorId, shared: Vec<Range<u64>> },
    /// Someone who may read the channel but takes no part - its session's
    /// owner, a reader from another channel: what went to everybody.
    Outsider,
}

impl Recorded {
    /// What `reader` may see of the entry: nothing, the full form, or the
    /// published form.
    pub fn as_seen_by(&self, reader: &Reader) -> Option<Recorded>;
}
```

- **Whether**: the reader is among the audience (`Everybody` read by `HistoryEntry::concerns`, or
  in `Only`), or is its `sender`, or the entry went to everybody and lies in a range shared with
  them. An `Outsider` reads what went to everybody.
- **Which form**: the full one for the sender and for `ContextOf` the sender; `published` (and
  without `produced_from`) for everyone else.
- **What they may read by classification** stays the views' (`may_read_entry`), applied after.

The `HistoryStore` reads only for a reader: `page`, `said` and the entries a joining agent catches
up with take a `&Reader`, keep what it may see - as a `WHERE` over the entry's own fields (issue
332): `sender`, `everybody`, and the recipients of an `Only` entry - and project what they return
through `as_seen_by`. There is no read of the history without one; what reads the chain in memory
is the channel itself (its checks before a change, `given_up_last`) and needs no reader.

Every reader names itself:

| Read | Reader |
|---|---|
| a participant's page over REST, their snapshot, their unread count and read mark | `Participant` |
| the agent's history panel, `context_of`, `classification_of` | `ContextOf` |
| the session's owner who takes no part; `read_history` from another channel | `Outsider` |
| a joining agent catching up | `Participant` (itself) |
| the moderator | given the published form at step 3 |

`ChannelView::history_page`, `message`, `unread`, `participant` and `UserController::mark_read`
keep their signatures; what they pass to the store changes. In Postgres
([04](04-postgres-state-store.md)) `history_entries` and `history_items` carry `sender` and
`everybody`, and the recipients of an `Only` entry are a table `history_recipients(key, seq,
actor)`.

### Pages

A page is still the last `limit` items before a position, in the channel's order, of those the
reader may see, with `earlier` as the position of its oldest item (issue 218). Nothing changes for
a client: it passes `earlier` back. Each item carries its position (`seq`) now, and the live
envelope the position of the entry it stems from (`EventContext::position`, stamped by the channel
at delivery), so that a client tells a stored item from the live event that announced it by one
number instead of by a message id - which a notice has none of.

The snapshot's last page and `GET …/history` answer the same page.

### Joining: `HistoryShared`

Whoever adds a participant decides how much of the history they see (`HistoryAccess`, issue 46).
Today that is fixed as a `HistoryLimit` in the channel's document, and a joining agent reads what
it may see from the store itself (`catch_up`), into its history and transcript. With the delivery
recorded, catching up is a delivery too: the newcomer *received* what was shared with it.

The channel records `HistoryShared { participant, from: u64, by }` as it takes the participant in,
right before `ParticipantJoined`, delivered to the newcomer alone: "you may read this history from
position `from` on". A joining agent takes it in like every other entry: from it, it reads what
went to everybody from `from` on, as the reader it is, into its transcript. The views read the
newcomer's view by the same entry: the shared range is `[from, seq of the HistoryShared)`. A
participant added twice has two such entries; a range is never widened backwards past an entry the
participant sent, as today (CHANNELS.md, a rejoining actor).

The channel's document keeps the index `visible_from` needs without a scan of the chain, as the
position `from` instead of a message id (`HistoryLimit { participant, from: u64 }`) - what the
entry records, kept where it is read on every page.

### The agent's history goes; the transcript stays

`AgentChannelState::history` and `history_from`, `HistoryKey::Agent`, the agent's history files
and the `JsonFileStore`'s and `MemoryStore`'s keeping of them go. `HistoryKey` has one variant
left and becomes the `ChannelId`.

What read the agent's history in memory reads the transcript instead:

- `Ending::finish` (`caelumora-agents/src/turn/ending.rs`): the turn's messages
  (`History::said_in_turn`) - from the transcript's own messages with the turn's id, across its
  branches (`all_entries`, in the order added): a compaction in the middle of a turn moves on to a
  new branch, and the turn's earlier messages lie on the old one. The "last message id" the notes
  and the memory are anchored at is the transcript's last message.
- `AgentActor` on a retry (`agent.rs:328`): whether a message is one of its own - the transcript.
- `take_in`: a change of another's message is applied to the transcript alone; the channel's
  history already holds it.
- `catch_up`: the last message of its transcript that the channel holds says where to go on from,
  as its history's did; then the channel's entries from there, as the reader it is.
- `ExternalAgentActor` (`external_agent.rs:594`): what it said goes to its transcript alone, and
  to the channel with what it was produced from.
- `ChannelLoop::given_up_last` reads the channel's chain and asks whether a turn's end is "the last
  thing": with notices in the chain, the last thing *said or done in the conversation* - a
  `ResponseRequested` or `RetryRejected` after it does not count. One rule,
  `HistoryEntry::is_conversation`.

The transcript is what the model was given: what passed the perceive processors, what was given to
read, its own messages in full, compacted as it was. It is not a record of what happened in the
channel and is not touched here.

## Where it carries, where it may not

Carries:

- One record for live and reload, for users and agents alike; what went to one actor alone is
  there after a reload, for that actor; what an agent saw and said is in the same record, in the
  same order, for whom may read its context.
- The copies of every message in every agent's history go, and with them the ways an agent's record
  could differ from the channel's (`catch_up`, a lost state), the agent's window of its history
  (issue 387) and `Envelope::announced`.
- The `Recorder` (`record_send`, `record_receive`, only ever a `BlackholeRecorder`, issue 05) is
  what this record is; it goes.

Where it may not, and what is done about it:

1. **The guarantee moves from the sender to the reader.** Today nothing only an owner sees ever
   enters the channel; now it does, and `as_seen_by` keeps it in. It holds only if no read
   bypasses it: the store takes a `Reader` on every read, the channel delivers through
   `as_seen_by`, and the security suite checks the places that must not see the full form - another
   participant live and on a page, the moderator, an outsider, `read_history`, a joining agent - and
   that whom may read the context does.
2. **What only the owner sees is kept twice**, in the channel and in the agent's transcript - as
   much as the agent's history and the transcript keep today, not more.
3. **The agent's own turn is read from the transcript across branches.** Low risk: `all_entries`
   iterates in the order added, over every branch; a test with a compaction in the middle of a
   turn.
4. **The chain grows with notices and requests.** A `ResponseRequested` per answer, a notice per
   refusal; small entries. Pages are read by what the reader may see, so a user's page costs what
   it costs today plus what is skipped; the chain's size is not a concern (issue 379 made it so).
5. **`given_up_last` must skip the notices** (`is_conversation`), or "answer again" fails after a
   `RetryRejected`.
6. **The REST items and the web UI** are the real work of the feature: six new item kinds rendered
   from the history, and the chat's snapshot path and live reducer agreeing on them.
7. **`HistoryPage` items carry no position today**; they will, which the client needs anyway.

## What changes

`caelumora-model`

- `history`: `Recorded`, `Audience`, `Reader`, `as_seen_by`, `HistoryEntry::concerns` (the rule of
  `Envelope::concerns`, read off an entry), `is_conversation`; `History = Chain<Recorded>`; `fold`,
  `StandingItems`, `page_of` over it; the new variants; `Said` without `context`; `said_in_turn`
  and `turn_began` move to the transcript (`Transcript::said_in_turn`).
- `actor`: `MessageGiven`, `ContextCompacted`, `HistoryShared`; `EventContext::position`;
  `Envelope::announced` becomes the one place an entry is built from an envelope.
- `message`: `Message::produced_from`, withheld by `published`.
- `channel`: `HistoryLimit { participant, from: u64 }`; `history_start_of` answers a position.
- `agent`: `AgentChannelState` without `history`, `history_from`; `said`, `perceive`,
  `note_announced`, `given`, `compact`, `finish_turn`, `give_up_turn` write the transcript alone;
  `keep_window` without the history.
- `store`: `HistoryKey` → `ChannelId`; every read of `HistoryStore` takes a `&Reader`;
  `StateStore::put_agent_channel_state` without a history; `MemoryStore`, `JsonFileStore`
  (`histories.rs`) without the agents' histories; the store suite: what each reader may see,
  `Everybody`, `Only`, the projection, the items equal the chain folded.

`caelumora-runtime`

- `channel/run.rs`: `enter` as the flow says - project for the moderator, decide, audience, record,
  deliver through `as_seen_by`, notices and requests as entries; `reject` records
  `MessageRejected`; `append_history` the one place that builds an entry from an envelope and its
  routing; `position` stamped at delivery.
- `channel/participants.rs`, `mod.rs`: `take_in` records `HistoryShared`; `given_up_last` by
  `is_conversation`.
- `channel/history.rs`: `visible_from` from the position in the document.
- `actor/mod.rs`: `ActorSender::send` without the projection.
- `view/channel.rs`, `view/context.rs`, `view/participant.rs`: each read with its `Reader`;
  `AgentContextView` over the channel's history.
- `controller/user.rs`: `mark_read` as a `Participant`.
- `recorder.rs` and `new_recorder`: removed.
- `runtime/actor.rs`: `channel_entries` as the agent's reader; `channel_position_of` stays for
  `catch_up`.
- `tests/security_suite.rs`: as said under "where it may not", 1.

`caelumora-agents`

- `actor/intake.rs`, `body.rs`, `agent.rs`, `external_agent.rs`, `turn/mod.rs`, `turn/ending.rs`:
  as the section on the agent's history says; `MessageGiven` and `ContextCompacted` sent where the
  state records them; `produced_from` on what a turn says.
- `components/channel_history.rs`: as an `Outsider`.
- `tests/security_suite.rs`: the agent's view through `AgentContextView`; `read_history`.

`caelumora-rest-api`, `caelumora-rest-server`, `caelumora-web-ui`

- The new item kinds in `HistoryPageResponse`, with `seq`; `get_channel_history`, `agent_history`,
  `channel_snapshot` as they are over the views; the chat and the history panel render the six
  notices from the history and no longer drop anything on reload.

`docs`: CHANNELS.md (what is recorded, whom to, joining), STORAGE.md (one history, the entry, the
reader, Postgres), AGENT-TURN.md (no history of the agent's; the transcript's part),
REST-SERVER.md (items with `seq`, `position` on the stream), WEB-UI.md (nothing gone on reload),
CONFIDENTIALITY.md (where what only the owner sees is held, and how it is withheld: decision 4),
SECURITY.md and PERMISSION-CHECKS.md (the reader of a history).

Stored data to delete: everything under `runtime/` - the channel states with their histories, the
agents' channel states with their histories and transcripts.

## Plan

Each step is reviewable on its own; 1-2 change no behaviour.

1. **418**, done: the moderator returns a `Routing`; the channel owns the rule of "everybody" and
   builds every envelope.
2. **The entry and its reader.** `Recorded` with `sender` and `Audience`; `Reader` and
   `as_seen_by`; every read of the `HistoryStore` names its reader. Everything recorded so far goes
   to everybody, so every page reads as before; the agents' histories are still kept, as
   `Chain<Recorded>` with the agent as reader of its own. Store suite and S4 pass as before.
3. **The full form enters the channel.** `ActorSender::send` without the projection; the channel
   projects for the moderator and delivers through `as_seen_by`; a message of which nothing is for
   others recorded with an audience of nobody. Security tests for every reader that must not see
   the full form, and for the one that must.
4. **The agents' histories go.** `MessageGiven`, `ContextCompacted`, `produced_from`;
   `AgentChannelState` without a
   history; readers moved to the transcript; `catch_up` as the agent's reader; `AgentContextView`
   over the channel's history; `HistoryKey` to `ChannelId`; the stores without the agents'
   histories; the `Recorder` removed.
5. **Joining.** `HistoryShared` recorded at joining and read for the shared range; `HistoryLimit`
   as a position.
6. **Everything shown is recorded.** The six notices and `ResponseRequested` as entries;
   `MessageRejected` from `reject`; `CommandExecuted` with its output; `position` stamped at
   delivery, items with `seq`; the REST items and the web UI render them from the history.
   Feature: nothing is gone on reload.
7. **Who sees what is a predicate of the store.** Whom an entry went to is resolved as it is
   recorded, into what a store can filter by: `sender`, the audience - every member at the time
   of these kinds (users, agents, both), or these alone - and whether anything of it is published
   for others. Membership is read from the chain alone: every member has its `ParticipantJoined`,
   `ParticipantLeft` carries the position from which the leaver was given nothing. Beside the
   items the store derives each member's visible ranges - from the `from` of its `HistoryShared`
   to the end of its membership - rebuilt with them. A page counts what its reader sees, and
   `HistoryStore::page` takes no `from`; `HistoryLimit` goes. `HistoryShared` is recorded only
   where something is shared, in the system's name. Depends on every founder joining by the one
   way in ([420](420-channel-built-empty.md)).
8. **Documents**, and 05, 140 and 404's read mark as they stand after this.

## Done when

- Every envelope that enters the channel, every notice and request the channel sends, every refusal
  and what an agent was given is one entry of the channel's history, with its sender, its audience
  and its position, which the delivered envelope carries.
- Every read of the history names its reader, and what it returns is what `as_seen_by` lets that
  reader see; the security suite checks that the full form reaches only the sender and whom may
  read its context, live and on every page.
- What went to a user alone is shown after a reload, and only to them; a message a moderator did
  not deliver to a user is not on their page.
- An agent's actor keeps no history; what it said in a turn and what it caught up with are read
  from the transcript and the channel; its history panel and the context of its messages are read
  from the channel's history.
- The store suite checks the readers, the audience, the projection and the items against the chain
  folded.
- Which entries a reader is given is a predicate over what a row holds and the reader's visible
  ranges - one `WHERE` for a database -, and a page counts the entries its reader sees.
- The documents named above describe the one history and its reader.

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
- 2026-10-17: [418](418-moderator-decides-delivery-only.md) is done without `Audience`: a
  moderator answers `deliver_to: Option<Vec<ActorId>>`, and the rule of "everybody" is
  `Envelope::concerns`. `Audience` is introduced here, in `caelumora-model`, with the entry that
  records it; `delivered_to(kind, V)` in the predicate above is `Envelope::concerns` read off the
  entry.
- 2026-10-17: Decision 4 turned round as work began, and the plan reordered. Removing the agents'
  histories showed what keeping the owner's parts out of the channel costs: shells, an entry for
  what an agent was given that could not carry it, and every page of an agent's history read from
  two stores. The maintainer asked why a message given to an agent should not be carried by its
  entry, when its audience - the agent alone - already says nobody else gets it. It should: the
  audience protects a whole entry as well as its absence did, once every read applies it. So the
  channel holds everything, and one function (`Recorded::as_seen_by`) decides for a named reader
  whether they get an entry and in which form; the store reads only for a reader (decision 4, F
  set aside). The context of a message travels with it (`Message::produced_from`), not in the
  transcript. The entry is a struct around the enum (`Recorded`); the steps are reordered so that
  the entry and its reader come first, and the agents' histories go before joining and the
  notices.
- 2026-10-17: Step 2 done on `issue-417-one-channel-history`. `History = Chain<Recorded>`;
  `Audience` and `Reader` in `caelumora-model::history`; `Recorded::as_seen_by` is the one rule,
  applied by both stores to `page`, `said` and `entries`, which take a `&Reader`. The rule of
  "everybody" read off an entry is `HistoryEntry::concerns`; a test holds it to
  `Envelope::concerns` for every event the history records. What the history records of an
  envelope is built in one place, `Envelope::entry`. The channel asks the moderator before it
  records (`ChannelLoop::decide`), so that the entry carries the moderator's audience, and records
  through `History::enter`; a `Routing` names its `Audience`, now that the enum exists. The digest
  of an entry covers its sender and audience. Readers named: a participant's page, message,
  unread count and read mark as `Participant`; a session's owner without an actor and a reader
  from another channel as `Outsider`; the channel's own checks and where a view begins as
  `Channel`; a joining agent as itself; what an agent saw as the agent. Moved to step 6, where the
  client needs them: the position stamped on a delivered envelope and items with `seq`. One
  behaviour changes already: an agent reading its own channel through the views - `read_history`
  naming its own channel - is given what went to it and what it said, no longer the steps and
  turn ends of other agents; users read as before, since every entry concerns them.
- 2026-10-17: Step 3 done. `ActorSender::send` sends an event as it is; the channel projects it
  once as it enters (`Envelope::published`): the moderator decides on the published form, which
  is what is delivered and announced on the event bus (`channel::SAID`); the history records the
  event whole, and an event with nothing left to show as gone to nobody, without asking the
  moderator. Tested in the channel - the moderator and a participant live are given the
  published form, the record holds the whole -, and in the agents' security suite (C1) with an
  agent whose reasoning is its owner's: not to bob live, on a page or by its id, not to an agent
  that joins later; the record holds it for the agent. `produced_from` moved to step 4, where the
  context of a message is first read from the channel.
- 2026-10-17: Step 4 done. `AgentChannelState` keeps no history; `HistoryKey` is gone, the
  `HistoryStore` is keyed by `ChannelId`; both stores keep no agents' histories; the `Recorder`
  is removed. An agent's own message carries what it was produced from (`Message::produced_from`,
  set on what `AgentChannelState::said` returns for sending, not on what the transcript keeps,
  withheld by `Message::published`); `HistoryEntry::Said` lost its `context`. What an agent was
  given and its compactions are told to the channel after every step (`AgentChannelState::to_tell`,
  sent by `AgentBody::tell_recorded`, and before a command's answer) as `MessageGiven` and
  `ContextCompacted`, admitted only from the agent in its own name - what it was given only if its
  classification may flow in -, recorded as gone to nobody. `AgentContextView` reads the channel's
  history as the agent, from where its view begins. Four things differ from the design above:
  - **The turn's answer is kept with the turn** (`TurnState::answer`), not read from the transcript
    across its branches: the agent holds only the transcript's active branch, so a compaction in
    the middle of a turn leaves the turn's earlier messages nowhere it could read them.
  - **What an agent was given is an entry of its own** (`HistoryEntry::Given`), not `Said`: as a
    message said, it would be found by its id, and its sender - a skill's issuer - could modify or
    revoke it over REST. `Given` is no message said: found by no id, changed by nobody, read only
    by whom may read what the agent saw; REST shows it in an agent's history as before.
  - **`MessageGiven` carries no knowledge**: the channel's record does not need it; the turn
    records the knowledge it read in its context, as before.
  - **An agent remembers the last message it took in** (`AgentChannelState::last_message`), heard,
    said or given: `catch_up` goes on after it, and a turn's changes to the agent's state are
    anchored at it. The transcript cannot tell: its active branch begins anew with every call of
    an external agent and every compaction, so an agent joining again was given what it had
    heard before.
- 2026-10-17: Step 5 done. As a participant's actor starts - added, joining, or an agent restored
  -, the channel records `HistoryShared { participant, from, by }` right before its
  `ParticipantJoined`, sent in the name of who granted it and delivered to the newcomer alone
  (`Envelope::concerns`, `HistoryEntry::concerns`); no actor may send it (`is_announcement`). The
  channel's document keeps the position (`HistoryLimit { participant, from: u64 }`,
  `Channel::history_from`): the grant is read off the messages once, as the participant joins,
  and every read of its view applies the position without reading the history - the case of a
  view whose first message is gone is gone with it. `KnowledgeSource::History` and the REST
  field `history_start` carry the position. Tested in the channel (recorded right before the
  join, from the position kept, read by the newcomer and its sender alone) and in S4. Where it
  differs from the design above:
  - **The reader carries no ranges.** `Reader::Participant` stays the actor; the view begins at
    the one position its document keeps, as before. What went to everybody while a participant
    was away is read again only as far as a new grant reaches - the earlier stint's is not kept
    apart; that needs the delivery of an entry to everybody to name who took part then.
  - **Catching up stays the actor's**, before it starts (`Actor::catch_up`), not taken from the
    delivered `HistoryShared`: a restored agent and one started with its channel catch up the
    same way, and the agents' filters let the event pass by.
  - **Who granted it reads it**, as its sender; the clients do not show it yet (step 6).
- 2026-10-17: Step 6 done. Everything shown is recorded: `CommandExecuted` with its output and
  whom it was answered to, the session's too; `CommandFailed`, `RetryAccepted`, `RetryRejected` as
  their actors send them; `ChainCutOff` and `ResponseRequested` recorded by the channel in the
  system's name, to whom the moderator names or asks, before they are delivered; `MessageRejected`
  from `reject`, to the sender alone. `HistoryEntry::is_conversation` leaves notices and requests
  out of "the last thing" (`turn_given_up_last`). What the channel delivers carries the position
  of its entry (`EventContext::position`); a page's items carry theirs (`HistoryPage::items` as
  `(seq, entry)`, over REST `{"seq": …, <kind>: …}`). The REST items and the web UI show the
  notices from the history, and the chat keeps each once by `entry:<position>`. Tested: the
  entries' rule against the envelopes' for every new kind, a turn given up found past a notice, the
  rejections recorded to their sender alone in the system's name, the request recorded after what
  it answers, the positions delivered, a command's failure to its issuer alone, and in the agents'
  security suite a private answer on its issuer's page alone. Where it differs from the design
  above:
  - **`ResponseRequested` carries only `by`**: whom it asked is its audience, and what it answers
    is the entry before it. A quiet channel asks nobody, and records no request.
  - **The rule of delivery is read off the entry**, so `CommandExecuted` and `CommandFailed` carry
    `to`. A private answer to a command is no longer on the pages of other users, who were not
    shown it either; before, they read the command without its answer.
  - **A message of a user's that was not let in** is marked live, as before; after a reload, the
    notice of its refusal stands in its place - the message entered nothing.
  - **A change of a message counts as conversation**: a retry after a turn given up is refused
    once something was changed after it, as before.
- 2026-10-18: Step 7 added to the plan, the documents move to step 8. Who sees what was decided
  by `as_seen_by` after the read - a `match` over the kind of entry and the kind of reader - which
  no database can filter by, and a page passed over what its reader does not see. Decided: the
  audience is resolved as an entry is recorded, membership is read from the chain alone
  (`ParticipantJoined` for every member, `ParticipantLeft` with the position from which the leaver
  was given nothing, so that issue 262's order stays), the store keeps each member's visible
  ranges beside the items, and a page counts what its reader sees. That founders join by the same
  way as everybody else - a channel built empty, its participants added - and that `Keep` becomes
  data a store can filter by are issues of their own (420, 421).
